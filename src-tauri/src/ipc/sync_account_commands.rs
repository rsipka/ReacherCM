use std::{
    collections::BTreeMap,
    net::Ipv4Addr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::{header, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{oneshot, Mutex, Notify},
};
use url::Url;
use zeroize::Zeroizing;

const CLIENT_ID: &str = "reachercm-desktop";
const TIMEOUT: Duration = Duration::from_secs(600);
type Result<T> = std::result::Result<T, &'static str>;
const SERVER_ERROR: &str = "sync_account.error_server";
const REVOKED: &str = "sync_account.error_revoked";
const KEYCHAIN_ERROR: &str = "sync_account.error_keychain";
const CALLBACK_ERROR: &str = "sync_account.error_callback";
const CANCELED: &str = "sync_account.canceled";

static ACTION: Mutex<()> = Mutex::const_new(());
static CANCEL: Notify = Notify::const_new();
static ACCESS: Mutex<Option<CachedAccess>> = Mutex::const_new(None);

struct CachedAccess {
    server: Url,
    device_id: String,
    token: Zeroizing<String>,
    valid_until: Instant,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub(crate) server: String,
    pub(crate) username: String,
    device_id: String,
}

#[derive(Deserialize, Serialize)]
struct StoredAccount {
    account: AccountView,
    refresh: String,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    device_id: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct Identity {
    username: String,
    device_id: String,
}

pub(crate) fn origin(input: &str) -> Result<Url> {
    const ERROR: &str = "sync_account.error_url";
    let url = Url::parse(input.trim()).map_err(|_| ERROR)?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    };
    let allowed_scheme = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    if !allowed_scheme
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ERROR);
    }
    Ok(url)
}

fn endpoint(origin: &Url, path: &str) -> Url {
    let mut url = origin.clone();
    url.set_path(path);
    url
}

fn http(origin: &Url) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none());
    if origin.scheme() == "http" {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|_| SERVER_ERROR)
}

fn entry() -> Result<keyring_core::Entry> {
    crate::vault::manager::keychain_entry_in("reachercm-sync-account", "desktop")
        .map_err(|_| KEYCHAIN_ERROR)
}

fn saved() -> Result<Option<StoredAccount>> {
    match entry()?.get_password() {
        Ok(secret) => serde_json::from_str(&Zeroizing::new(secret))
            .map(Some)
            .map_err(|_| KEYCHAIN_ERROR),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(_) => Err(KEYCHAIN_ERROR),
    }
}

fn save(account: &StoredAccount) -> Result<()> {
    let secret = Zeroizing::new(serde_json::to_string(account).map_err(|_| KEYCHAIN_ERROR)?);
    entry()?.set_password(&secret).map_err(|_| KEYCHAIN_ERROR)
}

fn forget() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(_) => Err(KEYCHAIN_ERROR),
    }
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn valid_token(value: &str) -> bool {
    value.len() == 43
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|bytes| bytes.len() == 32)
}

type CallbackResult = Result<String>;

struct Callback {
    state: String,
    code: Mutex<Option<oneshot::Sender<CallbackResult>>>,
}

async fn receive_callback(State(flow): State<Arc<Callback>>, uri: Uri) -> impl IntoResponse {
    let result = callback_code(uri.query().unwrap_or_default(), &flow.state);
    let (status, message) = match result {
        Some(result) => match flow.code.lock().await.take() {
            Some(sender) => {
                let text = if result.is_ok() {
                    "Approval received. Return to Reacher CM to finish signing in."
                } else {
                    "Sign-in canceled. You can return to Reacher CM."
                };
                let _ = sender.send(result);
                (StatusCode::OK, text)
            }
            None => (StatusCode::GONE, "This sign-in request has ended."),
        },
        None => (StatusCode::BAD_REQUEST, "This sign-in response is invalid."),
    };
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        message,
    )
}

fn callback_code(query: &str, expected_state: &str) -> Option<CallbackResult> {
    let (mut state, mut code, mut error) = (None, None, None);
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let field = match name.as_ref() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if field.replace(value).is_some() {
            return None;
        }
    }
    if state.as_deref() != Some(expected_state) {
        return None;
    }
    match (code, error) {
        (Some(code), None) if valid_token(&code) => Some(Ok(code.into_owned())),
        (None, Some(error)) if error == "access_denied" => Some(Err(CANCELED)),
        _ => None,
    }
}

async fn post_form(
    client: &Client,
    origin: &Url,
    path: &str,
    fields: &[(&str, &str)],
) -> Result<Response> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish();
    client
        .post(endpoint(origin, path))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|_| SERVER_ERROR)
}

async fn tokens(client: &Client, origin: &Url, fields: &[(&str, &str)]) -> Result<Tokens> {
    let response = post_form(client, origin, "/oauth/token", fields).await?;
    if response.status() == StatusCode::BAD_REQUEST {
        let body = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| SERVER_ERROR)?;
        return Err(if body["error"] == "invalid_grant" {
            REVOKED
        } else {
            SERVER_ERROR
        });
    }
    let value = response
        .error_for_status()
        .map_err(|_| SERVER_ERROR)?
        .json::<Tokens>()
        .await
        .map_err(|_| SERVER_ERROR)?;
    if !valid_token(&value.access_token)
        || !valid_token(&value.refresh_token)
        || !valid_token(&value.device_id)
    {
        return Err(SERVER_ERROR);
    }
    Ok(value)
}

async fn identity(client: &Client, server: &Url, tokens: &Tokens) -> Result<Identity> {
    let response = client
        .get(endpoint(server, "/api/v1/me"))
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .map_err(|_| SERVER_ERROR)?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(REVOKED);
    }
    let identity = response
        .error_for_status()
        .map_err(|_| SERVER_ERROR)?
        .json::<Identity>()
        .await
        .map_err(|_| SERVER_ERROR)?;
    if identity.device_id != tokens.device_id || identity.username.is_empty() {
        return Err(SERVER_ERROR);
    }
    Ok(identity)
}

async fn revoke(client: &Client, server: &Url, refresh: &str) -> bool {
    post_form(
        client,
        server,
        "/oauth/revoke",
        &[("client_id", CLIENT_ID), ("token", refresh)],
    )
    .await
    .is_ok_and(|response| response.status().is_success())
}

async fn browser_code(server: &Url, label: &str, verifier: &str) -> Result<(String, String)> {
    let cancel = CANCEL.notified();
    tokio::pin!(cancel);
    cancel.as_mut().enable();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| CALLBACK_ERROR)?;
    let address = listener.local_addr().map_err(|_| CALLBACK_ERROR)?;
    let redirect = format!("http://{address}/oauth/callback");
    let state = random_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let platform = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        _ => "Linux",
    };
    let mut authorization = endpoint(server, "/oauth/authorize");
    authorization.query_pairs_mut().extend_pairs([
        ("client_id", CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", redirect.as_str()),
        ("state", state.as_str()),
        ("code_challenge_method", "S256"),
        ("code_challenge", challenge.as_str()),
        ("device_name", label),
        ("platform", platform),
    ]);
    let (send_code, receive_code) = oneshot::channel();
    let (shutdown, stopped) = oneshot::channel();
    let router = Router::new()
        .route("/oauth/callback", get(receive_callback))
        .with_state(Arc::new(Callback {
            state,
            code: Mutex::new(Some(send_code)),
        }));
    tokio::spawn(async move {
        let server = axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = stopped.await;
        });
        let _ = tokio::time::timeout(TIMEOUT + Duration::from_secs(5), server).await;
    });
    // Dropping shutdown also stops the listener if opening the browser fails.
    tauri_plugin_opener::open_url(authorization.as_str(), None::<&str>)
        .map_err(|_| "sync_account.error_browser")?;
    let result = tokio::select! {
        _ = &mut cancel => Err(CANCELED),
        result = tokio::time::timeout(TIMEOUT, receive_code) =>
            result.map_err(|_| "sync_account.error_expired")?.map_err(|_| CALLBACK_ERROR)?,
    };
    let _ = shutdown.send(());
    Ok((result?, redirect))
}

#[tauri::command]
pub fn sync_account_status() -> Result<Option<AccountView>> {
    Ok(saved()?.map(|account| account.account))
}

#[tauri::command]
pub async fn sync_account_sign_in(server: String, label: String) -> Result<AccountView> {
    let _action = ACTION.try_lock().map_err(|_| "sync_account.busy")?;
    if saved()?.is_some() {
        return Err("sync_account.error_existing");
    }
    let server = origin(&server)?;
    let http = http(&server)?;
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
        return Err("sync_account.error_label");
    }
    let verifier = Zeroizing::new(random_token());
    let (code, redirect) = browser_code(&server, label, &verifier).await?;
    let code = Zeroizing::new(code);
    let value = tokens(
        &http,
        &server,
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ],
    )
    .await?;
    let identity = match identity(&http, &server, &value).await {
        Ok(identity) => identity,
        Err(err) => {
            revoke(&http, &server, &value.refresh_token).await;
            return Err(err);
        }
    };
    let account = AccountView {
        server: server.to_string(),
        username: identity.username,
        device_id: identity.device_id,
    };
    let stored = StoredAccount {
        account: account.clone(),
        refresh: value.refresh_token.clone(),
    };
    if let Err(err) = save(&stored) {
        revoke(&http, &server, &stored.refresh).await;
        return Err(err);
    }
    remember_access(&server, value).await;
    Ok(account)
}

#[tauri::command]
pub fn sync_account_cancel() {
    CANCEL.notify_waiters();
}

#[tauri::command]
pub async fn sync_account_check() -> Result<AccountView> {
    let _action = ACTION.try_lock().map_err(|_| "sync_account.busy")?;
    let mut stored = saved()?.ok_or(REVOKED)?;
    let server = origin(&stored.account.server)?;
    let http = http(&server)?;
    let fresh = refresh_access(&http, &server, &mut stored).await?;
    match identity(&http, &server, &fresh).await {
        Ok(identity) if identity.username == stored.account.username => {
            remember_access(&server, fresh).await;
            Ok(stored.account)
        }
        Err(REVOKED) => {
            clear_account().await?;
            Err(REVOKED)
        }
        Err(error) => Err(error),
        Ok(_) => Err(SERVER_ERROR),
    }
}

#[tauri::command]
pub async fn sync_account_sign_out() -> Result<bool> {
    let _action = ACTION.try_lock().map_err(|_| "sync_account.busy")?;
    let Some(stored) = saved()? else {
        return Ok(true);
    };
    clear_account().await?;
    let Ok(server) = origin(&stored.account.server) else {
        return Ok(false);
    };
    let Ok(http) = http(&server) else {
        return Ok(false);
    };
    Ok(revoke(&http, &server, &stored.refresh).await)
}

#[tauri::command]
pub fn sync_account_manage() -> Result<()> {
    let stored = saved()?.ok_or(REVOKED)?;
    let server = origin(&stored.account.server)?;
    tauri_plugin_opener::open_url(endpoint(&server, "/devices").as_str(), None::<&str>)
        .map_err(|_| "sync_account.error_browser")
}

async fn clear_account() -> Result<()> {
    *ACCESS.lock().await = None;
    forget()
}

async fn refresh_access(
    client: &Client,
    server: &Url,
    stored: &mut StoredAccount,
) -> Result<Tokens> {
    let fresh = match tokens(
        client,
        server,
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", &stored.refresh),
        ],
    )
    .await
    {
        Ok(tokens) => tokens,
        Err(REVOKED) => {
            clear_account().await?;
            return Err(REVOKED);
        }
        Err(err) => return Err(err),
    };
    if fresh.device_id != stored.account.device_id {
        return Err(SERVER_ERROR);
    }
    stored.refresh.clone_from(&fresh.refresh_token);
    if let Err(err) = save(stored) {
        revoke(client, server, &stored.refresh).await;
        return Err(err);
    }
    Ok(fresh)
}

async fn remember_access(server: &Url, value: Tokens) -> Zeroizing<String> {
    let token = Zeroizing::new(value.access_token);
    // Refresh a short time before expiry so a request has time to reach the server.
    let valid_until =
        Instant::now() + Duration::from_secs(value.expires_in.min(900).saturating_sub(30));
    *ACCESS.lock().await = Some(CachedAccess {
        server: server.clone(),
        device_id: value.device_id,
        token: token.clone(),
        valid_until,
    });
    token
}

#[derive(Deserialize, Serialize)]
pub struct PreferencePatch {
    revision: u64,
    changes: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize)]
pub struct PreferenceSnapshot {
    revision: u64,
    values: BTreeMap<String, String>,
    #[serde(skip_deserializing)]
    conflict: bool,
}

#[tauri::command]
pub async fn sync_account_preferences(
    device_id: String,
    patch: Option<PreferencePatch>,
) -> Result<PreferenceSnapshot> {
    let response = account_request(
        |account| account.device_id == device_id,
        |http, server| {
            let url = endpoint(server, "/api/v1/preferences");
            match &patch {
                Some(patch) => http.patch(url).json(patch),
                None => http.get(url),
            }
        },
    )
    .await?;
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::CONFLICT {
        return Err(SERVER_ERROR);
    }
    let mut snapshot = response
        .json::<PreferenceSnapshot>()
        .await
        .map_err(|_| SERVER_ERROR)?;
    snapshot.conflict = status == StatusCode::CONFLICT;
    Ok(snapshot)
}

pub(crate) async fn vault_request(
    server: &Url,
    username: &str,
    request: impl Fn(&Client) -> reqwest::RequestBuilder,
) -> Result<Response> {
    account_request(
        |account| account.username == username && account.server == server.as_str(),
        |http, _| request(http),
    )
    .await
}

async fn account_request(
    matches: impl Fn(&AccountView) -> bool,
    request: impl Fn(&Client, &Url) -> reqwest::RequestBuilder,
) -> Result<Response> {
    let _action = ACTION.try_lock().map_err(|_| "sync_account.busy")?;
    let mut stored = saved()?.ok_or(REVOKED)?;
    if !matches(&stored.account) {
        return Err(REVOKED);
    }
    let server = origin(&stored.account.server)?;
    let http = http(&server)?;
    let cached = ACCESS
        .lock()
        .await
        .as_ref()
        .filter(|entry| {
            entry.server == server
                && entry.device_id == stored.account.device_id
                && entry.valid_until > Instant::now()
        })
        .map(|entry| entry.token.clone());
    let mut access = match cached {
        Some(token) => token,
        None => remember_access(&server, refresh_access(&http, &server, &mut stored).await?).await,
    };
    for attempt in 0..2 {
        let request = request(&http, &server);
        let response = request
            .bearer_auth(access.as_str())
            .send()
            .await
            .map_err(|_| SERVER_ERROR)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            if attempt == 0 {
                access =
                    remember_access(&server, refresh_access(&http, &server, &mut stored).await?)
                        .await;
                continue;
            }
            clear_account().await?;
            return Err(REVOKED);
        }
        return Ok(response);
    }
    Err(SERVER_ERROR)
}

#[cfg(test)]
pub(crate) async fn seed_vault_test_account(server: &str, username: &str) -> AccountView {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = origin(server).unwrap();
    let account = AccountView {
        server: server.to_string(),
        username: username.into(),
        device_id: random_token(),
    };
    let refresh = random_token();
    save(&StoredAccount {
        account: account.clone(),
        refresh: refresh.clone(),
    })
    .unwrap();
    remember_access(
        &server,
        Tokens {
            access_token: random_token(),
            refresh_token: refresh,
            device_id: account.device_id.clone(),
            expires_in: 900,
        },
    )
    .await;
    account
}

#[cfg(test)]
pub(crate) async fn clear_vault_test_account() {
    clear_account().await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_origins_require_https_or_loopback() {
        assert!(origin("https://sync.example.com").is_ok());
        assert!(origin("http://127.0.0.1:8080").is_ok());
        for invalid in [
            "http://sync.example.com",
            "https://user@sync.example.com",
            "https://sync.example.com/base",
            "https://sync.example.com/#token",
        ] {
            assert!(origin(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn callback_requires_one_matching_state_and_one_valid_result() {
        let state = random_token();
        let code = random_token();
        assert_eq!(
            callback_code(&format!("state={state}&code={code}"), &state),
            Some(Ok(code.clone()))
        );
        assert_eq!(
            callback_code(&format!("error=access_denied&state={state}"), &state),
            Some(Err(CANCELED))
        );
        assert_eq!(
            callback_code(&format!("state=other&code={code}"), &state),
            None
        );
        assert_eq!(
            callback_code(&format!("state={state}&state=other&code={code}"), &state),
            None
        );
    }
}
