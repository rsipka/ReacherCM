//! FTP sessions: connecting one for the file manager, and the certificates
//! it trusts (see `crate::ftp`). Only `ftp.certs` of a record is changed
//! here; every other field is written back as stored.

use super::*;

/// What the window is told to ask when a connect needs the user.
const PASSWORD_NEEDED: &str = "FTP_PASSWORD_NEEDED";
const UNTRUSTED_CERT: &str = "FTP_UNTRUSTED_CERT:";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtpOpened {
    /// The folder the login starts in.
    pub home: String,
}

/// Connect a saved FTP session under the connection id `id`. The address,
/// login and certificates are the stored session's; `password`, typed in
/// for this connect, wins over the saved one. Fails with `FTP_PASSWORD_NEEDED`
/// when there is none, and `FTP_UNTRUSTED_CERT:<fingerprint>` for a
/// certificate neither the system nor the session trusts.
#[tauri::command]
#[tracing::instrument(skip(state, app, password))]
pub async fn ftp_connect(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    session_id: String,
    password: Option<String>,
) -> Result<FtpOpened, String> {
    let stored = stored_session(&state, &session_id).await?;
    if stored.kind != crate::state::SessionKind::Ftp {
        return Err("Not an FTP session".into());
    }
    let opts = stored.ftp.clone().unwrap_or_default();
    let anonymous = stored.username.trim().is_empty() || stored.username.eq_ignore_ascii_case("anonymous");
    let password = match password.filter(|p| !p.is_empty()) {
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
    if password.is_empty() && !anonymous {
        return Err(PASSWORD_NEEDED.into());
    }
    let params = crate::ftp::Params {
        host: stored.host.clone(),
        port: stored.port,
        username: stored.username.clone(),
        password,
        security: crate::ftp::Security::parse(opts.tls.as_deref()),
        trusted: opts.certs.clone(),
        wireguard: None,
        tunnel: None,
    };
    // Through the session's WireGuard tunnel (the webview turned it on),
    // kept on while the session is open.
    let mut params = params;
    // Off: the person chose to connect without it.
    if let Some(wg) = stored.wireguard_id.clone().filter(|w| !w.is_empty()) {
        if let Some(hold) = crate::wireguard::manager::hold(&wg).await {
            params.tunnel = Some(std::sync::Arc::new(hold));
            params.wireguard = Some(wg);
        }
    }
    let conn = crate::ftp::FtpConn::open(params).await.map_err(|e| match e {
        crate::ftp::FtpError::Tls { untrusted: Some(fp), .. } => format!("{UNTRUSTED_CERT}{fp}"),
        other => other.to_string(),
    })?;
    tracing::info!("FTP session {} connected", stored.name);
    let home = conn.home.clone();
    state.ftp_manager.insert(id, conn);
    Ok(FtpOpened { home })
}

/// Close an FTP session's connections, and stop its transfers.
#[tauri::command]
pub async fn ftp_disconnect(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    crate::ipc::sftp_commands::cancel_transfers_on(&state, &connection_id);
    if let Some(conn) = state.ftp_manager.remove(&connection_id) {
        conn.close().await;
    }
    Ok(())
}

/// Trust a certificate for an FTP session, by the fingerprint the failed
/// connect reported and the user agreed to. At most 16 are kept.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn ftp_session_cert_trust(
    state: State<'_, AppState>,
    session_id: String,
    fingerprint: String,
) -> Result<(), String> {
    trust_session_cert(&state, &session_id, &fingerprint, "ftp").await
}

/// Add a certificate fingerprint to `<kind>.certs` of a stored session of
/// that kind (`ftp`, `bmc`), keeping the newest 16.
pub(super) async fn trust_session_cert(state: &AppState, session_id: &str, fingerprint: &str, kind: &str) -> Result<(), String> {
    let valid = fingerprint.len() == 95 && fingerprint.split(':').all(|b| b.len() == 2 && b.bytes().all(|c| c.is_ascii_hexdigit()));
    if !valid {
        return Err("Not a SHA-256 fingerprint".into());
    }
    let fp = fingerprint.to_ascii_uppercase();
    edit_session_options(state, session_id, kind, |opts| {
        let mut certs: Vec<String> = opts.get("certs").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        certs.retain(|c| !c.eq_ignore_ascii_case(&fp));
        certs.push(fp);
        let extra = certs.len().saturating_sub(16);
        certs.drain(..extra);
        opts.insert("certs".into(), serde_json::to_value(&certs).map_err(|e| e.to_string())?);
        Ok(())
    })
    .await
}

/// Set one field of a session's own options (`kind`'s object), or with
/// `null` remove it, in place in the stored session.
pub(super) async fn set_session_option(state: &AppState, session_id: &str, kind: &str, field: &str, value: serde_json::Value) -> Result<(), String> {
    edit_session_options(state, session_id, kind, |opts| {
        if value.is_null() {
            opts.remove(field);
        } else {
            opts.insert(field.into(), value);
        }
        Ok(())
    })
    .await
}

/// Change a `kind` session's own options object in place (made when it
/// has none) and store the session again.
async fn edit_session_options(
    state: &AppState,
    session_id: &str,
    kind: &str,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), String>,
) -> Result<(), String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let loc = locate_session(&manager, session_id).await.ok_or_else(|| format!("Session not found: {session_id}"))?;
    let plaintext = manager.read_secret(&loc.vault_id, &loc.secret_id).await.map_err(|e| e.to_string())?;
    use secrecy::ExposeSecret;
    let mut value: serde_json::Value =
        serde_json::from_slice(plaintext.expose_secret()).map_err(|e| format!("Invalid session data: {e}"))?;
    let Some(obj) = value.as_object_mut() else { return Err(format!("Invalid session data: {session_id}")) };
    if obj.get("kind").and_then(|k| k.as_str()) != Some(kind) {
        return Err(format!("Not a {kind} session"));
    }
    let opts = obj.entry(kind).or_insert_with(|| serde_json::json!({}));
    if opts.is_null() {
        *opts = serde_json::json!({});
    }
    let Some(opts) = opts.as_object_mut() else { return Err(format!("Invalid session data: {session_id}")) };
    edit(opts)?;
    let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    manager
        .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())
}
