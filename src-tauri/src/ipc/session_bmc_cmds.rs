//! Server management sessions (see `crate::bmc`): connecting a saved one,
//! the certificates it trusts, and what its pages show and do. Every page
//! command takes the connection id the tab connected under.

use super::*;
use crate::bmc::{model, BmcConn, BmcError, Opened};

/// What the window is told to ask when a connect needs the user.
const PASSWORD_NEEDED: &str = "BMC_PASSWORD_NEEDED";
const UNTRUSTED_CERT: &str = "BMC_UNTRUSTED_CERT:";
const LOGIN_REFUSED: &str = "BMC_LOGIN_REFUSED:";
const LICENCE: &str = "BMC_LICENCE:";
const WG_OFF: &str = "The WireGuard tunnel for this session is off. Open the session again to turn it on.";

fn err(e: BmcError) -> String {
    match e {
        BmcError::UntrustedCert(fp) => format!("{UNTRUSTED_CERT}{fp}"),
        BmcError::Login(m) => format!("{LOGIN_REFUSED}{m}"),
        BmcError::Licence(m) => format!("{LICENCE}{m}"),
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
        socks: None,
        tunnel: None,
    };
    // Through the session's WireGuard tunnel (the webview turned it on), by
    // a local SOCKS5 proxy into it; kept on while the session is open.
    let mut params = params;
    if let Some((port, wg)) = crate::ipc::wireguard_commands::session_socks(&state, Some(&session_id), &stored.host, stored.port).await? {
        params.tunnel = Some(crate::wireguard::manager::hold(&wg).await.ok_or(WG_OFF)?);
        params.socks = Some(port);
    }
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

/// Save (or with `None` forget) how a server management session reaches
/// its controller's VNC server, from the VNC dialog.
#[tauri::command]
#[tracing::instrument(skip(state, vnc))]
pub async fn bmc_session_vnc_save(state: State<'_, AppState>, session_id: String, vnc: Option<crate::bmc::BmcVnc>) -> Result<(), String> {
    super::ftp::set_session_option(&state, &session_id, "bmc", "vnc", match vnc {
        Some(v) => serde_json::to_value(v).map_err(|e| e.to_string())?,
        None => serde_json::Value::Null,
    })
    .await
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

/// Open the controller's remote console in a web window (desktop): the
/// iDRAC's HTML5 console with a one-time login, or the web interface of the
/// others, signed in with this connection's login. The window is a web
/// session's (`crate::web`): no app commands, its own storage, and only the
/// certificates this session trusts.
#[cfg(desktop)]
#[tauri::command]
#[tracing::instrument(skip(state, app))]
pub async fn bmc_remote_console(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    connection_id: String,
    session_id: String,
) -> Result<String, String> {
    let c = conn(&state, &connection_id)?;
    let stored = stored_session(&state, &session_id).await?;
    if stored.kind != crate::state::SessionKind::Bmc {
        return Err("Not a server management session".into());
    }
    let web = console_window_session(&stored, c.client.base(), c.console().await.map_err(err)?)?;
    let password = (web.web.as_ref().and_then(|w| w.login.as_deref()) != Some("off")).then(|| c.client.password().to_string());
    // The console window goes through the session's WireGuard tunnel too.
    let socks = match crate::ipc::wireguard_commands::session_socks(&state, Some(&session_id), &stored.host, stored.port).await? {
        Some(_) if cfg!(target_os = "macos") => return Err("The remote console through a WireGuard tunnel is not available on macOS yet.".into()),
        Some((port, wg)) => Some((port, crate::wireguard::manager::hold(&wg).await.ok_or(WG_OFF)?)),
        None => None,
    };
    crate::web::window::open(&app, &web, password, socks)
}

/// The session as the web session its console window opens: the
/// controller's own address, the console's path, the certificates this
/// session trusts. Refused when the connection is not to this session's
/// controller, so its login never goes to another one.
fn console_window_session(
    stored: &crate::state::SessionConfig,
    connected_to: &str,
    launch: crate::bmc::ConsoleLaunch,
) -> Result<crate::state::SessionConfig, String> {
    let port = if stored.port == 0 { 443 } else { stored.port };
    let own = format!("https://{}:{port}", crate::bmc::client::url_host(&stored.host));
    if !own.eq_ignore_ascii_case(connected_to) {
        return Err("This tab is connected to another controller; reconnect it".into());
    }
    let mut s = stored.clone();
    s.kind = crate::state::SessionKind::Web;
    s.port = port;
    s.web = Some(crate::web::WebOptions {
        path: Some(launch.path),
        login: launch.login.map(str::to_owned),
        certs: stored
            .bmc
            .as_ref()
            .map(|b| b.certs.iter().map(|f| crate::web::WebCert { sha256: f.clone(), host: stored.host.clone(), saved_at: None }).collect())
            .unwrap_or_default(),
        ..Default::default()
    });
    Ok(s)
}

#[cfg(test)]
mod console_tests {
    use super::*;

    #[test]
    fn the_console_window_goes_to_the_sessions_own_controller() {
        let s: crate::state::SessionConfig = serde_json::from_value(serde_json::json!({
            "id": "s1", "name": "bmc", "kind": "bmc", "host": "bmc.example.com", "port": 443, "username": "root",
            "auth_method": { "type": "Password" }, "folder_id": null, "tags": [],
            "bmc": { "certs": ["AB:CD"] },
        }))
        .unwrap();
        let launch = crate::bmc::ConsoleLaunch { path: "/console?x=1".into(), login: Some("off") };
        let w = console_window_session(&s, "https://bmc.example.com:443", launch.clone()).unwrap();
        assert_eq!(w.kind, crate::state::SessionKind::Web);
        let opts = w.web.unwrap();
        assert_eq!(opts.path.as_deref(), Some("/console?x=1"));
        assert_eq!(opts.login.as_deref(), Some("off"));
        assert_eq!(opts.certs[0].sha256, "AB:CD");
        assert_eq!(crate::web::session_url(&w.host, w.port, &opts).unwrap().as_str(), "https://bmc.example.com/console?x=1");
        assert!(console_window_session(&s, "https://other.example.com:443", launch).is_err());
    }
}
