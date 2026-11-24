//! Web sessions: opening one, and the certificates it trusts (see
//! `crate::web`). Only `web.certs` of a record is changed here; every other
//! field is written back as stored.

use super::*;
use crate::web::WebCert;

/// Change the trusted certificates of one web session; `edit` returns
/// whether it changed anything, and only then is the record written.
async fn edit_certs(
    manager: &crate::vault::VaultManager,
    session_id: &str,
    edit: impl FnOnce(&mut Vec<WebCert>) -> bool,
) -> Result<Vec<WebCert>, String> {
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let loc = locate_session(manager, session_id).await.ok_or_else(|| format!("Session not found: {session_id}"))?;
    let plaintext = manager.read_secret(&loc.vault_id, &loc.secret_id).await.map_err(|e| e.to_string())?;
    use secrecy::ExposeSecret;
    let mut value: serde_json::Value =
        serde_json::from_slice(plaintext.expose_secret()).map_err(|e| format!("Invalid session data: {e}"))?;
    let Some(obj) = value.as_object_mut() else { return Err(format!("Invalid session data: {session_id}")) };
    if obj.get("kind").and_then(|k| k.as_str()) != Some("web") {
        return Err("Not a web session".into());
    }
    let web = obj.entry("web").or_insert_with(|| serde_json::json!({}));
    let Some(web) = web.as_object_mut() else { return Err(format!("Invalid session data: {session_id}")) };
    let mut certs: Vec<WebCert> = web.get("certs").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    if !edit(&mut certs) {
        return Ok(certs);
    }
    if certs.is_empty() {
        web.remove("certs");
    } else {
        web.insert("certs".into(), serde_json::to_value(&certs).map_err(|e| e.to_string())?);
    }
    let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    manager
        .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    Ok(certs)
}

/// Add a certificate, or refresh when it was seen; at most 16 are kept,
/// the oldest going first.
fn put(certs: &mut Vec<WebCert>, cert: WebCert) {
    certs.retain(|c| !c.sha256.eq_ignore_ascii_case(&cert.sha256));
    certs.push(cert);
    let extra = certs.len().saturating_sub(16);
    certs.drain(..extra);
}

/// Open a web session's page in its own window (desktop). Fetches the
/// password (from the vault, or the password manager) here, so it goes to
/// the page without passing through the app's own.
#[cfg(desktop)]
#[tauri::command]
#[tracing::instrument(skip(state, app))]
pub async fn web_session_open(state: State<'_, AppState>, app: tauri::AppHandle, session_id: String) -> Result<String, String> {
    let (stored, via) = stored_session_and_via(&state, &session_id).await?;
    if stored.kind != crate::state::SessionKind::Web {
        return Err("Not a web session".into());
    }
    let opts = stored.web.clone().unwrap_or_default();
    let mut password = None;
    if opts.login_mode() != "off" && !crate::web::window::is_open(&session_id) {
        if let AuthMethod::Password { password: own, password_ref } = &stored.auth_method {
            if password_ref.is_some() {
                require_trusted_refs(&stored, via.as_ref(), &["password".to_string()])?;
                let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
                let entry = desktop_ref(&stored)?;
                password = Some(
                    crate::pwmanager::with_prompt(&app, reason, crate::pwmanager::resolve_to_string(&state, entry))
                        .await
                        .inspect_err(|e| tracing::warn!("Web session {session_id}: {e}"))?,
                );
            } else {
                password = own.clone().filter(|p| !p.is_empty());
            }
        }
    }
    // Through the session's WireGuard tunnel, by a local SOCKS5 proxy. An
    // open window already has its proxy.
    let socks = if crate::web::window::is_open(&session_id) {
        None
    } else {
        crate::ipc::wireguard_commands::session_socks(&state, Some(&session_id), &stored.host, stored.port).await?
    };
    // Without Tauri's `macos-proxy` feature (macOS 14 and later) the proxy
    // would be ignored and the page loaded straight, outside the tunnel.
    if socks.is_some() && cfg!(target_os = "macos") {
        return Err("Web sessions through a WireGuard tunnel are not available on macOS yet.".into());
    }
    let socks = match socks {
        Some((port, tunnel)) => Some((port, crate::wireguard::manager::hold(&tunnel).await.ok_or("The WireGuard tunnel for this session is off. Open it again to turn it on.")?)),
        None => None,
    };
    crate::web::window::open(&app, &stored, password, socks)
}

/// The answer to `web-cert-ask`: trust the certificate (kept on the
/// session, and the window loads again) or not.
#[cfg(desktop)]
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn web_session_cert_answer(
    state: State<'_, AppState>,
    label: String,
    fingerprint: String,
    trust: bool,
) -> Result<(), String> {
    let Some((session_id, cert)) = crate::web::window::answer(&label, &fingerprint, trust)? else { return Ok(()) };
    // A server management session's console window: kept with the
    // certificates its Redfish connection trusts.
    if stored_session(&state, &session_id).await.is_ok_and(|s| s.kind == crate::state::SessionKind::Bmc) {
        return super::ftp::trust_session_cert(&state, &session_id, &cert.sha256, "bmc").await;
    }
    let manager = state.vault_manager.lock().await;
    edit_certs(&manager, &session_id, |certs| {
        put(certs, cert);
        true
    })
    .await
    .map(|_| ())
}

/// Trust a certificate by a fingerprint typed in (known before opening).
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn web_session_cert_add(
    state: State<'_, AppState>,
    session_id: String,
    fingerprint: String,
    host: Option<String>,
) -> Result<Vec<WebCert>, String> {
    let sha256 = crate::web::parse_fingerprint(&fingerprint)
        .ok_or_else(|| "Not a SHA-256 fingerprint: 64 hex digits, with or without colons.".to_string())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let manager = state.vault_manager.lock().await;
    edit_certs(&manager, &session_id, |certs| {
        put(certs, WebCert { sha256, host: host.unwrap_or_default().trim().to_string(), saved_at: Some(now) });
        true
    })
    .await
}

/// Stop trusting one certificate, or all (`fingerprint` unset). Takes
/// effect the next time the session's window opens.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn web_session_cert_remove(
    state: State<'_, AppState>,
    session_id: String,
    fingerprint: Option<String>,
) -> Result<Vec<WebCert>, String> {
    let manager = state.vault_manager.lock().await;
    edit_certs(&manager, &session_id, |certs| {
        let before = certs.len();
        certs.retain(|c| fingerprint.as_ref().is_some_and(|f| !c.sha256.eq_ignore_ascii_case(f)));
        certs.len() != before
    })
    .await
}

/// Forget a session's cookies and storage (logins it keeps, settings the
/// page saved). Not while its window is open.
#[cfg(desktop)]
#[tauri::command]
pub async fn web_session_clear_data(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    if crate::web::window::is_open(&session_id) {
        return Err("Close the session's window first".into());
    }
    #[cfg(target_os = "macos")]
    {
        crate::web::window::clear_store(&app, &session_id).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        let dir = crate::web::window::session_data_dir(&session_id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Could not remove {}: {e}", dir.display())),
        }
    }
}

#[cfg(test)]
mod webcert_tests {
    use super::*;

    fn cert(n: u8) -> WebCert {
        WebCert { sha256: format!("{n:02X}"), host: String::new(), saved_at: None }
    }

    #[test]
    fn put_replaces_and_keeps_sixteen() {
        let mut c = Vec::new();
        for n in 0..20 {
            put(&mut c, cert(n));
        }
        assert_eq!(c.len(), 16);
        assert_eq!(c[0].sha256, "04");
        put(&mut c, cert(10));
        assert_eq!(c.len(), 16);
        assert_eq!(c.last().unwrap().sha256, "0A");
    }
}
