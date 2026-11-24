//! Proxmox sessions (see `crate::proxmox`): connecting a saved one with
//! its API token or user login, the console login, the certificates it
//! trusts, and what its pages show and do. Every page command takes the
//! connection id the tab connected under.

use super::*;
use crate::proxmox::{client::full_user, model, Auth, Client, Opened, Product, ProxmoxConn, ProxmoxError, UserLogin};

/// What the window is told to ask when a connect needs the user.
const PASSWORD_NEEDED: &str = "PVE_PASSWORD_NEEDED";
const TFA_NEEDED: &str = "PVE_TFA_NEEDED";
const CONSOLE_LOGIN: &str = "PVE_CONSOLE_LOGIN";
const UNTRUSTED_CERT: &str = "PVE_UNTRUSTED_CERT:";
const LOGIN_REFUSED: &str = "PVE_LOGIN_REFUSED:";

pub(crate) fn err(e: ProxmoxError) -> String {
    match e {
        ProxmoxError::UntrustedCert(fp) => format!("{UNTRUSTED_CERT}{fp}"),
        ProxmoxError::PasswordNeeded => PASSWORD_NEEDED.into(),
        ProxmoxError::TfaNeeded => TFA_NEEDED.into(),
        ProxmoxError::ConsoleLoginNeeded => CONSOLE_LOGIN.into(),
        ProxmoxError::Login(m) => format!("{LOGIN_REFUSED}{m}"),
        other => other.to_string(),
    }
}

fn conn(state: &AppState, id: &str) -> Result<std::sync::Arc<ProxmoxConn>, String> {
    state.proxmox_manager.get(id).ok_or_else(|| "The Proxmox session is closed; reconnect it".to_string())
}

async fn stored_proxmox(state: &AppState, session_id: &str) -> Result<crate::state::SessionConfig, String> {
    let stored = stored_session(state, session_id).await?;
    if stored.kind != crate::state::SessionKind::Proxmox {
        return Err("Not a Proxmox session".into());
    }
    Ok(stored)
}

/// The user login's password: typed in now, else the saved one (from a
/// password manager when it is kept there). Empty: none.
async fn password_for(state: &AppState, app: &tauri::AppHandle, stored: &crate::state::SessionConfig, typed: Option<String>) -> Result<String, String> {
    if let Some(p) = typed.filter(|p| !p.is_empty()) {
        return Ok(p);
    }
    Ok(match &stored.auth_method {
        AuthMethod::Password { password_ref: Some(_), .. } => {
            require_trusted_refs(stored, None, &["password".to_string()])?;
            let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
            let entry = desktop_ref(stored)?;
            crate::pwmanager::with_prompt(app, reason, crate::pwmanager::resolve_to_string(state, entry)).await?
        }
        AuthMethod::Password { password: own, .. } => own.clone().unwrap_or_default(),
        _ => String::new(),
    })
}

fn client_for(stored: &crate::state::SessionConfig) -> Result<Client, ProxmoxError> {
    let opts = stored.proxmox.clone().unwrap_or_default();
    Client::new(&crate::proxmox::Params {
        host: stored.host.clone(),
        port: stored.port,
        product: Product::for_port(stored.port),
        trusted: opts.certs,
    })
}

/// Connect a saved Proxmox session under the connection id `id`: with its
/// API token when it has one, else its user login (`password` typed in now
/// wins over the saved one; `otp` is the second factor). Fails with
/// `PVE_PASSWORD_NEEDED`, `PVE_TFA_NEEDED`, `PVE_LOGIN_REFUSED:<message>` or
/// `PVE_UNTRUSTED_CERT:<fingerprint>` when the user has to answer.
#[tauri::command]
#[tracing::instrument(skip(state, app, password, otp))]
pub async fn proxmox_connect(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    session_id: String,
    password: Option<String>,
    otp: Option<String>,
) -> Result<Opened, String> {
    let stored = stored_proxmox(&state, &session_id).await?;
    let opts = stored.proxmox.clone().unwrap_or_default();
    let client = client_for(&stored).map_err(err)?;
    let (auth, user) = match opts.token() {
        Some((token_id, secret)) => {
            if !crate::proxmox::valid_token_id(token_id) {
                return Err("The API token id looks like user@realm!name".into());
            }
            (Auth::Token { header: client.product.token_header(token_id, secret) }, token_id.to_string())
        }
        None => {
            let password = password_for(&state, &app, &stored, password).await?;
            let user = full_user(&stored.username, opts.realm());
            let ticket = client.ticket_login(&UserLogin { user: user.clone(), password, otp }).await.map_err(err)?;
            let user = ticket.user.clone();
            (Auth::Ticket(ticket), user)
        }
    };
    let conn = ProxmoxConn::open(client, session_id, auth, user).await.map_err(err)?;
    tracing::info!("Proxmox session {} connected ({:?} {})", stored.name, conn.client.product, conn.version.as_deref().unwrap_or("?"));
    let opened = conn.opened().await;
    state.proxmox_manager.remove(&id);
    state.proxmox_manager.insert(id, conn);
    Ok(opened)
}

/// Whether the consoles can open now: the connection has a user login.
#[tauri::command]
pub async fn proxmox_console_ready(state: State<'_, AppState>, connection_id: String) -> Result<bool, String> {
    Ok(conn(&state, &connection_id)?.client.console_ticket().await.is_some())
}

/// The user login the consoles need, for a connection made with an API
/// token: the session's user and realm, `password` typed in now or the
/// saved one, `otp` the second factor. Fails as `proxmox_connect` does.
#[tauri::command]
#[tracing::instrument(skip(state, app, password, otp))]
pub async fn proxmox_console_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    connection_id: String,
    password: Option<String>,
    otp: Option<String>,
) -> Result<(), String> {
    let c = conn(&state, &connection_id)?;
    let stored = stored_proxmox(&state, &c.session_id).await?;
    if stored.username.trim().is_empty() {
        return Err("The consoles need a user login: add the user name to the session".into());
    }
    let opts = stored.proxmox.clone().unwrap_or_default();
    let password = password_for(&state, &app, &stored, password).await?;
    let user = full_user(&stored.username, opts.realm());
    let ticket = c.client.ticket_login(&UserLogin { user, password, otp }).await.map_err(err)?;
    c.client.set_console(ticket).await;
    Ok(())
}

#[tauri::command]
pub async fn proxmox_disconnect(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    state.proxmox_manager.remove(&connection_id);
    Ok(())
}

/// Trust a certificate for a Proxmox session, by the fingerprint the failed
/// connect reported and the user agreed to.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn proxmox_session_cert_trust(state: State<'_, AppState>, session_id: String, fingerprint: String) -> Result<(), String> {
    super::ftp::trust_session_cert(&state, &session_id, &fingerprint, "proxmox").await
}

#[tauri::command]
pub async fn proxmox_overview(state: State<'_, AppState>, connection_id: String) -> Result<model::Overview, String> {
    conn(&state, &connection_id)?.overview().await.map_err(err)
}

#[tauri::command]
pub async fn proxmox_resources(state: State<'_, AppState>, connection_id: String) -> Result<Vec<model::Resource>, String> {
    conn(&state, &connection_id)?.resources().await.map_err(err)
}

#[tauri::command]
pub async fn proxmox_guest(state: State<'_, AppState>, connection_id: String, node: String, kind: String, vmid: u32) -> Result<model::GuestDetail, String> {
    conn(&state, &connection_id)?.guest(&node, &kind, vmid).await.map_err(err)
}

/// A guest's power button (`start`, `shutdown`, `stop`, `reboot`, `reset`,
/// `suspend`, `resume`); the task it started.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn proxmox_guest_action(state: State<'_, AppState>, connection_id: String, node: String, kind: String, vmid: u32, action: String) -> Result<String, String> {
    conn(&state, &connection_id)?.guest_action(&node, &kind, vmid, &action).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state, description))]
#[allow(clippy::too_many_arguments)]
pub async fn proxmox_snapshot_create(
    state: State<'_, AppState>,
    connection_id: String,
    node: String,
    kind: String,
    vmid: u32,
    name: String,
    description: String,
    vmstate: bool,
) -> Result<String, String> {
    conn(&state, &connection_id)?.snapshot_create(&node, &kind, vmid, &name, &description, vmstate).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn proxmox_snapshot_rollback(state: State<'_, AppState>, connection_id: String, node: String, kind: String, vmid: u32, name: String) -> Result<String, String> {
    conn(&state, &connection_id)?.snapshot_rollback(&node, &kind, vmid, &name).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn proxmox_snapshot_delete(state: State<'_, AppState>, connection_id: String, node: String, kind: String, vmid: u32, name: String) -> Result<String, String> {
    conn(&state, &connection_id)?.snapshot_delete(&node, &kind, vmid, &name).await.map_err(err)
}

/// Restart (`reboot`) or shut down (`shutdown`) a node.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn proxmox_node_action(state: State<'_, AppState>, connection_id: String, node: String, command: String) -> Result<(), String> {
    conn(&state, &connection_id)?.node_action(&node, &command).await.map_err(err)
}

#[tauri::command]
pub async fn proxmox_tasks(state: State<'_, AppState>, connection_id: String) -> Result<Vec<model::Task>, String> {
    conn(&state, &connection_id)?.tasks().await.map_err(err)
}

#[tauri::command]
pub async fn proxmox_task_log(state: State<'_, AppState>, connection_id: String, node: String, upid: String, start: u64) -> Result<model::TaskLog, String> {
    conn(&state, &connection_id)?.task_log(&node, &upid, start).await.map_err(err)
}

#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn proxmox_task_stop(state: State<'_, AppState>, connection_id: String, node: String, upid: String) -> Result<(), String> {
    conn(&state, &connection_id)?.task_stop(&node, &upid).await.map_err(err)
}

/// The open connection a console tab of `session_id` goes through.
pub(crate) fn console_conn(state: &AppState, session_id: &str) -> Result<std::sync::Arc<ProxmoxConn>, String> {
    state
        .proxmox_manager
        .for_session(session_id)
        .ok_or_else(|| "Open the Proxmox session's tab first; its consoles go through it".to_string())
}
