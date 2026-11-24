use super::*;
use crate::ipc::sync_account_commands::AccountView;
use crate::vault::sync_account_store::{AccountTarget, ORG_PREFIX};
use reqwest::StatusCode;
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
pub struct Organization {
    id: Uuid,
    name: String,
    role: String,
}

#[derive(Deserialize)]
struct SharedAccess {
    id: Uuid,
    role: String,
    wrapped_key: Option<String>,
}

#[derive(Serialize)]
pub struct SharedOrganizationVault {
    id: Uuid,
    role: String,
    name: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct OrganizationVaultGrant {
    username: String,
    role: String,
}

#[derive(Deserialize, Serialize)]
struct Envelope {
    issuer: String,
    wrapped: WrappedDek,
}

fn organization_error(message: &str) -> VaultError {
    VaultError::SyncError(message.to_owned())
}

pub(in crate::vault) fn valid_organization_vault_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with("__")
        && name.chars().count() <= 64
        && !name.chars().any(char::is_control)
}

fn key_context(target: &AccountTarget) -> Result<[u8; 32], VaultError> {
    let org = target
        .organization
        .ok_or_else(|| organization_error("Missing vault organization"))?;
    let mut context = [0u8; 32];
    context[..16].copy_from_slice(org.as_bytes());
    context[16..].copy_from_slice(target.vault_id.as_bytes());
    Ok(context)
}

fn peer_key(value: &str) -> Result<PublicKey, VaultError> {
    let bytes: [u8; 32] = BASE64
        .decode(value.trim())
        .map_err(|_| organization_error("Invalid vault public key"))?
        .try_into()
        .map_err(|_| organization_error("Invalid vault public key"))?;
    Ok(PublicKey::from(bytes))
}

fn wrapping_key(
    identity: &UserIdentity,
    peer: &PublicKey,
    target: &AccountTarget,
) -> Result<zeroize::Zeroizing<[u8; 32]>, VaultError> {
    let shared = identity.with_secret(|key| key.diffie_hellman(peer));
    if !shared.was_contributory() {
        return Err(organization_error("Invalid vault public key"));
    }
    let mut key = zeroize::Zeroizing::new([0; 32]);
    let context = key_context(target)?;
    hkdf::Hkdf::<Sha256>::new(Some(&context), shared.as_bytes())
        .expand(b"reachercm-organization-vault", &mut key[..])
        .map_err(|_| organization_error("Could not derive the vault member key"))?;
    Ok(key)
}

fn wrap_for_member(
    identity: &UserIdentity,
    master: &Dek,
    peer: &str,
    target: &AccountTarget,
) -> Result<String, VaultError> {
    let recipient = peer_key(peer)?;
    let key = wrapping_key(identity, &recipient, target)?;
    let envelope = Envelope {
        issuer: BASE64.encode(identity.public_key.as_bytes()),
        wrapped: wrap_dek_with_key(&key, master)?,
    };
    Ok(BASE64.encode(serde_json::to_vec(&envelope)?))
}

fn open_envelope(
    identity: &UserIdentity,
    envelope: &str,
    target: &AccountTarget,
) -> Result<Dek, VaultError> {
    let data = BASE64
        .decode(envelope)
        .map_err(|_| organization_error("Invalid shared vault key"))?;
    let envelope: Envelope = serde_json::from_slice(&data)?;
    let key = wrapping_key(identity, &peer_key(&envelope.issuer)?, target)?;
    unwrap_dek_with_key(&key, &envelope.wrapped)
}

impl VaultConnection {
    pub(super) fn is_organization_vault(&self) -> bool {
        self.sync_url
            .as_deref()
            .is_some_and(|url| url.starts_with(ORG_PREFIX))
    }

    async fn save_organization_access(
        &self,
        role: &str,
        envelope: Option<&str>,
    ) -> Result<(), VaultError> {
        let conn = self.conn()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS organization_vault_access (
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1), role TEXT NOT NULL, wrapped_key TEXT
            )", (),
        ).await?;
        conn.execute(
            "INSERT OR REPLACE INTO organization_vault_access (singleton,role,wrapped_key) VALUES (1,?,?)",
            (role, envelope),
        ).await?;
        Ok(())
    }

    async fn organization_access(&self) -> Result<(String, Option<String>), VaultError> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT role, wrapped_key FROM organization_vault_access WHERE singleton = 1",
                (),
            )
            .await?;
        let row = rows.next().await?.ok_or_else(|| {
            organization_error("Open the organization vault from Settings → Account")
        })?;
        Ok((row.get(0)?, row.get(1)?))
    }

    pub(super) async fn organization_master(
        &self,
        identity: &UserIdentity,
    ) -> Result<Dek, VaultError> {
        let (_, key) = self.organization_access().await?;
        let key =
            key.ok_or_else(|| organization_error("This account has no wrapped key for the vault"))?;
        let target = AccountTarget::parse(
            self.sync_url.as_deref().ok_or(VaultError::Locked)?,
            self.auth_token.as_deref().ok_or(VaultError::Locked)?,
        )?;
        open_envelope(identity, &key, &target)
    }

    pub(super) async fn ensure_organization_writable(&self) -> Result<(), VaultError> {
        if self.is_organization_vault() {
            let (role, _) = self.organization_access().await?;
            if !matches!(role.as_str(), "owner" | "editor") {
                return Err(VaultError::AccessDenied(
                    "This organization vault is read-only".into(),
                ));
            }
        }
        Ok(())
    }
}

impl VaultManager {
    pub(crate) async fn organizations(
        account: &AccountView,
    ) -> Result<Vec<Organization>, VaultError> {
        let server = AccountTarget::from_account(account, Uuid::nil())?;
        let url = server.api("/api/v1/organizations");
        server
            .request(|http| http.get(url.clone()))
            .await?
            .error_for_status()
            .map_err(|_| organization_error("The server could not list organizations"))?
            .json()
            .await
            .map_err(|_| organization_error("Invalid organization list"))
    }

    async fn shared_access(
        account: &AccountView,
        org: Uuid,
    ) -> Result<Vec<SharedAccess>, VaultError> {
        let server = AccountTarget::from_account(account, Uuid::nil())?;
        let url = server.api(&format!("/api/v1/organizations/{org}/vaults"));
        server
            .request(|http| http.get(url.clone()))
            .await?
            .error_for_status()
            .map_err(|_| organization_error("The server could not list organization vaults"))?
            .json()
            .await
            .map_err(|_| organization_error("Invalid organization vault list"))
    }

    pub(crate) async fn organization_vaults(
        &self,
        account: &AccountView,
        org: Uuid,
    ) -> Result<Vec<SharedOrganizationVault>, VaultError> {
        let access = Self::shared_access(account, org).await?;
        access
            .into_iter()
            .map(|shared| {
                let config = AccountTarget::shared(account, org, shared.id)?.config();
                Ok(SharedOrganizationVault {
                    id: shared.id,
                    role: shared.role,
                    name: self
                        .user_vaults
                        .iter()
                        .find(|entry| {
                            entry.sync_url.as_deref() == Some(config.as_str())
                                && entry.sync_token.as_deref() == Some(account.username.as_str())
                        })
                        .map(|entry| entry.name.clone()),
                })
            })
            .collect()
    }

    pub(crate) async fn create_organization_vault(
        &mut self,
        account: &AccountView,
        org: Uuid,
        name: &str,
    ) -> Result<VaultInfo, VaultError> {
        let name = name.trim();
        if !valid_organization_vault_name(name) {
            return Err(organization_error(
                "Enter a vault name of up to 64 characters",
            ));
        }
        // Confirm organization access before creating the local copy.
        Self::shared_access(account, org).await?;
        let info = self
            .create_vault(
                name,
                VaultType::Shared {
                    members: Vec::new(),
                },
                None,
                None,
            )
            .await?;
        let id = Uuid::parse_str(&info.id).map_err(|_| organization_error("Invalid vault ID"))?;
        let target = AccountTarget::shared(account, org, id)?;
        let vault = self.vaults.get(&info.id).ok_or(VaultError::Locked)?;
        let master = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;
        let sync = sync_engine::Target {
            vault_id: info.id.clone(),
            local_path: self.app_dir.join("vaults").join(format!("{id}.db")),
            backend: sync_engine::Backend::Account(target.clone()),
            key: sync_record::RecordKey::derive(master, &info.id)?,
            master: master.with_key(|k| Dek::from_fn(|d| d.copy_from_slice(k))),
            device: self.device_id.clone().ok_or(VaultError::Locked)?,
            my_uuid: self.user_uuid.clone().ok_or(VaultError::Locked)?,
        };
        vault.save_organization_access("owner", None).await?;
        sync_engine::sync(&sync).await?;
        self.save_organization_vault(info.clone(), target.config(), &account.username)
            .await?;
        Ok(info)
    }

    async fn save_organization_vault(
        &mut self,
        info: VaultInfo,
        config: String,
        user: &str,
    ) -> Result<(), VaultError> {
        let before = self.user_vaults.clone();
        self.user_vaults.retain(|entry| entry.id != info.id);
        self.user_vaults.push(StoredVaultRef {
            id: info.id.clone(),
            name: info.name,
            vault_type: info.vault_type,
            sync_url: Some(config.clone()),
            sync_token: Some(user.to_owned()),
        });
        if let Err(err) = self.save_identity_current().await {
            self.user_vaults = before;
            return Err(err);
        }
        let vault = self.vaults.get_mut(&info.id).ok_or(VaultError::Locked)?;
        vault.sync_url = Some(config);
        vault.auth_token = Some(user.to_owned());
        Ok(())
    }

    pub(crate) async fn open_organization_vault(
        &mut self,
        account: &AccountView,
        org: Uuid,
        id: Uuid,
    ) -> Result<VaultInfo, VaultError> {
        let target = AccountTarget::shared(account, org, id)?;
        let config = target.config();
        let id_string = id.to_string();
        let access = Self::shared_access(account, org)
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| organization_error("This account has no access to the vault"))?;
        if !matches!(access.role.as_str(), "owner" | "editor" | "viewer") {
            return Err(organization_error("Invalid shared vault role"));
        }
        let dir = self.app_dir.join("vaults");
        let path = dir.join(format!("{id}.db"));
        let existing = self.user_vaults.iter().find(|entry| entry.id == id_string);
        let local_conflict = match existing {
            Some(entry) => {
                entry.sync_url.as_deref() != Some(config.as_str())
                    || entry.sync_token.as_deref() != Some(account.username.as_str())
            }
            None => path.exists() || self.vaults.contains_key(&id_string),
        };
        if local_conflict || self.internal_vault_ids.values().any(|id| *id == id_string) {
            return Err(organization_error(
                "Another local vault already uses this ID",
            ));
        }
        let kek = self.kek.as_ref().ok_or(VaultError::Locked)?;
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let (bytes, _) = target
            .get()
            .await?
            .ok_or_else(|| organization_error("The vault owner has not uploaded a bundle"))?;
        let (header, _) = crate::vault::sync_bundle::read_header(&bytes)?;
        let [_, libsql::Value::Text(name), _, libsql::Value::Text(owner), _, libsql::Value::Text(kind), libsql::Value::Text(wrapped)] =
            header.as_slice()
        else {
            return Err(organization_error("Invalid organization vault header"));
        };
        if !valid_organization_vault_name(name)
            || !matches!(
                serde_json::from_str::<VaultType>(kind)?,
                VaultType::Shared { members } if members.is_empty()
            )
        {
            return Err(organization_error("Invalid organization vault type"));
        }
        let master = match &access.wrapped_key {
            Some(envelope) => open_envelope(identity, envelope, &target)?,
            None if self.user_uuid.as_ref() == Some(owner) => {
                unwrap_dek(kek, &serde_json::from_str(wrapped)?)?
            }
            None => {
                return Err(organization_error(
                    "This device does not have the vault owner's identity",
                ))
            }
        };
        let key = sync_record::RecordKey::derive(&master, &id_string)?;
        let (header, members) = crate::vault::sync_bundle::verify(&key, &bytes)?;
        if !members.is_empty() {
            return Err(organization_error("Invalid organization vault members"));
        }
        private_fs::private_dir(&dir)?;
        if !path.exists() {
            Self::write_bootstrap_header(&path, &header, &members).await?;
        }
        let info = self.open_vault(&id_string, None, None).await?;
        let vault = self.vaults.get_mut(&id_string).ok_or(VaultError::Locked)?;
        vault
            .save_organization_access(&access.role, access.wrapped_key.as_deref())
            .await?;
        vault.master_dek = Some(master);
        self.save_organization_vault(info.clone(), config, &account.username)
            .await?;
        let target = self
            .sync_targets(Some(&id_string))?
            .pop()
            .ok_or(VaultError::Locked)?;
        sync_engine::sync(&target).await?;
        Ok(info)
    }

    pub(crate) async fn organization_grants(
        account: &AccountView,
        org: Uuid,
        id: Uuid,
    ) -> Result<Vec<OrganizationVaultGrant>, VaultError> {
        let target = AccountTarget::shared(account, org, id)?;
        let url = target.api(&format!("{}/members", target.vault_path()));
        target
            .request(|http| http.get(url.clone()))
            .await?
            .error_for_status()
            .map_err(|_| organization_error("The server could not list the vault's grants"))?
            .json()
            .await
            .map_err(|_| organization_error("Invalid vault grant list"))
    }

    pub(crate) async fn change_organization_grant(
        &self,
        id: &str,
        username: &str,
        role: Option<&str>,
        public_key: &str,
    ) -> Result<(), VaultError> {
        let username = username.trim().to_ascii_lowercase();
        if !(3..=32).contains(&username.len())
            || !username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(organization_error(
                "Enter the invited user's server username",
            ));
        }
        let vault = self.vaults.get(id).ok_or(VaultError::Locked)?;
        if !vault.is_organization_vault() {
            return Err(organization_error(
                "This vault is not an organization vault",
            ));
        }
        let target = AccountTarget::parse(
            vault.sync_url.as_deref().ok_or(VaultError::Locked)?,
            vault.auth_token.as_deref().ok_or(VaultError::Locked)?,
        )?;
        let url = target.api(&format!("{}/members/{username}", target.vault_path()));
        let response = if let Some(role) = role {
            if !matches!(role, "viewer" | "editor" | "owner") {
                return Err(organization_error("Invalid vault sharing role"));
            }
            let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
            let master = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;
            let key = wrap_for_member(identity, master, public_key, &target)?;
            let body = serde_json::json!({"role": role, "wrapped_key": key});
            target
                .request(|http| http.put(url.clone()).json(&body))
                .await?
        } else {
            target.request(|http| http.delete(url.clone())).await?
        };
        if response.status() != StatusCode::NO_CONTENT {
            return Err(organization_error("The server refused the vault access change. Check that you own the vault and the recipient belongs to the organization"));
        }
        if role == Some("owner") {
            let (_, envelope) = vault.organization_access().await?;
            vault
                .save_organization_access("viewer", envelope.as_deref())
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> UserIdentity {
        UserIdentity::new(
            Uuid::new_v4().to_string(),
            x25519_dalek::StaticSecret::from(rand::random::<[u8; 32]>()),
        )
    }

    #[test]
    fn envelope_opens_only_for_the_recipient_and_vault() {
        let owner = identity();
        let recipient = identity();
        let stranger = identity();
        let org = Uuid::new_v4();
        let id = Uuid::new_v4();
        let target_for = |org, id| {
            AccountTarget::parse(
                &format!("{ORG_PREFIX}https://sync.example.com/{org}/{id}"),
                "recipient",
            )
            .unwrap()
        };
        let target = target_for(org, id);
        let master = generate_dek();
        let envelope = wrap_for_member(
            &owner,
            &master,
            &BASE64.encode(recipient.public_key.as_bytes()),
            &target,
        )
        .unwrap();
        let unwrapped = open_envelope(&recipient, &envelope, &target).unwrap();
        master.with_key(|expected| unwrapped.with_key(|key| assert_eq!(key, expected)));
        assert!(open_envelope(&stranger, &envelope, &target).is_err());
        assert!(open_envelope(&recipient, &envelope, &target_for(org, Uuid::new_v4())).is_err());
        assert!(open_envelope(&recipient, &envelope, &target_for(Uuid::new_v4(), id)).is_err());
        assert!(wrap_for_member(&owner, &master, &BASE64.encode([0; 32]), &target).is_err());
    }
}
