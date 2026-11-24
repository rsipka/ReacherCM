//! Server management sessions (see `crate::bmc`): connecting a saved one,
//! the certificates it trusts, and what its pages show and do. Every page
//! command takes the connection id the tab connected under.

use super::*;
use crate::bmc::{model, BmcConn, BmcError, Opened};

/// What the window is told to ask when a connect needs the user.
const PASSWORD_NEEDED: &str = "BMC_PASSWORD_NEEDED";
const UNTRUSTED_CERT: &str = "BMC_UNTRUSTED_CERT:";
const LOGIN_REFUSED: &str = "BMC_LOGIN_REFUSED:";

fn err(e: BmcError) -> String {
    match e {
        BmcError::UntrustedCert(fp) => format!("{UNTRUSTED_CERT}{fp}"),
        BmcError::Login(m) => format!("{LOGIN_REFUSED}{m}"),
        other => other.to_string(),
    }
}

fn conn(state: &AppState, id: &str) -> Result<std::sync::Arc<BmcConn>, String> {
    state.bmc_manager.get(id).ok_or_else(|| "The server management session is closed; reconnect it".to_string())
}

/// Connect a saved server management session under the connection id `id`.
/// The address, login and certificates are the stored session's; `password`,
/// typed in for this connect, wins over the saved one. Fails with
/// `BMC_PASSWORD_NEEDED`, `BMC_LOGIN_REFUSED:<message>` or
/// `BMC_UNTRUSTED_CERT:<fingerprint>` when the user has to answer.
#[tauri::command]
#[tracing::instrument(skip(state, app, password))]
pub async fn bmc_connect(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    session_id: String,
    password: Option<String>,
) -> Result<Opened, String> {
    let stored = stored_session(&state, &session_id).await?;
    if stored.kind != crate::state::SessionKind::Bmc {
        return Err("Not a server management session".into());
    }
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
    if password.is_empty() {
        return Err(PASSWORD_NEEDED.into());
    }
    let params = crate::bmc::Params {
        host: stored.host.clone(),
        port: stored.port,
        username: stored.username.clone(),
        password,
        trusted: stored.bmc.as_ref().map(|b| b.certs.clone()).unwrap_or_default(),
    };
    let conn = BmcConn::open(params).await.map_err(err)?;
    tracing::info!("Server management session {} connected ({})", stored.name, conn.vendor);
    let opened = conn.opened();
    if let Some(old) = state.bmc_manager.remove(&id) {
        old.client.logout().await;
    }
    state.bmc_manager.insert(id, conn);
    Ok(opened)
}

/// Log out of the BMC, which frees its login slot.
#[tauri::command]
pub async fn bmc_disconnect(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    if let Some(conn) = state.bmc_manager.remove(&connection_id) {
        conn.client.logout().await;
    }
    Ok(())
}

/// Trust a certificate for a server management session, by the fingerprint
/// the failed connect reported and the user agreed to.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_session_cert_trust(state: State<'_, AppState>, session_id: String, fingerprint: String) -> Result<(), String> {
    super::ftp::trust_session_cert(&state, &session_id, &fingerprint, "bmc").await
}

#[tauri::command]
pub async fn bmc_overview(state: State<'_, AppState>, connection_id: String) -> Result<model::Overview, String> {
    conn(&state, &connection_id)?.overview().await.map_err(err)
}

#[tauri::command]
pub async fn bmc_sensors(state: State<'_, AppState>, connection_id: String) -> Result<model::Sensors, String> {
    conn(&state, &connection_id)?.sensors().await.map_err(err)
}

#[tauri::command]
pub async fn bmc_hardware(state: State<'_, AppState>, connection_id: String) -> Result<model::Hardware, String> {
    conn(&state, &connection_id)?.hardware().await.map_err(err)
}

#[tauri::command]
pub async fn bmc_log_services(state: State<'_, AppState>, connection_id: String) -> Result<Vec<model::LogService>, String> {
    conn(&state, &connection_id)?.log_services().await.map_err(err)
}

#[tauri::command]
pub async fn bmc_log_entries(state: State<'_, AppState>, connection_id: String, path: String) -> Result<Vec<model::LogEntry>, String> {
    conn(&state, &connection_id)?.log_entries(&path).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_log_clear(state: State<'_, AppState>, connection_id: String, path: String) -> Result<(), String> {
    conn(&state, &connection_id)?.log_clear(&path).await.map_err(err)
}

#[tauri::command]
pub async fn bmc_jobs(state: State<'_, AppState>, connection_id: String) -> Result<Vec<model::Job>, String> {
    conn(&state, &connection_id)?.jobs().await.map_err(err)
}

#[tauri::command]
pub async fn bmc_virtual_media(state: State<'_, AppState>, connection_id: String) -> Result<Vec<model::VirtualMedia>, String> {
    conn(&state, &connection_id)?.virtual_media().await.map_err(err)
}

/// Press the power button (`On`, `GracefulShutdown`, `ForceOff`...).
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_power(state: State<'_, AppState>, connection_id: String, reset_type: String) -> Result<Option<String>, String> {
    conn(&state, &connection_id)?.power(&reset_type).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_boot_once(state: State<'_, AppState>, connection_id: String, target: String) -> Result<(), String> {
    conn(&state, &connection_id)?.boot_once(&target).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_identify(state: State<'_, AppState>, connection_id: String, on: bool) -> Result<(), String> {
    conn(&state, &connection_id)?.identify(on).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_media_insert(state: State<'_, AppState>, connection_id: String, path: String, image: String) -> Result<(), String> {
    conn(&state, &connection_id)?.media_insert(&path, &image).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_media_eject(state: State<'_, AppState>, connection_id: String, path: String) -> Result<(), String> {
    conn(&state, &connection_id)?.media_eject(&path).await.map_err(err)
}

/// Restart the BMC itself; the server keeps running.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn bmc_reset_bmc(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    let c = conn(&state, &connection_id)?;
    c.reset_bmc().await.map_err(err)?;
    // Its login is gone with it.
    state.bmc_manager.remove(&connection_id);
    Ok(())
}
