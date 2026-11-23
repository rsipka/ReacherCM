//! Folders in the credentials list.
//!
//! A credential folder only keeps saved credentials in order: it has no
//! say in which login a session uses. It lives in the same vault as the
//! credentials it holds (this device's own in `__sessions__`, a shared
//! vault's in that vault, so it syncs with them), and holds only
//! credentials and folders of that vault.
//!
//! Folders are records of their own (`custom:credential_folder`), which an
//! older Reach never reads. A credential names its folder (`folder_id`); an
//! older Reach that saves the credential drops that, and the credential is
//! back at the top of the list. A credential whose folder is gone, or is in
//! another vault, shows at the top too.

use crate::ipc::session_commands as sessions;
use crate::state::{AppState, CredentialFolder};
use crate::vault::types::SecretCategory;
use secrecy::{ExposeSecret, SecretBox};
use std::collections::{HashMap, HashSet};
use tauri::State;

/// The category the folders are stored under.
const CATEGORY: &str = "credential_folder";

fn category() -> SecretCategory {
    SecretCategory::Custom(CATEGORY.to_string())
}

fn stored_category() -> String {
    format!("custom:{CATEGORY}")
}

/// Every credential folder: this device's own (vault None) and each user
/// vault's.
pub(crate) async fn read_folders(manager: &crate::vault::VaultManager) -> Vec<CredentialFolder> {
    let mut out = Vec::new();
    if let Some(vault) = sessions::sessions_vault_id(manager) {
        out.extend(folders_in(manager, &vault, None).await);
    }
    for vault in manager.list_vaults().await.unwrap_or_default() {
        if vault.name.starts_with("__") {
            continue;
        }
        for f in folders_in(manager, &vault.id, Some(vault.id.clone())).await {
            if !out.iter().any(|o: &CredentialFolder| o.id == f.id) {
                out.push(f);
            }
        }
    }
    out
}

async fn folders_in(manager: &crate::vault::VaultManager, vault_id: &str, as_vault: Option<String>) -> Vec<CredentialFolder> {
    let cat = stored_category();
    let Ok(secrets) = manager.read_secrets_in(vault_id, &[cat.as_str()]).await else { return Vec::new() };
    secrets
        .into_iter()
        .filter_map(|(_, plaintext)| plaintext.ok())
        .filter_map(|p| serde_json::from_slice::<CredentialFolder>(p.expose_secret()).ok())
        .map(|mut f| {
            // A vault has its own id on each device.
            f.vault_id = as_vault.clone();
            f
        })
        .collect()
}

/// The vault a folder's row is in.
async fn locate(manager: &crate::vault::VaultManager, id: &str) -> Option<String> {
    if let Some(vault) = sessions::sessions_vault_id(manager) {
        if manager.secret_exists(&vault, id).await {
            return Some(vault);
        }
    }
    sessions::user_vault_holding(manager, id).await
}

/// The folder a credential in `vault` may be filed in: `wanted` when it is
/// a credential folder of that same vault, else none (the top).
pub(crate) async fn folder_to_keep(
    manager: &crate::vault::VaultManager,
    wanted: Option<String>,
    vault: Option<&str>,
) -> Option<String> {
    let wanted = wanted.filter(|f| !f.is_empty())?;
    let folders = read_folders(manager).await;
    in_vault(&folders, &wanted, vault).then_some(wanted)
}

/// Whether `folder_id` is a folder of `vault`.
fn in_vault(folders: &[CredentialFolder], folder_id: &str, vault: Option<&str>) -> bool {
    folders.iter().any(|f| f.id == folder_id && f.vault_id.as_deref() == vault)
}

/// Whether `folder` may go under `parent` (None: the top): the parent is in
/// the same vault, and is not the folder itself or one inside it.
fn check_parent(folders: &[CredentialFolder], folder: &CredentialFolder, parent: Option<&str>) -> Result<(), String> {
    let Some(parent) = parent else { return Ok(()) };
    let by_id: HashMap<&str, &CredentialFolder> = folders.iter().map(|f| (f.id.as_str(), f)).collect();
    let target = by_id.get(parent).ok_or_else(|| format!("Folder not found: {parent}"))?;
    if target.vault_id != folder.vault_id {
        return Err("Folders in different vaults cannot be nested".to_string());
    }
    let mut current = Some(parent);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if id == folder.id {
            return Err("A folder cannot be moved into itself or a folder inside it".to_string());
        }
        if !seen.insert(id) {
            break;
        }
        current = by_id.get(id).and_then(|f| f.parent_id.as_deref());
    }
    Ok(())
}

fn folder_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Folder name cannot be empty".to_string());
    }
    Ok(name.to_string())
}

async fn write(manager: &crate::vault::VaultManager, folder: &CredentialFolder) -> Result<(), String> {
    let vault = locate(manager, &folder.id).await.ok_or_else(|| format!("Folder not found: {}", folder.id))?;
    let json = serde_json::to_vec(folder).map_err(|e| e.to_string())?;
    manager.update_secret(&vault, &folder.id, SecretBox::new(Box::new(json))).await.map_err(|e| e.to_string())
}

async fn find(manager: &crate::vault::VaultManager, folder_id: &str) -> Result<(Vec<CredentialFolder>, CredentialFolder), String> {
    let folders = read_folders(manager).await;
    let folder = folders
        .iter()
        .find(|f| f.id == folder_id)
        .cloned()
        .ok_or_else(|| format!("Folder not found: {folder_id}"))?;
    Ok((folders, folder))
}

/// All credential folders, each with its vault.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn credential_folder_list(state: State<'_, AppState>) -> Result<Vec<CredentialFolder>, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Ok(Vec::new());
    }
    Ok(read_folders(&manager).await)
}

/// A new folder in a vault (None, or one not a user vault: this device's
/// own), at the top or in `parent_id`, which must be in the same vault.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn credential_folder_create(
    state: State<'_, AppState>,
    name: String,
    parent_id: Option<String>,
    vault_id: Option<String>,
) -> Result<CredentialFolder, String> {
    let name = folder_name(&name)?;
    let mut manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked. Set a master password first.".to_string());
    }
    let vault_id = match vault_id {
        Some(v) if sessions::user_vault(&manager, &v).await? => Some(v),
        _ => None,
    };
    let storage = match vault_id.clone() {
        Some(v) => v,
        None => sessions::sessions_vault(&mut manager).await?,
    };
    let folder = CredentialFolder {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        parent_id: parent_id.filter(|p| !p.is_empty()),
        vault_id,
        icon: None,
        color: None,
    };
    check_parent(&read_folders(&manager).await, &folder, folder.parent_id.as_deref())?;
    let json = serde_json::to_vec(&folder).map_err(|e| e.to_string())?;
    manager
        .create_secret_with_id(&storage, &folder.id, &folder.name, category(), SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Created credential folder {}", folder.id);
    Ok(folder)
}

/// Rename a folder.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn credential_folder_rename(
    state: State<'_, AppState>,
    folder_id: String,
    name: String,
) -> Result<CredentialFolder, String> {
    let name = folder_name(&name)?;
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let (_, mut folder) = find(&manager, &folder_id).await?;
    folder.name = name;
    write(&manager, &folder).await?;
    Ok(folder)
}

/// Move a folder under another of its vault, or to the top (None).
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn credential_folder_move(
    state: State<'_, AppState>,
    folder_id: String,
    parent_id: Option<String>,
) -> Result<CredentialFolder, String> {
    let parent_id = parent_id.filter(|p| !p.is_empty());
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let (folders, mut folder) = find(&manager, &folder_id).await?;
    check_parent(&folders, &folder, parent_id.as_deref())?;
    folder.parent_id = parent_id;
    write(&manager, &folder).await?;
    Ok(folder)
}

/// Set or clear a folder's icon and colour, as for session folders.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn credential_folder_set_icon(
    state: State<'_, AppState>,
    folder_id: String,
    icon: Option<String>,
    color: Option<String>,
) -> Result<CredentialFolder, String> {
    let icon = sessions::folder_icon_value(icon, "icon")?;
    let color = sessions::folder_icon_value(color, "colour")?;
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let (_, mut folder) = find(&manager, &folder_id).await?;
    folder.icon = icon;
    folder.color = color;
    write(&manager, &folder).await?;
    Ok(folder)
}

/// Delete a folder. Nothing inside it is deleted: its credentials and
/// subfolders move up to where the folder was.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn credential_folder_delete(state: State<'_, AppState>, folder_id: String) -> Result<(), String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let Ok((folders, folder)) = find(&manager, &folder_id).await else {
        tracing::warn!("credential_folder_delete: {folder_id} is not in any open vault");
        return Ok(());
    };
    let Some(vault) = locate(&manager, &folder.id).await else { return Ok(()) };
    let parent = folder.parent_id.clone();
    // Subfolders first, then credentials: each record is edited as stored,
    // so fields a newer Reach added stay.
    for child in folders.iter().filter(|f| f.parent_id.as_deref() == Some(folder.id.as_str())) {
        let mut child = child.clone();
        child.parent_id = parent.clone();
        write(&manager, &child).await?;
    }
    let mut moved = 0u32;
    if let Ok(secrets) = manager.read_secrets_in(&vault, &["credential"]).await {
        for (meta, plaintext) in secrets {
            let Ok(plaintext) = plaintext else { continue };
            let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(plaintext.expose_secret()) else { continue };
            let Some(obj) = value.as_object_mut() else { continue };
            if obj.get("folder_id").and_then(|v| v.as_str()) != Some(folder.id.as_str()) {
                continue;
            }
            match &parent {
                Some(p) => obj.insert("folder_id".into(), serde_json::Value::String(p.clone())),
                None => obj.remove("folder_id"),
            };
            let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
            manager
                .update_secret(&vault, &meta.id, SecretBox::new(Box::new(json)))
                .await
                .map_err(|e| e.to_string())?;
            moved += 1;
        }
    }
    manager.delete_secret(&vault, &folder.id).await.map_err(|e| e.to_string())?;
    tracing::info!("Deleted credential folder {}, {moved} credentials moved up", folder.id);
    Ok(())
}

/// File credentials in a folder of their vault, or at the top (None). A
/// credential in another vault than the folder is left where it is.
/// Returns how many moved.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn saved_credential_move(
    state: State<'_, AppState>,
    credential_ids: Vec<String>,
    folder_id: Option<String>,
) -> Result<u32, String> {
    let folder_id = folder_id.filter(|f| !f.is_empty());
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let folders = read_folders(&manager).await;
    let target = match &folder_id {
        Some(id) => Some(folders.iter().find(|f| &f.id == id).ok_or_else(|| format!("Folder not found: {id}"))?),
        None => None,
    };
    let wanted: HashSet<&str> = credential_ids.iter().map(String::as_str).collect();
    let mut vaults: Vec<(String, Option<String>)> = Vec::new();
    if let Some(v) = sessions::sessions_vault_id(&manager) {
        vaults.push((v, None));
    }
    for v in manager.list_vaults().await.unwrap_or_default() {
        if !v.name.starts_with("__") {
            vaults.push((v.id.clone(), Some(v.id)));
        }
    }
    let mut moved = 0u32;
    for (vault, as_vault) in vaults {
        if let Some(t) = target {
            if t.vault_id != as_vault {
                continue;
            }
        }
        let Ok(secrets) = manager.read_secrets_in(&vault, &["credential"]).await else { continue };
        for (meta, plaintext) in secrets {
            let Ok(plaintext) = plaintext else { continue };
            let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(plaintext.expose_secret()) else { continue };
            let Some(obj) = value.as_object_mut() else { continue };
            let Some(id) = obj.get("id").and_then(|v| v.as_str()) else { continue };
            if !wanted.contains(id) {
                continue;
            }
            match &folder_id {
                Some(f) => obj.insert("folder_id".into(), serde_json::Value::String(f.clone())),
                None => obj.remove("folder_id"),
            };
            let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
            manager
                .update_secret(&vault, &meta.id, SecretBox::new(Box::new(json)))
                .await
                .map_err(|e| e.to_string())?;
            moved += 1;
        }
    }
    tracing::info!("Moved {moved} credentials to folder {folder_id:?}");
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, parent: Option<&str>, vault: Option<&str>) -> CredentialFolder {
        CredentialFolder {
            id: id.into(),
            name: id.to_uppercase(),
            parent_id: parent.map(Into::into),
            vault_id: vault.map(Into::into),
            icon: None,
            color: None,
        }
    }

    #[test]
    fn a_folder_goes_under_one_of_its_own_vault() {
        let folders = vec![folder("a", None, None), folder("b", None, None), folder("s", None, Some("shared"))];
        assert!(check_parent(&folders, &folders[0], Some("b")).is_ok());
        assert!(check_parent(&folders, &folders[0], None).is_ok());
        assert!(check_parent(&folders, &folders[0], Some("s")).is_err());
        assert!(check_parent(&folders, &folders[0], Some("gone")).is_err());
    }

    #[test]
    fn a_folder_does_not_go_into_itself_or_below() {
        let folders = vec![folder("a", None, None), folder("b", Some("a"), None), folder("c", Some("b"), None)];
        assert!(check_parent(&folders, &folders[0], Some("a")).is_err());
        assert!(check_parent(&folders, &folders[0], Some("c")).is_err());
        assert!(check_parent(&folders, &folders[2], Some("a")).is_ok());
    }

    #[test]
    fn a_cycle_in_stored_folders_ends() {
        let folders = vec![folder("a", Some("b"), None), folder("b", Some("a"), None), folder("x", None, None)];
        assert!(check_parent(&folders, &folders[2], Some("a")).is_ok());
    }

    #[test]
    fn a_credential_is_filed_only_in_its_own_vault() {
        let folders = vec![folder("p", None, None), folder("s", None, Some("shared"))];
        assert!(in_vault(&folders, "p", None));
        assert!(!in_vault(&folders, "p", Some("shared")));
        assert!(in_vault(&folders, "s", Some("shared")));
        assert!(!in_vault(&folders, "gone", None));
    }

    #[test]
    fn a_credential_without_a_folder_reads_and_writes_as_before() {
        let old: crate::state::SavedCredential = serde_json::from_str(
            r#"{"id":"c","name":"C","username":"u","auth_method":{"type":"Agent"}}"#,
        )
        .unwrap();
        assert_eq!(old.folder_id, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("folder_id"));
    }

    #[test]
    fn folders_are_stored_where_older_versions_do_not_look() {
        assert_eq!(stored_category(), category().to_string());
        assert_ne!(stored_category(), "folder");
        assert_ne!(stored_category(), "credential");
    }
}
