//! A small Proxmox API client: HTTPS with the session's pinned
//! certificates, and one of two logins. An API token goes in a header on
//! every request; a user login (password, then a TOTP code or recovery key
//! when the account has a second factor) gives a ticket, sent as a cookie
//! with a CSRF token on changes, and renewed before its two hours run out.
//!
//! Proxmox VE and Backup Server share the API's shape (`/api2/json`, a
//! `data` field) and differ only in the names the login goes by.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A ticket lasts two hours; it is renewed once it is older than this.
const RENEW_AFTER: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Product {
    /// Proxmox Virtual Environment.
    Pve,
    /// Proxmox Backup Server.
    Pbs,
}

impl Product {
    /// What the login cookie is called.
    fn cookie(self) -> &'static str {
        match self {
            Product::Pve => "PVEAuthCookie",
            Product::Pbs => "PBSAuthCookie",
        }
    }

    /// The `Authorization` header for an API token: `PVEAPIToken=id=secret`
    /// or `PBSAPIToken=id:secret`.
    pub fn token_header(self, id: &str, secret: &str) -> String {
        match self {
            Product::Pve => format!("PVEAPIToken={id}={secret}"),
            Product::Pbs => format!("PBSAPIToken={id}:{secret}"),
        }
    }

    /// The product a port is the default of: 8007 is Backup Server's.
    pub fn for_port(port: u16) -> Self {
        if port == 8007 { Product::Pbs } else { Product::Pve }
    }
}

#[derive(Debug)]
pub enum ProxmoxError {
    /// TLS refused a certificate nothing trusts; its SHA-256 fingerprint.
    UntrustedCert(String),
    /// A user login is needed and no password is saved.
    PasswordNeeded,
    /// The account has a second factor: a TOTP code (or recovery key) is needed.
    TfaNeeded,
    /// A console needs a user login, and this connection has only a token.
    ConsoleLoginNeeded,
    /// The user name, password, code or token was refused.
    Login(String),
    /// The server answered with an error status.
    Status { code: u16, message: String },
    /// No answer, or one that is not the Proxmox API.
    Other(String),
}

impl std::fmt::Display for ProxmoxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxmoxError::UntrustedCert(fp) => write!(f, "Untrusted certificate {fp}"),
            ProxmoxError::PasswordNeeded => f.write_str("A password is needed"),
            ProxmoxError::TfaNeeded => f.write_str("A second-factor code is needed"),
            ProxmoxError::ConsoleLoginNeeded => f.write_str("A console needs a user login, not an API token"),
            ProxmoxError::Login(m) if m.is_empty() => f.write_str("Login refused"),
            ProxmoxError::Login(m) => write!(f, "Login refused: {m}"),
            ProxmoxError::Status { code, message } if message.is_empty() => write!(f, "The server answered {code}"),
            ProxmoxError::Status { code, message } => write!(f, "{message} ({code})"),
            ProxmoxError::Other(m) => f.write_str(m),
        }
    }
}

/// A user login's ticket.
#[derive(Clone)]
pub struct Ticket {
    pub ticket: String,
    pub csrf: String,
    /// `user@realm`, as the server wrote it.
    pub user: String,
    at: Instant,
}

impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ticket").field("user", &self.user).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub enum Auth {
    Token { header: String },
    Ticket(Ticket),
}

/// What a user login needs.
#[derive(Clone, Default)]
pub struct UserLogin {
    /// `user@realm`.
    pub user: String,
    pub password: String,
    /// A TOTP code, or a recovery key (anything not six digits).
    pub otp: Option<String>,
}

/// The user name with its realm: `alice` and `pam` make `alice@pam`; a name
/// that already names one is kept.
pub fn full_user(username: &str, realm: &str) -> String {
    let u = username.trim();
    let r = realm.trim();
    if u.contains('@') || r.is_empty() { u.to_string() } else { format!("{u}@{r}") }
}

/// The second factor as the server takes it: `totp:123456`, or
/// `recovery:<key>` for anything that is not six digits.
pub fn tfa_answer(code: &str) -> String {
    let c: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if c.len() == 6 && c.chars().all(|c| c.is_ascii_digit()) { format!("totp:{c}") } else { format!("recovery:{c}") }
}

/// An API path, checked: under `/api2/json`, no way out of it.
pub fn valid_path(path: &str) -> bool {
    path.starts_with("/api2/json/")
        && !path.contains("..")
        && !path.contains('\\')
        && !path.contains("://")
        && !path.chars().any(|c| c.is_control() || c == ' ' || c == '?' || c == '#')
}

/// A host for a URL: an IPv6 address in brackets.
pub fn url_host(host: &str) -> String {
    crate::bmc::client::url_host(host)
}

/// The message an error answer carries: Proxmox puts it in the status
/// line's reason, and parameter errors in the body's `errors`.
pub fn error_message(reason: Option<&str>, body: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
        parts.push(r.to_string());
    }
    if let Some(m) = body["message"].as_str().map(str::trim).filter(|m| !m.is_empty()) {
        if !parts.iter().any(|p| p == m) {
            parts.push(m.to_string());
        }
    }
    if let Some(errs) = body["errors"].as_object() {
        let mut list: Vec<String> = errs.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or_default().trim())).collect();
        list.sort();
        parts.extend(list);
    }
    parts.join(" · ")
}

pub struct Params {
    pub host: String,
    pub port: u16,
    pub product: Product,
    /// Fingerprints (`AB:CD:..`) of certificates trusted for this session.
    pub trusted: Vec<String>,
}

pub struct Client {
    http: reqwest::Client,
    /// `https://host:port`, no trailing slash.
    base: String,
    pub host: String,
    pub port: u16,
    pub product: Product,
    trusted: Vec<String>,
    auth: tokio::sync::Mutex<Option<Auth>>,
    /// The ticket the consoles use: the API login's when that is one, else a
    /// user login made for them (a token cannot open a console).
    console: tokio::sync::Mutex<Option<Ticket>>,
    refused: Arc<Mutex<Option<String>>>,
}

impl Client {
    pub fn new(p: &Params) -> Result<Self, ProxmoxError> {
        let (tls, refused) = crate::ftp::tls::pinning_config(&p.trusted).map_err(ProxmoxError::Other)?;
        let http = crate::http::client_builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| ProxmoxError::Other(format!("HTTPS setup failed: {e}")))?;
        let port = if p.port == 0 { if p.product == Product::Pbs { 8007 } else { 8006 } } else { p.port };
        Ok(Self {
            http,
            base: format!("https://{}:{port}", url_host(&p.host)),
            host: p.host.trim().trim_matches(['[', ']']).to_string(),
            port,
            product: p.product,
            trusted: p.trusted.clone(),
            auth: tokio::sync::Mutex::new(None),
            console: tokio::sync::Mutex::new(None),
            refused,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn trusted(&self) -> &[String] {
        &self.trusted
    }

    fn net_error(&self, e: reqwest::Error) -> ProxmoxError {
        if let Some(fp) = self.refused.lock().ok().and_then(|mut r| r.take()) {
            return ProxmoxError::UntrustedCert(fp);
        }
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg = format!("{msg}: {s}");
            src = s.source();
        }
        if e.is_timeout() {
            return ProxmoxError::Other(format!("The server did not answer in time ({msg})"));
        }
        ProxmoxError::Other(msg)
    }

    /// Log in with a user and password (and the second factor when the
    /// account has one). Returns the ticket; nothing is kept here.
    pub async fn ticket_login(&self, login: &UserLogin) -> Result<Ticket, ProxmoxError> {
        if login.password.is_empty() {
            return Err(ProxmoxError::PasswordNeeded);
        }
        let first = self
            .raw_ticket(&json!({ "username": login.user, "password": login.password, "new-format": 1 }))
            .await?;
        let needs_tfa = first["NeedTFA"].as_i64().unwrap_or(0) != 0 || first["NeedTFA"].as_bool().unwrap_or(false);
        let data = if needs_tfa {
            let Some(code) = login.otp.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
                return Err(ProxmoxError::TfaNeeded);
            };
            let challenge = first["ticket"].as_str().unwrap_or_default();
            self.raw_ticket(&json!({ "username": login.user, "password": tfa_answer(code), "tfa-challenge": challenge }))
                .await?
        } else {
            first
        };
        ticket_from(&data, &login.user)
    }

    async fn raw_ticket(&self, body: &Value) -> Result<Value, ProxmoxError> {
        let res = self
            .http
            .post(format!("{}/api2/json/access/ticket", self.base))
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| self.net_error(e))?;
        let status = res.status();
        let reason = reason_of(&res);
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ProxmoxError::Login(error_message(reason.as_deref(), &body).replace("authentication failure", "wrong user name, password or code")));
        }
        if !status.is_success() {
            return Err(ProxmoxError::Status { code: status.as_u16(), message: error_message(reason.as_deref(), &body) });
        }
        if body["data"].is_null() {
            return Err(ProxmoxError::Other("This address does not answer as a Proxmox server".into()));
        }
        Ok(body["data"].clone())
    }

    /// Use this login for the API from now on (and for the consoles, when it
    /// is a ticket).
    pub async fn set_auth(&self, auth: Auth) {
        if let Auth::Ticket(t) = &auth {
            *self.console.lock().await = Some(t.clone());
        }
        *self.auth.lock().await = Some(auth);
    }

    pub async fn uses_token(&self) -> bool {
        matches!(*self.auth.lock().await, Some(Auth::Token { .. }))
    }

    /// The console ticket, renewed when it is getting old; none yet: a
    /// user login is needed first (see [`Client::set_console`]).
    pub async fn console_ticket(&self) -> Option<Ticket> {
        let current = self.console.lock().await.clone()?;
        if current.at.elapsed() < RENEW_AFTER {
            return Some(current);
        }
        match self.renew(&current).await {
            Ok(t) => {
                *self.console.lock().await = Some(t.clone());
                let mut auth = self.auth.lock().await;
                if let Some(Auth::Ticket(a)) = auth.as_mut() {
                    if a.user == t.user {
                        *a = t.clone();
                    }
                }
                Some(t)
            }
            Err(e) => {
                tracing::info!("Proxmox console login could not be renewed: {e}");
                *self.console.lock().await = None;
                None
            }
        }
    }

    pub async fn set_console(&self, t: Ticket) {
        *self.console.lock().await = Some(t);
    }

    /// A ticket for a new one, before it runs out (no second factor again).
    async fn renew(&self, t: &Ticket) -> Result<Ticket, ProxmoxError> {
        let data = self.raw_ticket(&json!({ "username": t.user, "password": t.ticket })).await?;
        ticket_from(&data, &t.user)
    }

    /// The API login, a ticket renewed when it is getting old.
    async fn api_auth(&self) -> Result<Auth, ProxmoxError> {
        let auth = self.auth.lock().await.clone().ok_or_else(|| ProxmoxError::Other("Not logged in".into()))?;
        if let Auth::Ticket(t) = &auth {
            if t.at.elapsed() >= RENEW_AFTER {
                let fresh = self.renew(t).await?;
                *self.auth.lock().await = Some(Auth::Ticket(fresh.clone()));
                *self.console.lock().await = Some(fresh.clone());
                return Ok(Auth::Ticket(fresh));
            }
        }
        Ok(auth)
    }

    /// Send a request with `auth` (the API login by default).
    pub async fn send_as(&self, auth: Option<&Auth>, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<Value, ProxmoxError> {
        if !valid_path(path) {
            return Err(ProxmoxError::Other(format!("Not an API path: {path}")));
        }
        let auth = match auth {
            Some(a) => a.clone(),
            None => self.api_auth().await?,
        };
        let mut url = url::Url::parse(&format!("{}{path}", self.base)).map_err(|e| ProxmoxError::Other(e.to_string()))?;
        if !query.is_empty() {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
        }
        let mut req = self.http.request(method.clone(), url).header("Accept", "application/json");
        match &auth {
            Auth::Token { header } => req = req.header("Authorization", header.as_str()),
            Auth::Ticket(t) => {
                req = req.header("Cookie", format!("{}={}", self.product.cookie(), t.ticket));
                if method != Method::GET {
                    req = req.header("CSRFPreventionToken", t.csrf.as_str());
                }
            }
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req.send().await.map_err(|e| self.net_error(e))?;
        let status = res.status();
        let reason = reason_of(&res);
        let text = res.text().await.map_err(|e| self.net_error(e))?;
        let value: Value = if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::Null) };
        if status == StatusCode::UNAUTHORIZED {
            return Err(ProxmoxError::Login(error_message(reason.as_deref(), &value)));
        }
        if !status.is_success() {
            return Err(ProxmoxError::Status { code: status.as_u16(), message: error_message(reason.as_deref(), &value) });
        }
        Ok(value["data"].clone())
    }

    pub async fn get(&self, path: &str) -> Result<Value, ProxmoxError> {
        self.send_as(None, Method::GET, path, &[], None).await
    }

    pub async fn get_q(&self, path: &str, query: &[(&str, String)]) -> Result<Value, ProxmoxError> {
        self.send_as(None, Method::GET, path, query, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, ProxmoxError> {
        self.send_as(None, Method::POST, path, &[], Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, ProxmoxError> {
        self.send_as(None, Method::DELETE, path, &[], None).await
    }
}

fn reason_of(res: &reqwest::Response) -> Option<String> {
    // Only a reason of the server's own: a standard one ("Forbidden") says
    // nothing the status does not.
    res.extensions().get::<hyper::ext::ReasonPhrase>().map(|r| String::from_utf8_lossy(r.as_bytes()).into_owned())
}

fn ticket_from(data: &Value, user: &str) -> Result<Ticket, ProxmoxError> {
    let ticket = data["ticket"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| ProxmoxError::Login("No ticket in the answer".into()))?;
    let csrf = data["CSRFPreventionToken"].as_str().unwrap_or_default();
    Ok(Ticket {
        ticket: ticket.to_string(),
        csrf: csrf.to_string(),
        user: data["username"].as_str().filter(|u| !u.is_empty()).unwrap_or(user).to_string(),
        at: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_names_get_their_realm() {
        assert_eq!(full_user("alice", "pam"), "alice@pam");
        assert_eq!(full_user(" backup ", "pbs"), "backup@pbs");
        assert_eq!(full_user("admin@ldap", "pam"), "admin@ldap");
        assert_eq!(full_user("root", ""), "root");
    }

    #[test]
    fn second_factor_answers() {
        assert_eq!(tfa_answer("123 456"), "totp:123456");
        assert_eq!(tfa_answer("abcd-efgh-ijkl"), "recovery:abcd-efgh-ijkl");
        assert_eq!(tfa_answer("1234567"), "recovery:1234567");
    }

    #[test]
    fn token_headers() {
        assert_eq!(Product::Pve.token_header("u@pam!t", "s"), "PVEAPIToken=u@pam!t=s");
        assert_eq!(Product::Pbs.token_header("u@pbs!t", "s"), "PBSAPIToken=u@pbs!t:s");
        assert_eq!(Product::for_port(8007), Product::Pbs);
        assert_eq!(Product::for_port(8006), Product::Pve);
        assert_eq!(Product::for_port(443), Product::Pve);
    }

    #[test]
    fn paths_stay_in_the_api() {
        assert!(valid_path("/api2/json/cluster/resources"));
        assert!(valid_path("/api2/json/nodes/pve1/tasks/UPID:pve1:00001:0001:6500:qmstart:100:root@pam:/log"));
        assert!(!valid_path("/api2/json/../../etc"));
        assert!(!valid_path("/other"));
        assert!(!valid_path("/api2/json/x?y=1"));
        assert!(!valid_path("/api2/json/x http://evil"));
        assert!(!valid_path("/api2/json/x\nHost: evil"));
    }

    #[test]
    fn error_messages() {
        let body = json!({"data": null, "errors": {"vmid": "invalid format", "node": "missing"}});
        assert_eq!(error_message(Some("Parameter verification failed."), &body), "Parameter verification failed. · node: missing · vmid: invalid format");
        assert_eq!(error_message(Some("Permission check failed (/vms/100, VM.PowerMgmt)"), &json!({"data": null})), "Permission check failed (/vms/100, VM.PowerMgmt)");
        assert_eq!(error_message(None, &json!({"message": "x"})), "x");
        assert_eq!(error_message(None, &Value::Null), "");
    }

    #[test]
    fn tickets_from_answers() {
        let t = ticket_from(&json!({"ticket": "PVE:a@pam:1::x", "CSRFPreventionToken": "c", "username": "a@pam"}), "b@pam").unwrap();
        assert_eq!((t.ticket.as_str(), t.csrf.as_str(), t.user.as_str()), ("PVE:a@pam:1::x", "c", "a@pam"));
        assert!(ticket_from(&json!({}), "a").is_err());
        // The ticket never shows in a debug print.
        assert!(!format!("{t:?}").contains("PVE:"));
    }
}
