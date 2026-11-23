//! The host keys a session trusts (see `ssh::session_hostkeys`): saved by
//! connecting, and removed or replaced from the session editor, or all at
//! once from Settings. Only the `host_keys` field of a record is changed;
//! every other field is written back as stored, including fields a newer
//! build may have added.

use super::*;
use crate::ssh::session_hostkeys::{self as hk, SavedHostKey};
use serde::Serialize;

const FIELD: &str = hk::KEYS_FIELD;

/// The keys stored on a session record, as JSON.
fn keys_of(obj: &serde_json::Map<String, serde_json::Value>) -> Vec<SavedHostKey> {
    obj.get(FIELD).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// Change the host keys of one session; `edit` returns whether it changed
/// anything, and only then is the record written. An empty list is no field;
/// a server whose key is removed gets a removal mark (see `hk::set_keys`).
async fn edit_keys(
    manager: &crate::vault::VaultManager,
    session_id: &str,
    edit: impl FnOnce(&mut Vec<SavedHostKey>) -> bool,
) -> Result<Vec<SavedHostKey>, String> {
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let loc = locate_session(manager, session_id).await.ok_or_else(|| format!("Session not found: {session_id}"))?;
    let plaintext = manager.read_secret(&loc.vault_id, &loc.secret_id).await.map_err(|e| e.to_string())?;
    use secrecy::ExposeSecret;
    let mut value: serde_json::Value =
        serde_json::from_slice(plaintext.expose_secret()).map_err(|e| format!("Invalid session data: {e}"))?;
    let Some(obj) = value.as_object_mut() else { return Err(format!("Invalid session data: {session_id}")) };
    let before = keys_of(obj);
    let mut keys = before.clone();
    if !edit(&mut keys) {
        return Ok(keys);
    }
    hk::set_keys(obj, &before, &keys);
    let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    manager
        .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    Ok(keys)
}

/// Keep a key accepted while connecting (Accept and save).
pub(crate) async fn store_host_key(state: &AppState, session_id: &str, entry: SavedHostKey) -> Result<(), String> {
    let manager = state.vault_manager.lock().await;
    edit_keys(&manager, session_id, |keys| {
        if keys.contains(&entry) {
            return false;
        }
        hk::put(keys, entry);
        true
    })
    .await
    .map(|_| ())
}

/// The trust a connect for `session_id` starts with: the keys stored on the
/// session. `None` without a session, or when it cannot be read (then this
/// device's own list is used, as for Quick Connect).
pub(crate) async fn session_trust(
    state: &AppState,
    session_id: Option<&str>,
) -> Option<std::sync::Arc<hk::SessionTrust>> {
    let id = session_id.filter(|s| !s.is_empty())?;
    let manager = state.vault_manager.lock().await;
    let loc = locate_session(&manager, id).await?;
    let s = read_session(&manager, &loc.vault_id, &loc.secret_id).await.ok()?;
    let addresses = s.address_ids();
    Some(hk::SessionTrust::with_addresses(id, s.host_keys.unwrap_or_default(), addresses))
}

/// Remove a session's key for one server (`host`, as `host:port`), or all
/// of its keys. The server is dropped from this device's own list too, so
/// the key is not copied back onto the session on the next connect: the
/// next connect asks again. Returns the keys left.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_host_key_remove(
    state: State<'_, AppState>,
    session_id: String,
    host: Option<String>,
) -> Result<Vec<SavedHostKey>, String> {
    let manager = state.vault_manager.lock().await;
    let mut gone = Vec::new();
    let left = edit_keys(&manager, &session_id, |keys| {
        let before = keys.len();
        keys.retain(|k| {
            let drop = host.as_ref().is_none_or(|h| &k.host == h);
            if drop {
                gone.push(k.host.clone());
            }
            !drop
        });
        keys.len() != before
    })
    .await?;
    if let Some(h) = &host {
        if !gone.contains(h) {
            gone.push(h.clone());
        }
    }
    if let Err(e) = crate::ssh::client::forget_hosts(Some(&gone)) {
        tracing::warn!("Could not update this device's list of host keys: {e}");
    }
    tracing::info!("Removed {} host keys from a session", gone.len());
    Ok(left)
}

/// Replace a session's key for one server with a fingerprint typed in (as
/// `ssh-keygen -lf` prints it), for a server whose new key is known before
/// connecting to it. Returns the keys now kept.
#[tauri::command]
#[tracing::instrument(skip(state, fingerprint))]
pub async fn session_host_key_replace(
    state: State<'_, AppState>,
    session_id: String,
    host: String,
    fingerprint: String,
) -> Result<Vec<SavedHostKey>, String> {
    let fingerprint = hk::parse_typed(&fingerprint)
        .ok_or_else(|| "Not a SHA256 fingerprint. Copy it as ssh-keygen -lf prints it: SHA256: and 43 characters.".to_string())?;
    if host.trim().is_empty() {
        return Err("No server given".to_string());
    }
    let manager = state.vault_manager.lock().await;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    edit_keys(&manager, &session_id, |keys| {
        hk::put(keys, SavedHostKey { host: host.trim().to_string(), key_type: String::new(), fingerprint, saved_at: Some(now) });
        true
    })
    .await
}

/// What Remove all saved host keys did.
#[derive(Debug, Serialize, Default, PartialEq)]
pub struct HostKeysCleared {
    /// Sessions whose keys were removed.
    pub sessions: u32,
    /// Sessions whose keys could not be removed (a vault that refuses the
    /// write).
    pub failed: u32,
    /// Servers dropped from this device's own list.
    pub device: u32,
}

async fn clear_all(manager: &crate::vault::VaultManager) -> HostKeysCleared {
    let mut out = HostKeysCleared::default();
    let mut vaults: Vec<String> = get_sessions_vault_id_if_exists(manager).into_iter().collect();
    for v in manager.list_vaults().await.unwrap_or_default() {
        if !v.name.starts_with("__") {
            vaults.push(v.id);
        }
    }
    use secrecy::ExposeSecret;
    for vault_id in vaults {
        let Ok(secrets) = manager.read_secrets_in(&vault_id, &["session", "custom:session"]).await else { continue };
        for (meta, plaintext) in secrets {
            let Ok(plaintext) = plaintext else { continue };
            let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(plaintext.expose_secret()) else { continue };
            let Some(obj) = value.as_object_mut() else { continue };
            let before = keys_of(obj);
            if before.is_empty() {
                continue;
            }
            hk::set_keys(obj, &before, &[]);
            let written = match serde_json::to_vec(&value) {
                Ok(json) => manager.update_secret(&vault_id, &meta.id, SecretBox::new(Box::new(json))).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            match written {
                Ok(()) => out.sessions += 1,
                Err(e) => {
                    tracing::warn!("Could not remove the host keys of session {}: {e}", meta.id);
                    out.failed += 1;
                }
            }
        }
    }
    out
}

/// Settings → Security → Remove all saved host keys: from every session
/// (in shared vaults too, for everyone in them) and from this device's own
/// list. Every server is asked about again on its next connect.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn host_keys_remove_all(state: State<'_, AppState>) -> Result<HostKeysCleared, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let mut out = clear_all(&manager).await;
    out.device = crate::ssh::client::forget_hosts(None).map_err(|e| format!("This device's list of host keys: {e}"))? as u32;
    tracing::info!("Removed saved host keys: {} sessions, {} failed, {} on this device", out.sessions, out.failed, out.device);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_session(id: &str, keys: Option<serde_json::Value>) -> SecretBox<Vec<u8>> {
        let mut json = serde_json::json!({
            "id": id, "name": id, "host": "h", "port": 22, "username": "u",
            "auth_method": { "type": "Agent" }, "folder_id": null, "tags": [],
            "a_field_from_a_newer_build": 7
        });
        if let Some(k) = keys {
            json["host_keys"] = k;
        }
        SecretBox::new(Box::new(serde_json::to_vec(&json).unwrap()))
    }

    fn entry(host: &str, fp: &str) -> SavedHostKey {
        SavedHostKey { host: host.into(), key_type: "ssh-ed25519".into(), fingerprint: format!("SHA256:{fp}"), saved_at: Some(1) }
    }

    async fn raw(m: &crate::vault::VaultManager, vault: &str, id: &str) -> serde_json::Value {
        use secrecy::ExposeSecret;
        serde_json::from_slice(m.read_secret(vault, id).await.unwrap().expose_secret()).unwrap()
    }

    #[tokio::test]
    async fn keys_are_kept_replaced_and_removed_on_the_record() {
        let dir = std::env::temp_dir().join(format!("reach-session-hostkeys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = crate::vault::VaultManager::new(dir.clone());
        m.init_identity("session-hostkeys").await.unwrap();
        let private = ensure_sessions_vault(&mut m).await.unwrap();
        m.create_secret_with_id(&private, "a", "a", SecretCategory::Session, put_session("a", None)).await.unwrap();
        let team = m.create_vault("Team", crate::vault::types::VaultType::Private, None, None).await.unwrap().id;
        let other = serde_json::to_value(vec![entry("other.example.com:22", "x")]).unwrap();
        m.create_secret_with_id(&team, "b", "b", SecretCategory::Session, put_session("b", Some(other))).await.unwrap();

        let add = |e: SavedHostKey| move |keys: &mut Vec<SavedHostKey>| {
            hk::put(keys, e);
            true
        };
        edit_keys(&m, "a", add(entry("example.com:22", "one"))).await.unwrap();
        edit_keys(&m, "a", add(entry("jump.example.com:22", "hop"))).await.unwrap();
        let left = edit_keys(&m, "a", add(entry("example.com:22", "two"))).await.unwrap();
        assert_eq!(left.len(), 2, "one key per server");
        let s = read_session(&m, &private, "a").await.unwrap();
        assert_eq!(s.host_keys.unwrap()[0].fingerprint, "SHA256:two");
        assert_eq!(raw(&m, &private, "a").await["a_field_from_a_newer_build"], 7, "other fields kept");

        let left = edit_keys(&m, "a", |keys| {
            keys.retain(|k| k.host != "example.com:22");
            true
        })
        .await
        .unwrap();
        assert_eq!(left, vec![entry("jump.example.com:22", "hop")]);
        edit_keys(&m, "a", |keys| {
            keys.clear();
            true
        })
        .await
        .unwrap();
        assert!(raw(&m, &private, "a").await.get("host_keys").is_none(), "no keys is no field");

        edit_keys(&m, "a", add(entry("example.com:22", "one"))).await.unwrap();
        let out = clear_all(&m).await;
        assert_eq!(out, HostKeysCleared { sessions: 2, failed: 0, device: 0 }, "the private vault's and the team vault's");
        assert!(read_session(&m, &private, "a").await.unwrap().host_keys.is_none());
        assert!(read_session(&m, &team, "b").await.unwrap().host_keys.is_none());
        assert_eq!(raw(&m, &team, "b").await["a_field_from_a_newer_build"], 7);
        assert_eq!(clear_all(&m).await.sessions, 0, "nothing left writes nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
