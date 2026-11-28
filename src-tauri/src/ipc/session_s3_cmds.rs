//! S3 sessions (see `crate::s3`): connecting a saved one for the file
//! manager, presigned links, and the certificates it trusts. The access key
//! is the session's user name, the secret key its password.

use super::*;

/// What the window is told to ask when a connect needs the user.
const SECRET_NEEDED: &str = "S3_SECRET_NEEDED";
const LOGIN_REFUSED: &str = "S3_LOGIN_REFUSED:";
const UNTRUSTED_CERT: &str = "S3_UNTRUSTED_CERT:";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct S3Opened {
    /// Where the file manager opens: `/`, or `/bucket`.
    pub home: String,
}

/// Connect a saved S3 session under the connection id `id`. `secret`,
/// typed in for this connect, wins over the saved one. Fails with
/// `S3_SECRET_NEEDED` when there is none, `S3_LOGIN_REFUSED:<why>` when the
/// store refused the keys, and `S3_UNTRUSTED_CERT:<fingerprint>` for a
/// certificate neither the system nor the session trusts.
#[tauri::command]
#[tracing::instrument(skip(state, app, secret))]
pub async fn s3_connect(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    session_id: String,
    secret: Option<String>,
) -> Result<S3Opened, String> {
    let stored = stored_session(&state, &session_id).await?;
    if stored.kind != crate::state::SessionKind::S3 {
        return Err("Not an S3 session".into());
    }
    let opts = stored.s3.clone().unwrap_or_default();
    let secret = match secret.filter(|p| !p.is_empty()) {
        Some(p) => p,
        None => match &stored.auth_method {
            AuthMethod::Password { password_ref: Some(_), .. } => {
                require_trusted_refs(&stored, None, &["password".to_string()])?;
                let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
                let entry = desktop_ref(&stored)?;
                crate::pwmanager::with_prompt(&app, reason, crate::pwmanager::resolve_to_string(&state, entry)).await?
            }
            AuthMethod::Password { password: own, .. } => own.clone().unwrap_or_default(),
            _ => String::new(),
        },
    };
    if secret.is_empty() {
        return Err(SECRET_NEEDED.into());
    }
    // Through the session's WireGuard tunnel when the window turned it on.
    let mut socks = None;
    if let Some(wg) = stored.wireguard_id.clone().filter(|w| !w.is_empty()) {
        if crate::wireguard::manager::is_up(&wg).await {
            socks = Some(crate::wireguard::socks(&wg).await.map_err(|e| e.to_string())?);
        }
    }
    let params = crate::s3::Params {
        host: stored.host.clone(),
        port: stored.port,
        http: opts.http,
        region: opts.region.clone(),
        path_style: opts.path_style,
        bucket: opts.bucket.clone(),
        access_key: stored.username.clone(),
        secret_key: secret,
        trusted: opts.certs.clone(),
        socks,
    };
    let fail = |e: crate::s3::S3Error| match e {
        crate::s3::S3Error::UntrustedCert(fp) => format!("{UNTRUSTED_CERT}{fp}"),
        crate::s3::S3Error::Login(m) => format!("{LOGIN_REFUSED}{m}"),
        other => other.to_string(),
    };
    let conn = crate::s3::S3Conn::new(&params).map_err(fail)?;
    conn.check().await.map_err(fail)?;
    tracing::info!("S3 session {} connected", stored.name);
    let home = conn.home();
    state.s3_manager.insert(id, conn);
    Ok(S3Opened { home })
}

/// Close an S3 session, and stop its transfers.
#[tauri::command]
pub async fn s3_disconnect(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    crate::ipc::sftp_commands::cancel_transfers_on(&state, &connection_id);
    state.s3_manager.remove(&connection_id);
    Ok(())
}

/// A link that downloads the object at `path` (`/bucket/key`) without
/// keys, for `expires` seconds (at most 7 days, S3's limit).
#[tauri::command]
pub async fn s3_presign(state: State<'_, AppState>, connection_id: String, path: String, expires: u32) -> Result<String, String> {
    let conn = state.s3_manager.get(&connection_id).ok_or("The S3 session is closed; reconnect it")?;
    let (bucket, key) = crate::s3::split(&path);
    let bucket = bucket.filter(|_| !key.is_empty()).ok_or("A link is for a file inside a bucket")?;
    Ok(conn.presign(&bucket, &key, expires.clamp(1, 7 * 24 * 3600)))
}

/// Trust a certificate for an S3 session, by the fingerprint the failed
/// connect reported and the user agreed to.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn s3_session_cert_trust(state: State<'_, AppState>, session_id: String, fingerprint: String) -> Result<(), String> {
    super::ftp::trust_session_cert(&state, &session_id, &fingerprint, "s3").await
}
