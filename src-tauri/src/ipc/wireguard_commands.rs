//! WireGuard profiles: kept in the `__wireguard__` vault (so encrypted and
//! synced like the rest), and the tunnels made from them (see
//! `crate::wireguard`). The private key never goes back to the window
//! except through `wireguard_get`, for the profile editor.

use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;
use crate::vault::manager::WIREGUARD_VAULT;
use crate::vault::types::SecretCategory;
use crate::wireguard::config::{encode_key, public_key, WgConfig};
use crate::wireguard::manager::{self, TunnelState};

const CATEGORY: &str = "wireguard";

/// A saved profile: its name and the wg-quick file it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WgProfile {
    pub id: String,
    pub name: String,
    pub conf: String,
}

/// What the list shows: the profile without its keys, and its tunnel.
#[derive(Debug, Clone, Serialize)]
pub struct WgProfileInfo {
    pub id: String,
    pub name: String,
    pub endpoint: Option<String>,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub allowed_ips: Vec<String>,
    pub public_key: String,
    /// The file no longer parses (edited by an older or newer build).
    pub invalid: Option<String>,
    pub state: TunnelState,
}

async fn info(p: &WgProfile) -> WgProfileInfo {
    let state = manager::state(&p.id).await;
    match WgConfig::parse(&p.conf) {
        Ok(c) => WgProfileInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            endpoint: c.peer.endpoint.clone(),
            addresses: c.addresses.iter().map(ToString::to_string).collect(),
            dns: c.dns.iter().map(ToString::to_string).collect(),
            allowed_ips: c.peer.allowed_ips.iter().map(ToString::to_string).collect(),
            public_key: encode_key(&public_key(&c.private_key)),
            invalid: None,
            state,
        },
        Err(e) => WgProfileInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            endpoint: None,
            addresses: Vec::new(),
            dns: Vec::new(),
            allowed_ips: Vec::new(),
            public_key: String::new(),
            invalid: Some(e),
            state,
        },
    }
}

async fn ensure_vault(manager: &mut crate::vault::VaultManager) -> Result<String, String> {
    if let Some(id) = manager.get_vault_id_by_name(WIREGUARD_VAULT) {
        let _ = manager.open_vault(&id, None, None).await;
        manager.unlock_vault(&id).await.map_err(|e| e.to_string())?;
        return Ok(id);
    }
    Ok(manager.create_vault(WIREGUARD_VAULT, crate::vault::types::VaultType::Private, None, None).await.map_err(|e| e.to_string())?.id)
}

async fn all_profiles(state: &AppState) -> Result<Vec<WgProfile>, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Ok(Vec::new());
    }
    let Some(vault_id) = manager.get_vault_id_by_name(WIREGUARD_VAULT) else { return Ok(Vec::new()) };
    let secrets = manager.read_secrets_in(&vault_id, &["custom:wireguard"]).await.map_err(|e| e.to_string())?;
    let mut out: Vec<WgProfile> = secrets
        .into_iter()
        .filter_map(|(_, plain)| plain.ok())
        .filter_map(|plain| serde_json::from_slice::<WgProfile>(plain.expose_secret()).ok())
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

pub(crate) async fn profile(state: &AppState, id: &str) -> Result<WgProfile, String> {
    all_profiles(state).await?.into_iter().find(|p| p.id == id).ok_or_else(|| "This WireGuard profile no longer exists".to_string())
}

#[tauri::command]
pub async fn wireguard_list(state: State<'_, AppState>) -> Result<Vec<WgProfileInfo>, String> {
    let mut out = Vec::new();
    for p in all_profiles(&state).await? {
        out.push(info(&p).await);
    }
    Ok(out)
}

/// The whole profile, private key included, for the editor.
#[tauri::command]
pub async fn wireguard_get(state: State<'_, AppState>, id: String) -> Result<WgProfile, String> {
    profile(&state, &id).await
}

/// Checks a wg-quick file without saving it; the error says what is wrong.
#[tauri::command]
pub fn wireguard_check(conf: String) -> Result<(), String> {
    WgConfig::parse(&conf).map(|_| ())
}

/// Saves a new profile (no `id`) or changes one. A running tunnel whose
/// file changed is turned off: it would keep the old keys.
#[tauri::command]
pub async fn wireguard_save(state: State<'_, AppState>, id: Option<String>, name: String, conf: String) -> Result<WgProfileInfo, String> {
    let parsed = WgConfig::parse(&conf)?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("The profile needs a name".into());
    }
    let existing = match &id {
        Some(id) => Some(profile(&state, id).await?),
        None => None,
    };
    let p = WgProfile { id: id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()), name: name.clone(), conf };
    let json = serde_json::to_vec(&p).map_err(|e| e.to_string())?;
    {
        let mut manager = state.vault_manager.lock().await;
        if manager.is_locked() {
            return Err("Vault is locked".into());
        }
        let vault_id = ensure_vault(&mut manager).await?;
        if existing.is_some() {
            manager.update_secret(&vault_id, &p.id, SecretBox::new(Box::new(json))).await.map_err(|e| e.to_string())?;
        } else {
            manager
                .create_secret_with_id(&vault_id, &p.id, &name, SecretCategory::Custom(CATEGORY.into()), SecretBox::new(Box::new(json)))
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    if existing.is_some_and(|old| WgConfig::parse(&old.conf).ok().as_ref() != Some(&parsed)) {
        manager::down(&p.id).await;
    }
    Ok(info(&p).await)
}

#[tauri::command]
pub async fn wireguard_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    manager::down(&id).await;
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".into());
    }
    let Some(vault_id) = manager.get_vault_id_by_name(WIREGUARD_VAULT) else { return Ok(()) };
    manager.delete_secret(&vault_id, &id).await.map_err(|e| e.to_string())
}

/// Brings the tunnel up (if it is not already) and waits for the server.
#[tauri::command]
pub async fn wireguard_up(state: State<'_, AppState>, id: String) -> Result<WgProfileInfo, String> {
    let p = profile(&state, &id).await?;
    let config = WgConfig::parse(&p.conf)?;
    manager::up(&id, &config).await.map_err(|e| format!("WireGuard '{}': {e}", p.name))?;
    Ok(info(&p).await)
}

#[tauri::command]
pub async fn wireguard_down(id: String) -> Result<(), String> {
    manager::down(&id).await;
    Ok(())
}

/// Ids of the tunnels that are on.
#[tauri::command]
pub async fn wireguard_running() -> Vec<String> {
    manager::running_ids().await
}

/// A new key pair, for a profile made by hand: the private key for the
/// file, the public one to give the server.
#[tauri::command]
pub fn wireguard_generate_key() -> (String, String) {
    let mut private: [u8; 32] = rand::random();
    // Clamped as `wg genkey` does; x25519 would clamp it anyway.
    private[0] &= 248;
    private[31] = (private[31] & 127) | 64;
    (encode_key(&private), encode_key(&public_key(&private)))
}

/// The saved session's WireGuard tunnel for `host:port`, if it routes that
/// address through one. None for a connection with no saved session.
async fn session_tunnel(state: &AppState, session_id: Option<&str>, host: &str, port: u16) -> Option<String> {
    let id = session_id.filter(|s| !s.is_empty())?;
    let stored = crate::ipc::session_commands::stored_session(state, id).await.ok()?;
    crate::ipc::ssh_commands::wireguard_route(&stored, host, port).map(str::to_owned)
}

/// For a client that opens its own TCP connection (RDP, VNC, SPICE,
/// WinBox): a port on 127.0.0.1 that reaches `host:port` through the saved
/// session's WireGuard tunnel, or None when the session is not routed
/// through one. The tunnel must be on (the webview asks first).
pub(crate) async fn session_forward(state: &AppState, session_id: Option<&str>, host: &str, port: u16) -> Result<Option<u16>, String> {
    session_forward_to(state, session_id, host, port, port).await
}

/// [`session_forward`] to another port of the same host than the saved one
/// (SPICE's TLS port).
pub(crate) async fn session_forward_to(state: &AppState, session_id: Option<&str>, host: &str, port: u16, to_port: u16) -> Result<Option<u16>, String> {
    let Some(wg) = session_tunnel(state, session_id, host, port).await else { return Ok(None) };
    crate::wireguard::forward(&wg, host, to_port).await.map(Some).map_err(|e| e.to_string())
}

/// For a web session: the port of a SOCKS5 proxy on 127.0.0.1 into the
/// saved session's WireGuard tunnel, or None when it is not routed through one.
pub(crate) async fn session_socks(state: &AppState, session_id: Option<&str>, host: &str, port: u16) -> Result<Option<u16>, String> {
    let Some(wg) = session_tunnel(state, session_id, host, port).await else { return Ok(None) };
    crate::wireguard::socks(&wg).await.map(Some).map_err(|e| e.to_string())
}

/// The WireGuard profile the saved session routes `host:port` through (its
/// main address when not given), so the webview can turn it on first.
#[tauri::command]
pub async fn wireguard_session_route(
    state: State<'_, AppState>,
    session_id: String,
    host: Option<String>,
    port: Option<u16>,
) -> Result<Option<String>, String> {
    let Ok(stored) = crate::ipc::session_commands::stored_session(&state, &session_id).await else { return Ok(None) };
    let host = host.unwrap_or_else(|| stored.host.clone());
    let port = port.unwrap_or(stored.port);
    Ok(crate::ipc::ssh_commands::wireguard_route(&stored, &host, port).map(str::to_owned))
}
