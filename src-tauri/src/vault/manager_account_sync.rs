use super::*;
#[cfg(desktop)]
use {
    crate::ipc::sync_account_commands::AccountView,
    crate::vault::sync_account_store::{AccountTarget, PREFIX},
    sha2::Digest,
};

#[cfg(desktop)]
const PERSONAL: &str = "__personal__";

#[cfg(desktop)]
fn sync_error(message: &str) -> VaultError {
    VaultError::SyncError(message.into())
}

#[cfg(desktop)]
impl VaultManager {
    pub(crate) async fn enable_account_sync(
        &mut self,
        account: &AccountView,
    ) -> Result<String, VaultError> {
        if self.is_locked() {
            return Err(VaultError::Locked);
        }
        let existing = self.personal_sync_url.is_some() && self.personal_sync_token.is_some();
        let key = format!(
            "__account_stage__:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(format!(
                "{}\0{}",
                account.server, account.username
            )))
        );
        let listed = AccountTarget::from_account(account, uuid::Uuid::nil())?
            .listed()
            .await?;
        let staged = self.internal_vault_ids.get(&key).cloned();
        let id = if existing {
            self.get_vault_id_by_name(PERSONAL)
                .ok_or_else(|| sync_error("Open the existing personal vault before moving it"))?
        } else if let Some(id) = staged {
            id
        } else {
            if !listed.is_empty() {
                return Err(sync_error("This sync account already has a personal vault. Use a vault link from the other device"));
            }
            let info = self
                .create_vault(PERSONAL, VaultType::Private, None, None)
                .await?;
            self.vault_names.remove(PERSONAL);
            self.internal_vault_ids.insert(key.clone(), info.id.clone());
            if let Err(error) = self.save_identity_current().await {
                self.internal_vault_ids.remove(&key);
                return Err(error);
            }
            info.id
        };
        let uuid =
            uuid::Uuid::parse_str(&id).map_err(|_| sync_error("Invalid personal vault ID"))?;
        if listed.iter().any(|remote| *remote != uuid) {
            return Err(sync_error(
                "This sync account already has a different personal vault",
            ));
        }
        let target = AccountTarget::from_account(account, uuid)?;
        let config = target.config();
        if existing {
            // Pull the latest changes from the old location before moving.
            let previous = self.personal_sync_url.as_deref().unwrap_or("");
            if previous.starts_with(PREFIX)
                && (previous != config
                    || self.personal_sync_token.as_deref() != Some(&account.username))
            {
                return Err(sync_error(
                    "Use the account already configured for this personal vault",
                ));
            }
            let old = self
                .sync_targets(Some(&id))?
                .into_iter()
                .next()
                .ok_or_else(|| sync_error("Unlock the existing personal vault before moving it"))?;
            if previous != config {
                sync_engine::sync(&old).await?;
            }
        } else {
            self.open_vault(&id, None, None).await?;
            self.vault_names.remove(PERSONAL);
            self.unlock_vault(&id).await?;
            self.stage_local_items(&id).await?;
        }

        let vault = self
            .vaults
            .get(&id)
            .ok_or_else(|| VaultError::NotFound(id.clone()))?;
        if !matches!(vault.header.vault_type, VaultType::Private) || vault.header.id != id {
            return Err(sync_error(
                "Only the personal vault can use Reacher CM Sync",
            ));
        }
        let master = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;
        let sync_target = sync_engine::Target {
            vault_id: id.clone(),
            local_path: self.app_dir.join("vaults").join(format!("{id}.db")),
            backend: sync_engine::Backend::Account(target.clone()),
            key: sync_record::RecordKey::derive(master, &id)?,
            master: master.with_key(|k| Dek::from_fn(|d| d.copy_from_slice(k))),
            device: self.device_id.clone().ok_or(VaultError::Locked)?,
            my_uuid: self.user_uuid.clone().ok_or(VaultError::Locked)?,
        };
        let report = sync_engine::sync(&sync_target).await?;
        let (bytes, _) = target
            .get()
            .await?
            .ok_or_else(|| sync_error("The server did not return the uploaded vault"))?;
        crate::vault::sync_bundle::verify(&sync_target.key, &bytes)?;

        // The previous backend and local vault names survive any failed upload
        // or identity save. Retire the local copies only from the active map.
        let previous_url = self.personal_sync_url.replace(config.clone());
        let previous_token = self.personal_sync_token.replace(account.username.clone());
        let previous_ids = self.internal_vault_ids.clone();
        let previous_names = self.vault_names.clone();
        self.internal_vault_ids.remove(&key);
        self.internal_vault_ids.insert(PERSONAL.into(), id.clone());
        self.vault_names.insert(PERSONAL.into(), id.clone());
        for name in INTERNAL_VAULTS {
            self.internal_vault_ids.insert(name.into(), id.clone());
            self.vault_names.insert(name.into(), id.clone());
        }
        if let Err(error) = self.save_identity_current().await {
            self.personal_sync_url = previous_url;
            self.personal_sync_token = previous_token;
            self.internal_vault_ids = previous_ids;
            self.vault_names = previous_names;
            return Err(error);
        }
        let vault = self
            .vaults
            .get_mut(&id)
            .expect("the personal vault is open");
        vault.sync_url = Some(config);
        vault.auth_token = Some(account.username.clone());
        for name in INTERNAL_VAULTS {
            if let Some(old_id) = previous_names.get(name).filter(|old| *old != &id) {
                self.vaults.remove(old_id);
            }
        }
        sync_engine::report_applied(&sync_target, report).await;
        Ok(id)
    }

    async fn stage_local_items(&self, target: &str) -> Result<(), VaultError> {
        let mut originals = std::collections::HashSet::new();
        for name in INTERNAL_VAULTS {
            let Some(source) = self.vault_names.get(name) else {
                continue;
            };
            for metadata in self.list_secrets(source).await? {
                let mut id = metadata.id.clone();
                if !originals.insert(id.clone()) {
                    id = unified_secret_id(name, &id);
                    if !originals.insert(id.clone()) {
                        return Err(sync_error("Two local items use the same vault ID"));
                    }
                }
                let value = self.read_secret(source, &metadata.id).await?;
                let category = metadata
                    .category
                    .parse::<SecretCategory>()
                    .map_err(|_| sync_error("A local vault item has an unknown category"))?;
                if self.find_secret(target, &id).await? {
                    self.delete_secret(target, &id).await?;
                }
                self.create_secret_with_id(target, &id, &metadata.name, category, value)
                    .await?;
                let conn = self.vaults.get(target).ok_or(VaultError::Locked)?.conn()?;
                conn.execute(
                    "UPDATE secrets SET created_at = ?, updated_at = ? WHERE id = ?",
                    (metadata.created_at, metadata.updated_at, id),
                )
                .await?;
            }
        }
        // On retry, remove staging copies deleted from the source in the meantime.
        for item in self.list_secrets(target).await? {
            if !originals.contains(&item.id) {
                self.delete_secret(target, &item.id).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "manager_account_sync_tests.rs"]
mod tests;
