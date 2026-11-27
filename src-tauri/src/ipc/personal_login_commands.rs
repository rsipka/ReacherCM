//! Each person's own login for sessions in a shared vault.
//!
//! A session (or a folder of sessions) in a vault other people use can say
//! that everyone signs in with their own login instead of one that is kept
//! in the shared vault: its `credential_id` is [`PERSONAL_LOGIN`]. What each
//! person uses is their own choice, kept in their own vault (this device's
//! `__sessions__`, carried to their other devices by personal sync) and never
//! written to the shared one. A choice is one of their own saved credentials
//! (password, key, agent or password manager entry), or [`DEFAULT_KEY`]: the
//! session's user name with the SSH key they picked as their default.
//!
//! A choice is filed under the session or folder that asked for it, by id
//! (ids are the same on every device; vault ids are not). A folder's choice
//! covers every session inside it that takes the login from the folder.
//!
//! The choice is filled in where a linked credential would be (see
//! [`crate::ipc::saved_credential_commands::fill`]), so everything that
//! reads a session sees the login it will really use, and a password manager
//! entry in it is approved per person as any other is.

use crate::ipc::session_commands as sessions;
use crate::state::AppState;
use crate::vault::types::SecretCategory;
use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

/// A session's or folder's `credential_id` that says: each person's own.
pub const PERSONAL_LOGIN: &str = "personal";

/// A choice that says: the session's user name, with my default SSH key.
/// Also an `AuthMethod::Key` `key_id` (see `sshkey_commands::resolve`).
pub const DEFAULT_KEY: &str = "@default";

/// The category the choices are stored under. An older Reach never reads it.
const CATEGORY: &str = "personal_login";

fn category() -> SecretCategory {
    SecretCategory::Custom(CATEGORY.to_string())
}

fn stored_category() -> String {
    format!("custom:{CATEGORY}")
}

/// The row a target's choice is kept in.
fn row_id(target: &str) -> String {
    format!("personal-login:{target}")
}

/// One person's choice for one session or folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersonalLogin {
    /// The session or folder id.
    pub target: String,
    /// One of their own saved credentials, or [`DEFAULT_KEY`].
    pub credential_id: String,
}

/// Every choice of the person using this device, by target.
pub(crate) async fn read_choices(manager: &crate::vault::VaultManager) -> HashMap<String, String> {
    let Some(vault) = sessions::sessions_vault_id(manager) else { return HashMap::new() };
    let cat = stored_category();
    let Ok(secrets) = manager.read_secrets_in(&vault, &[cat.as_str()]).await else { return HashMap::new() };
    secrets
        .into_iter()
        .filter_map(|(_, plaintext)| plaintext.ok())
        .filter_map(|p| serde_json::from_slice::<PersonalLogin>(p.expose_secret()).ok())
        .map(|c| (c.target, c.credential_id))
        .collect()
}

/// Whether `credential_id` can be a choice: [`DEFAULT_KEY`], or one of the
/// person's own saved credentials (never one of a shared vault: that one
/// everybody could already link).
fn check_choice(own: &[crate::state::SavedCredential], credential_id: &str) -> Result<(), String> {
    if credential_id == DEFAULT_KEY || own.iter().any(|c| c.id == credential_id && c.vault_id.is_none()) {
        Ok(())
    } else {
        Err("Choose one of your own saved credentials (in My sessions), or your default key".to_string())
    }
}

/// Everyone's own choices, for the session list and the pickers.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn personal_login_list(state: State<'_, AppState>) -> Result<Vec<PersonalLogin>, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Ok(Vec::new());
    }
    let mut list: Vec<PersonalLogin> =
        read_choices(&manager).await.into_iter().map(|(target, credential_id)| PersonalLogin { target, credential_id }).collect();
    list.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(list)
}

/// Choose the login the current person uses for a session or folder that
/// asks for each person's own (`credential_id` None: forget the choice, so
/// the next connect asks again).
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn personal_login_set(
    state: State<'_, AppState>,
    target: String,
    credential_id: Option<String>,
) -> Result<(), String> {
    let target = target.trim().to_string();
    if target.is_empty() {
        return Err("No session or folder given".to_string());
    }
    let mut manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let row = row_id(&target);
    let Some(credential_id) = credential_id.filter(|c| !c.is_empty()) else {
        if let Some(vault) = sessions::sessions_vault_id(&manager) {
            if manager.secret_exists(&vault, &row).await {
                manager.delete_secret(&vault, &row).await.map_err(|e| e.to_string())?;
            }
        }
        tracing::info!("Own login for {} forgotten", target);
        return Ok(());
    };
    let own = crate::ipc::saved_credential_commands::read_credentials(&manager).await;
    check_choice(&own, &credential_id)?;
    let vault = sessions::sessions_vault(&mut manager).await?;
    let json = serde_json::to_vec(&PersonalLogin { target: target.clone(), credential_id }).map_err(|e| e.to_string())?;
    manager
        .create_secret_with_id(&vault, &row, &row, category(), SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Own login for {} chosen", target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AuthMethod, SavedCredential};

    fn cred(id: &str, vault: Option<&str>) -> SavedCredential {
        SavedCredential {
            id: id.into(),
            name: id.into(),
            username: "me".into(),
            domain: None,
            auth_method: AuthMethod::Agent,
            vault_id: vault.map(Into::into),
            password_history: None,
            folder_id: None,
        }
    }

    #[test]
    fn only_own_credentials_or_the_default_key_can_be_chosen() {
        let creds = [cred("mine", None), cred("team", Some("v1"))];
        assert!(check_choice(&creds, "mine").is_ok());
        assert!(check_choice(&creds, DEFAULT_KEY).is_ok());
        assert!(check_choice(&creds, "team").is_err(), "a shared vault's credential is not a personal choice");
        assert!(check_choice(&creds, "gone").is_err());
    }
}
