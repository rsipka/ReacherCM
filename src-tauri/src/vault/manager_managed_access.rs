use super::*;
use crate::ipc::sync_account_commands::{
    AccountView, DeviceVaultPolicy, DeviceVaultState, ManagedVaultRemoval,
};
use crate::vault::sync_account_store::AccountTarget;

pub(super) const SUSPENDED: &str = "The Reacher CM Sync account for this vault is suspended";

const FILE: &str = "vault_access_policy.json";

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
struct AccountScope {
    server: String,
    username: String,
}

impl From<&AccountView> for AccountScope {
    fn from(account: &AccountView) -> Self {
        Self {
            server: account.server.clone(),
            username: account.username.clone(),
        }
    }
}

impl AccountScope {
    fn matches(&self, target: &AccountTarget) -> bool {
        target.matches_account(&self.server, &self.username)
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct VaultPolicyState {
    suspended: Vec<AccountScope>,
    allow_removal: Vec<AccountScope>,
}

impl VaultPolicyState {
    pub(super) fn load(app_dir: &std::path::Path) -> Self {
        std::fs::read(app_dir.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    async fn save(&self, app_dir: &std::path::Path) -> Result<(), VaultError> {
        private_fs::private_dir(app_dir)?;
        private_fs::write_async(&app_dir.join(FILE), serde_json::to_vec(self)?).await?;
        Ok(())
    }
}

fn account_target(url: Option<&str>, user: Option<&str>) -> Option<AccountTarget> {
    AccountTarget::parse(url?, user?).ok()
}

impl VaultManager {
    pub(super) fn has_account_vaults(&self) -> bool {
        account_target(
            self.personal_sync_url.as_deref(),
            self.personal_sync_token.as_deref(),
        )
        .is_some()
            || self.user_vaults.iter().any(|entry| {
                account_target(entry.sync_url.as_deref(), entry.sync_token.as_deref()).is_some()
            })
    }

    pub(crate) fn managed_removal_enabled(&self, account: &AccountView) -> bool {
        self.vault_policy.allow_removal.contains(&account.into())
    }

    pub(crate) async fn set_managed_removal(
        &mut self,
        account: &AccountView,
        enabled: bool,
    ) -> Result<(), VaultError> {
        let mut next = self.vault_policy.clone();
        let scope = AccountScope::from(account);
        next.allow_removal.retain(|item| item != &scope);
        if enabled {
            next.allow_removal.push(scope);
        }
        next.save(&self.app_dir).await?;
        self.vault_policy = next;
        Ok(())
    }

    fn policy_target(&self, id: &str, vault: &VaultConnection) -> Option<AccountTarget> {
        if vault.sync_url.is_some() {
            return account_target(vault.sync_url.as_deref(), vault.auth_token.as_deref());
        }
        if let Some(entry) = self.user_vaults.iter().find(|entry| entry.id == id) {
            return account_target(entry.sync_url.as_deref(), entry.sync_token.as_deref());
        }
        if self
            .internal_vault_ids
            .get("__personal__")
            .is_some_and(|value| value == id)
        {
            return account_target(
                self.personal_sync_url.as_deref(),
                self.personal_sync_token.as_deref(),
            );
        }
        None
    }

    pub(super) fn account_vault_suspended(&self, id: &str, vault: &VaultConnection) -> bool {
        self.policy_target(id, vault).is_some_and(|target| {
            self.vault_policy
                .suspended
                .iter()
                .any(|scope| scope.matches(&target))
        })
    }

    pub(super) async fn apply_managed_vault_policy(
        &mut self,
        account: &AccountView,
        policy: DeviceVaultPolicy,
    ) -> Vec<String> {
        let scope = AccountScope::from(account);
        let mut changed = Vec::new();
        match policy.state {
            DeviceVaultState::Suspended => {
                if !self.vault_policy.suspended.contains(&scope) {
                    self.vault_policy.suspended.push(scope.clone());
                }
                if let Err(error) = self.vault_policy.save(&self.app_dir).await {
                    tracing::warn!(%error, "Could not persist Sync vault suspension");
                }
                changed = self
                    .vaults
                    .iter()
                    .filter(|(id, vault)| {
                        vault.master_dek.is_some()
                            && self
                                .policy_target(id, vault)
                                .is_some_and(|target| scope.matches(&target))
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &changed {
                    self.lock_vault(id);
                }
            }
            DeviceVaultState::Active => {
                let was_suspended = self.vault_policy.suspended.contains(&scope);
                if was_suspended {
                    let mut next = self.vault_policy.clone();
                    next.suspended.retain(|item| item != &scope);
                    if let Err(error) = next.save(&self.app_dir).await {
                        tracing::warn!(%error, "Could not clear persisted Sync vault suspension");
                        return changed;
                    }
                    self.vault_policy = next;
                }
                if self.managed_removal_enabled(account) && !self.is_locked() {
                    for removal in policy.removals {
                        match self.apply_managed_vault_removal(&scope, &removal).await {
                            Ok(true) => changed.push(removal.vault_id.to_string()),
                            Ok(false) => {}
                            Err(error) => {
                                tracing::warn!(%error, "Could not remove a managed vault copy")
                            }
                        }
                    }
                }
                if !self.is_locked() {
                    if account_target(
                        self.personal_sync_url.as_deref(),
                        self.personal_sync_token.as_deref(),
                    )
                    .is_some_and(|target| scope.matches(&target))
                        && self
                            .personal_vault_id()
                            .is_none_or(|id| !self.vaults.contains_key(&id))
                    {
                        if let Err(error) = self.open_internal_vaults().await {
                            tracing::warn!(%error, "Could not reopen the personal Sync vault");
                        }
                    }
                    if was_suspended {
                        let ids: Vec<_> = self
                            .vaults
                            .iter()
                            .filter(|(id, vault)| {
                                self.policy_target(id, vault)
                                    .is_some_and(|target| scope.matches(&target))
                            })
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in ids {
                            if self.unlock_vault_keys(&id).await.is_ok() {
                                changed.push(id);
                            }
                        }
                    }
                }
            }
            DeviceVaultState::Revoked => {}
        }
        changed
    }

    async fn apply_managed_vault_removal(
        &mut self,
        scope: &AccountScope,
        removal: &ManagedVaultRemoval,
    ) -> Result<bool, VaultError> {
        let id = removal.vault_id.to_string();
        if self.internal_vault_ids.values().any(|value| value == &id) {
            return Ok(false);
        }
        let Some(index) = self.user_vaults.iter().position(|entry| {
            entry.id == id
                && entry.managed_grant == Some(removal.grant_id)
                && account_target(entry.sync_url.as_deref(), entry.sync_token.as_deref())
                    .is_some_and(|target| {
                        scope.matches(&target)
                            && target.vault_id == removal.vault_id
                            && target.organization == Some(removal.organization_id)
                    })
        }) else {
            return Ok(false);
        };
        // An open vault must agree with its persistent reference as well.
        let entry = &self.user_vaults[index];
        if self.vaults.get(&id).is_some_and(|vault| {
            vault.sync_url != entry.sync_url || vault.auth_token != entry.sync_token
        }) {
            return Ok(false);
        }

        self.close_vault(&id).await?;
        let vault_dir = self.app_dir.join("vaults");
        for suffix in [".db", ".db-wal", ".db-shm", ".db-journal", ".cache"] {
            match tokio::fs::remove_file(vault_dir.join(format!("{id}{suffix}"))).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }

        // Keep the reference on a persistence failure so the same instruction
        // can retry after an interrupted deletion.
        let entry = self.user_vaults.remove(index);
        if let Err(error) = self.save_identity_current().await {
            self.user_vaults.insert(index, entry);
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(all(test, desktop))]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn account(server: &str) -> AccountView {
        serde_json::from_value(json!({
            "server": server, "username": "member", "deviceId": "synthetic-installation"
        }))
        .unwrap()
    }

    fn policy(state: &str, removals: serde_json::Value) -> DeviceVaultPolicy {
        serde_json::from_value(json!({
            "version": 1, "device_id": "synthetic-installation", "username": "member",
            "state": state, "removals": removals,
        }))
        .unwrap()
    }

    fn removal(org: Uuid, id: &str, grant: Uuid) -> serde_json::Value {
        json!([{"organization_id": org, "vault_id": id, "grant_id": grant}])
    }

    #[tokio::test]
    async fn managed_removal_requires_opt_in_and_an_exact_shared_grant() {
        let app = tempfile::tempdir().unwrap();
        let mut manager = VaultManager::new(app.path().into());
        manager.init_identity("local test password").await.unwrap();
        let account = account("https://sync.example.com/");
        let foreign_account = self::account("https://other.example.com/");
        let org = Uuid::new_v4();
        let grant = Uuid::new_v4();
        let matched = manager
            .create_vault("Managed", VaultType::Private, None, None)
            .await
            .unwrap()
            .id;
        let other = manager
            .create_vault("Local", VaultType::Private, None, None)
            .await
            .unwrap()
            .id;
        let matched_id = Uuid::parse_str(&matched).unwrap();
        let config = AccountTarget::shared(&account, org, matched_id)
            .unwrap()
            .config();
        let entry = manager
            .user_vaults
            .iter_mut()
            .find(|v| v.id == matched)
            .unwrap();
        entry.sync_url = Some(config.clone());
        entry.sync_token = Some(account.username.clone());
        entry.managed_grant = Some(grant);
        let opened = manager.vaults.get_mut(&matched).unwrap();
        opened.sync_url = Some(config);
        opened.auth_token = Some(account.username.clone());
        manager.save_identity_current().await.unwrap();
        let matched_file = app.path().join("vaults").join(format!("{matched}.db"));
        let other_file = app.path().join("vaults").join(format!("{other}.db"));

        manager
            .apply_managed_vault_policy(&account, policy("active", removal(org, &matched, grant)))
            .await;
        assert!(matched_file.exists(), "the default policy is opt out");

        manager.set_managed_removal(&account, true).await.unwrap();
        manager
            .set_managed_removal(&foreign_account, true)
            .await
            .unwrap();
        drop(manager);
        let mut manager = VaultManager::new(app.path().into());
        assert!(manager.unlock("local test password").await.unwrap());
        assert!(manager.managed_removal_enabled(&account));
        assert!(manager
            .user_vaults
            .iter()
            .any(|v| v.id == matched && v.managed_grant == Some(grant)));

        for (scope, state, data) in [
            (&account, "active", json!([])),
            (&account, "revoked", removal(org, &matched, grant)),
            (&account, "active", removal(org, &matched, Uuid::new_v4())),
            (&account, "active", removal(Uuid::new_v4(), &matched, grant)),
            (&foreign_account, "active", removal(org, &matched, grant)),
            (&account, "active", removal(org, &other, grant)),
        ] {
            assert!(manager
                .apply_managed_vault_policy(scope, policy(state, data))
                .await
                .is_empty());
            assert!(matched_file.exists() && other_file.exists());
        }

        let expected = || policy("active", removal(org, &matched, grant));
        assert_eq!(
            manager
                .apply_managed_vault_policy(&account, expected())
                .await,
            vec![matched.clone()]
        );
        assert!(!matched_file.exists());
        assert!(other_file.exists());
        assert!(manager.user_vaults.iter().all(|v| v.id != matched));
        assert!(manager
            .apply_managed_vault_policy(&account, expected())
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn managed_suspension_survives_restart_and_does_not_delete_or_unlock_on_revocation() {
        let app = tempfile::tempdir().unwrap();
        let mut manager = VaultManager::new(app.path().into());
        let password = "local test password";
        manager.init_identity(password).await.unwrap();
        let account = account("https://sync.example.com/");
        let local = manager
            .create_vault("Local", VaultType::Private, None, None)
            .await
            .unwrap()
            .id;
        let synced = manager
            .create_vault("Synced", VaultType::Private, None, None)
            .await
            .unwrap()
            .id;
        let config = AccountTarget::from_account(&account, Uuid::parse_str(&synced).unwrap())
            .unwrap()
            .config();
        let local_db = app.path().join("vaults").join(format!("{synced}.db"));
        let entry = manager
            .user_vaults
            .iter_mut()
            .find(|v| v.id == synced)
            .unwrap();
        entry.sync_url = Some(config.clone());
        entry.sync_token = Some(account.username.clone());
        let opened = manager.vaults.get_mut(&synced).unwrap();
        opened.sync_url = Some(config);
        opened.auth_token = Some(account.username.clone());
        manager.unlock_vault_keys(&local).await.unwrap();
        manager.unlock_vault_keys(&synced).await.unwrap();
        manager.save_identity_current().await.unwrap();

        let suspended = manager
            .apply_managed_vault_policy(&account, policy("suspended", json!([])))
            .await;
        assert_eq!(suspended, vec![synced.clone()]);
        assert!(local_db.exists());
        assert!(manager.vaults[&local].master_dek.is_some());
        assert!(matches!(
            manager.unlock_vault_keys(&synced).await,
            Err(VaultError::AccessDenied(_))
        ));

        drop(manager);
        let mut reopened = VaultManager::new(app.path().into());
        assert!(reopened.unlock(password).await.unwrap());
        assert!(matches!(
            reopened.unlock_vault_keys(&synced).await,
            Err(VaultError::AccessDenied(_))
        ));
        assert!(reopened
            .apply_managed_vault_policy(&account, policy("revoked", json!([])))
            .await
            .is_empty());
        assert!(matches!(
            reopened.unlock_vault_keys(&synced).await,
            Err(VaultError::AccessDenied(_))
        ));

        // A generic open can omit sync settings. The persisted reference
        // still binds this local file to the suspended account.
        reopened.vaults.get_mut(&synced).unwrap().sync_url = None;
        assert!(matches!(
            reopened.unlock_vault_keys(&synced).await,
            Err(VaultError::AccessDenied(_))
        ));

        reopened
            .apply_managed_vault_policy(&account, policy("active", json!([])))
            .await;
        assert!(reopened.unlock_vault_keys(&synced).await.is_ok());
        assert!(local_db.exists());
    }
    #[tokio::test]
    async fn managed_missing_header_preserves_existing_database() {
        let app = tempfile::tempdir().unwrap();
        let mut manager = VaultManager::new(app.path().into());
        let account = account("https://sync.example.com/");
        let id = Uuid::new_v4();
        let config = AccountTarget::from_account(&account, id).unwrap().config();
        let vaults = app.path().join("vaults");
        std::fs::create_dir(&vaults).unwrap();
        let local_db = vaults.join(format!("{id}.db"));
        {
            let db = create_replica(&local_db, None).await.unwrap();
            let conn = local_connect(&db).unwrap();
            conn.execute("CREATE TABLE keep_local_data (value TEXT NOT NULL)", ())
                .await
                .unwrap();
            conn.execute(
                "INSERT INTO keep_local_data VALUES ('pending local data')",
                (),
            )
            .await
            .unwrap();
        }
        assert!(manager
            .open_vault(&id.to_string(), Some(&config), Some(&account.username))
            .await
            .is_err());
        let db = create_replica(&local_db, None).await.unwrap();
        let conn = local_connect(&db).unwrap();
        let mut rows = conn
            .query("SELECT value FROM keep_local_data", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "pending local data"
        );
    }
}
