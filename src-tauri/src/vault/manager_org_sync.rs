use super::*;
use crate::ipc::sync_account_commands::AccountView;
use crate::vault::sync_account_store::{AccountTarget, ORG_PREFIX};
use crate::vault::sync_local::{KEY_EPOCH_STATE, ROTATION_PENDING_STATE};
use reqwest::StatusCode;
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
pub struct Organization {
    id: Uuid,
    name: String,
    role: String,
}

#[derive(Deserialize)]
pub(super) struct SharedAccess {
    id: Uuid,
    role: String,
    wrapped_key: Option<String>,
    #[serde(default)]
    grant_id: Option<Uuid>,
    #[serde(default)]
    key_epoch: u64,
    #[serde(default)]
    rotation_required: bool,
}

impl SharedAccess {
    fn managed_grant(&self) -> Option<Uuid> {
        if self.role == "owner" { None } else { self.grant_id }
    }
}

#[derive(Serialize)]
pub struct SharedOrganizationVault {
    id: Uuid,
    role: String,
    name: Option<String>,
    rotation_required: bool,
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
    #[serde(default)]
    scope: Option<EnvelopeScope>,
}

#[derive(Deserialize, Serialize)]
struct EnvelopeScope {
    epoch: u64,
    role: String,
}

#[derive(Deserialize)]
struct RotationState {
    revision: String,
    key_epoch: u64,
    acl_version: u64,
    #[serde(default)]
    rotation_required: bool,
    grants: Vec<RotationRecipient>,
}

#[derive(Deserialize)]
struct RotationRecipient {
    username: String,
    #[serde(default)]
    recipient_proof: String,
}

#[derive(Serialize)]
struct RotatedGrant {
    wrapped_key: String,
    recipient_proof: String,
}

fn recipient_tag(
    master: &Dek,
    target: &AccountTarget,
    username: &str,
    recipient: &[u8; 32],
) -> Result<[u8; 32], VaultError> {
    let salt = key_context(target)?;
    let mut tag = [0; 32];
    let mut info = b"reachercm-organization-vault-recipient-v1\0".to_vec();
    info.extend_from_slice(recipient);
    info.extend_from_slice(username.as_bytes());
    master
        .with_key(|key| hkdf::Hkdf::<Sha256>::new(Some(&salt), key).expand(&info, &mut tag))
        .map_err(|_| organization_error("Could not authenticate the vault recipient key"))?;
    Ok(tag)
}

fn seal_recipient_proof(
    master: &Dek,
    target: &AccountTarget,
    username: &str,
    public_key: &str,
) -> Result<String, VaultError> {
    let public = peer_key(public_key)?;
    let mut proof = [0u8; 64];
    proof[..32].copy_from_slice(public.as_bytes());
    proof[32..].copy_from_slice(&recipient_tag(master, target, username, public.as_bytes())?);
    Ok(BASE64.encode(proof))
}

fn verified_recipient(
    master: &Dek,
    target: &AccountTarget,
    grant: &RotationRecipient,
) -> Result<String, VaultError> {
    use subtle::ConstantTimeEq;
    let refresh = || {
        organization_error(&format!(
            "Verify and save the public key again for {} before removing vault access",
            grant.username
        ))
    };
    let proof: [u8; 64] = BASE64
        .decode(&grant.recipient_proof)
        .map_err(|_| refresh())?
        .try_into()
        .map_err(|_| refresh())?;
    let (public, tag) = proof.split_at(32);
    let public: &[u8; 32] = public.try_into().expect("32-byte recipient key");
    if !bool::from(recipient_tag(master, target, &grant.username, public)?.ct_eq(tag)) {
        return Err(refresh());
    }
    Ok(BASE64.encode(public))
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
    scope: Option<&EnvelopeScope>,
) -> Result<zeroize::Zeroizing<[u8; 32]>, VaultError> {
    let shared = identity.with_secret(|key| key.diffie_hellman(peer));
    if !shared.was_contributory() {
        return Err(organization_error("Invalid vault public key"));
    }
    let mut key = zeroize::Zeroizing::new([0; 32]);
    let context = key_context(target)?;
    let info = match scope {
        Some(scope) => {
            let mut info = b"reachercm-organization-vault-envelope-v2\0".to_vec();
            info.extend_from_slice(&scope.epoch.to_be_bytes());
            info.extend_from_slice(scope.role.as_bytes());
            info
        }
        None => b"reachercm-organization-vault".to_vec(),
    };
    hkdf::Hkdf::<Sha256>::new(Some(&context), shared.as_bytes())
        .expand(&info, &mut key[..])
        .map_err(|_| organization_error("Could not derive the vault member key"))?;
    Ok(key)
}

fn wrap_for_member(
    identity: &UserIdentity,
    master: &Dek,
    peer: &str,
    target: &AccountTarget,
    scope: EnvelopeScope,
) -> Result<String, VaultError> {
    let recipient = peer_key(peer)?;
    let key = wrapping_key(identity, &recipient, target, Some(&scope))?;
    let envelope = Envelope {
        issuer: BASE64.encode(identity.public_key.as_bytes()),
        wrapped: wrap_dek_with_key(&key, master)?,
        scope: Some(scope),
    };
    Ok(BASE64.encode(serde_json::to_vec(&envelope)?))
}

fn decode_envelope(encoded: &str) -> Result<Envelope, VaultError> {
    let data = BASE64
        .decode(encoded)
        .map_err(|_| organization_error("Invalid shared vault key"))?;
    Ok(serde_json::from_slice(&data)?)
}

impl Envelope {
    fn open(&self, identity: &UserIdentity, target: &AccountTarget) -> Result<Dek, VaultError> {
        let key = wrapping_key(
            identity,
            &peer_key(&self.issuer)?,
            target,
            self.scope.as_ref(),
        )?;
        unwrap_dek_with_key(&key, &self.wrapped)
    }
}

fn envelope_role(role: &str) -> &'static str {
    if role == "owner" { "owner" } else { "member" }
}

fn authorize_envelope(
    identity: &UserIdentity,
    target: &AccountTarget,
    access: &SharedAccess,
    owner: Option<&PublicKey>,
    advancing: bool,
) -> Result<Option<Dek>, VaultError> {
    let Some(encoded) = &access.wrapped_key else {
        if advancing
            || access.key_epoch != 0
            || access.role != "owner"
            || owner.is_some_and(|owner| owner != &identity.public_key)
        {
            return Err(organization_error(
                "The server did not return the owner-authorized vault key",
            ));
        }
        return Ok(None);
    };
    let envelope = decode_envelope(encoded)?;
    let issuer = peer_key(&envelope.issuer)?;
    let bootstrap_own_key = owner.is_none() && access.role == "owner" && issuer == identity.public_key;
    if owner != Some(&issuer) && !bootstrap_own_key {
        return Err(VaultError::OrganizationOwnerVerificationRequired);
    }
    if let Some(scope) = &envelope.scope {
        if scope.epoch != access.key_epoch || scope.role != envelope_role(&access.role) {
            return Err(organization_error(
                "The vault key does not authorize this epoch or role",
            ));
        }
    } else if advancing {
        return Err(VaultError::OrganizationRotationUpgradeRequired);
    }
    envelope.open(identity, target).map(Some)
}

async fn write_organization_access(
    conn: &libsql::Connection,
    role: &str,
    envelope: Option<&str>,
) -> Result<(), VaultError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS organization_vault_access (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1), role TEXT NOT NULL, wrapped_key TEXT
        )",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR REPLACE INTO organization_vault_access (singleton,role,wrapped_key) VALUES (1,?,?)",
        (role, envelope),
    ).await?;
    Ok(())
}

async fn write_rotation_pending(
    conn: &libsql::Connection,
    pending: bool,
) -> Result<(), VaultError> {
    if pending {
        sync_local::set_state(conn, ROTATION_PENDING_STATE, "1").await
    } else {
        sync_local::clear_state(conn, ROTATION_PENDING_STATE).await
    }
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
        write_organization_access(&conn, role, envelope).await
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

    pub(super) async fn organization_key_epoch(&self) -> Result<u64, VaultError> {
        let conn = self.conn()?;
        sync_local::state(&conn, KEY_EPOCH_STATE)
            .await?
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| organization_error("Invalid local vault key epoch"))
            })
            .transpose()
            .map(|value| value.unwrap_or(0))
    }

    async fn adopt_organization_key(
        &mut self,
        access: &SharedAccess,
        next: Dek,
        owner_wrap: &str,
    ) -> Result<(), VaultError> {
        let current = self.organization_key_epoch().await?;
        if access.key_epoch < current {
            return Err(organization_error(
                "The sync server returned an older vault key",
            ));
        }
        let same_key = self.master_dek.as_ref().is_some_and(|previous| {
            use subtle::ConstantTimeEq;
            previous.with_key(|old| next.with_key(|new| bool::from(old.ct_eq(new))))
        });
        if current == access.key_epoch && self.master_dek.is_some() && !same_key {
            return Err(organization_error(
                "The vault key changed without a rotation",
            ));
        }
        // Install the new cached envelope and re-encrypt all existing data
        // together. The sync triggers stay off so offline edits keep their
        // original timestamps and normal conflict behavior.
        let conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        write_organization_access(&tx, &access.role, access.wrapped_key.as_deref()).await?;
        write_rotation_pending(&tx, access.rotation_required).await?;
        if access.key_epoch > current {
            if let Some(previous) = &self.master_dek {
                tx.execute("INSERT INTO sync_applying (x) VALUES (1)", ())
                    .await?;
                for mut row in sync_local::all_rows(&tx).await? {
                    crate::vault::sync_bundle::reencrypt_row(&mut row, previous, &next)?;
                    tx.execute(
                        "UPDATE secrets SET nonce=?,ciphertext=?,wrapped_dek=? WHERE id=?",
                        (row.nonce, row.ciphertext, row.wrapped_dek, row.id),
                    )
                    .await?;
                }
                tx.execute("DELETE FROM sync_applying", ()).await?;
            }
            tx.execute(
                "UPDATE vault_header SET wrapped_master_dek = ?",
                [owner_wrap],
            )
            .await?;
            sync_local::set_state(&tx, KEY_EPOCH_STATE, &access.key_epoch.to_string()).await?;
        }
        tx.commit().await?;
        self.master_dek = Some(next);
        Ok(())
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
        decode_envelope(&key)?.open(identity, &target)
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
        Self::target_access(&server, org).await
    }

    async fn target_access(
        server: &AccountTarget,
        org: Uuid,
    ) -> Result<Vec<SharedAccess>, VaultError> {
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
                    rotation_required: shared.rotation_required,
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
        let report = sync_engine::sync(&sync).await?;
        self.save_organization_vault(info.clone(), target.config(), &account.username, None)
            .await?;
        sync_engine::report_applied(&sync, report).await;
        Ok(info)
    }

    async fn save_organization_vault(
        &mut self,
        info: VaultInfo,
        config: String,
        user: &str,
        managed_grant: Option<Uuid>,
    ) -> Result<(), VaultError> {
        let before = self.user_vaults.clone();
        self.user_vaults.retain(|entry| entry.id != info.id);
        self.user_vaults.push(StoredVaultRef {
            id: info.id.clone(),
            name: info.name,
            vault_type: info.vault_type,
            sync_url: Some(config.clone()),
            sync_token: Some(user.to_owned()),
            managed_grant,
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
        verified_owner_key: Option<&str>,
    ) -> Result<VaultInfo, VaultError> {
        let verified_owner = verified_owner_key.map(peer_key).transpose()?;
        let target = AccountTarget::shared(account, org, id)?;
        let config = target.config();
        let id_string = id.to_string();
        let access = Self::shared_access(account, org)
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| organization_error("This account has no access to the vault"))?;
        let managed_grant = access.managed_grant();
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
        let info = if path.exists() {
            let info = self
                .open_vault(&id_string, Some(&config), Some(&account.username))
                .await?;
            self.unlock_vault_keys(&id_string).await?;
            self.apply_organization_access_with_owner(
                &id_string,
                &target,
                access,
                verified_owner.as_ref(),
            )
            .await?;
            info
        } else {
            let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
            let authorized =
                authorize_envelope(identity, &target, &access, verified_owner.as_ref(), false)?;
            let (master, header, members) = self
                .fetch_organization_key(&target, &access, authorized)
                .await?;
            let libsql::Value::Text(owner_wrap) = &header[6] else {
                return Err(organization_error("Invalid organization vault header"));
            };
            private_fs::private_dir(&dir)?;
            Self::write_bootstrap_header(&path, &header, &members).await?;
            let info = self
                .open_vault(&id_string, Some(&config), Some(&account.username))
                .await?;
            self.vaults
                .get_mut(&id_string)
                .ok_or(VaultError::Locked)?
                .adopt_organization_key(&access, master, owner_wrap)
                .await?;
            info
        };
        self.save_organization_vault(info.clone(), config, &account.username, managed_grant)
            .await?;
        let target = self
            .sync_targets(Some(&id_string))?
            .pop()
            .ok_or(VaultError::Locked)?;
        sync_engine::sync_with_receipt(&target).await?;
        Ok(info)
    }

    async fn fetch_organization_key(
        &self,
        target: &AccountTarget,
        access: &SharedAccess,
        authorized: Option<Dek>,
    ) -> Result<(Dek, Vec<libsql::Value>, Vec<Vec<libsql::Value>>), VaultError> {
        if !matches!(access.role.as_str(), "owner" | "editor" | "viewer") {
            return Err(organization_error("Invalid shared vault role"));
        }
        let (bytes, _) = target
            .get()
            .await?
            .ok_or_else(|| organization_error("The vault owner has not uploaded a bundle"))?;
        let (header, _) = crate::vault::sync_bundle::read_header(&bytes)?;
        let [
            _,
            libsql::Value::Text(name),
            _,
            libsql::Value::Text(owner),
            _,
            libsql::Value::Text(kind),
            libsql::Value::Text(wrapped),
        ] = header.as_slice()
        else {
            return Err(organization_error("Invalid organization vault header"));
        };
        if !valid_organization_vault_name(name)
            || !matches!(serde_json::from_str::<VaultType>(kind)?, VaultType::Shared { members } if members.is_empty())
        {
            return Err(organization_error("Invalid organization vault type"));
        }
        let master = match authorized {
            Some(master) => master,
            None if access.key_epoch == 0 && self.user_uuid.as_ref() == Some(owner) => {
                let kek = self.kek.as_ref().ok_or(VaultError::Locked)?;
                unwrap_dek(kek, &serde_json::from_str(wrapped)?)?
            }
            None => {
                return Err(organization_error(
                    "This device does not have the vault owner's identity",
                ));
            }
        };
        let key = sync_record::RecordKey::derive(&master, &target.vault_id.to_string())?;
        let (header, members) = crate::vault::sync_bundle::verify(&key, &bytes)?;
        if !members.is_empty() || (access.key_epoch > 0 && wrapped != "null") {
            return Err(organization_error("Invalid rotated vault header"));
        }
        Ok((master, header, members))
    }

    pub(super) async fn organization_access_for(
        target: &AccountTarget,
    ) -> Result<SharedAccess, VaultError> {
        let org = target.organization.ok_or(VaultError::Locked)?;
        Self::target_access(target, org)
            .await?
            .into_iter()
            .find(|entry| entry.id == target.vault_id)
            .ok_or_else(|| organization_error("This account has no access to the vault"))
    }

    async fn refresh_organization_key(
        &mut self,
        id: &str,
        target: &AccountTarget,
    ) -> Result<(), VaultError> {
        let access = Self::organization_access_for(target).await?;
        self.apply_organization_access(id, target, access).await
    }

    pub(super) async fn apply_organization_access(
        &mut self,
        id: &str,
        target: &AccountTarget,
        access: SharedAccess,
    ) -> Result<(), VaultError> {
        self.apply_organization_access_with_owner(id, target, access, None)
            .await
    }

    async fn apply_organization_access_with_owner(
        &mut self,
        id: &str,
        target: &AccountTarget,
        access: SharedAccess,
        verified_owner: Option<&PublicKey>,
    ) -> Result<(), VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let vault = self.vaults.get(id).ok_or(VaultError::Locked)?;
        let current = vault.organization_key_epoch().await?;
        if access.key_epoch < current {
            return Err(organization_error(
                "The sync server returned an older vault key",
            ));
        }
        if !matches!(access.role.as_str(), "owner" | "editor" | "viewer") {
            return Err(organization_error("Invalid shared vault role"));
        }
        let (local_role, cached) = vault.organization_access().await?;
        let cached = cached.as_deref().map(decode_envelope).transpose()?;
        let cached_owner = match &cached {
            Some(envelope) => peer_key(&envelope.issuer)?,
            // An original owner may have no envelope at epoch zero, even
            // after transferring access and losing the server's owner role.
            None if current == 0 && vault.header.user_uuid == identity.uuid => identity.public_key,
            None => return Err(organization_error("The vault has no trusted owner key")),
        };
        let advancing = access.key_epoch > current;
        let owner = if advancing {
            // A locally cached transfer grant can authorize our own key as
            // issuer after another device of the same identity rotates.
            let transferred = cached
                .as_ref()
                .and_then(|envelope| envelope.scope.as_ref())
                .is_some_and(|scope| scope.epoch == current && scope.role == "owner");
            let incoming_issuer = access
                .wrapped_key
                .as_deref()
                .map(decode_envelope)
                .transpose()?
                .map(|envelope| peer_key(&envelope.issuer))
                .transpose()?;
            verified_owner.unwrap_or_else(|| {
                if transferred
                    && local_role == "owner"
                    && incoming_issuer == Some(identity.public_key)
                {
                    &identity.public_key
                } else {
                    &cached_owner
                }
            })
        } else {
            if verified_owner.is_some_and(|owner| owner != &cached_owner) {
                return Err(organization_error(
                    "The supplied public key does not match the trusted vault owner",
                ));
            }
            &cached_owner
        };
        let authorized = authorize_envelope(identity, target, &access, Some(owner), advancing)?;
        if !advancing {
            if let Some(master) = authorized {
                let local = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;
                use subtle::ConstantTimeEq;
                if !local.with_key(|old| master.with_key(|new| bool::from(old.ct_eq(new)))) {
                    return Err(organization_error(
                        "The vault key changed without a rotation",
                    ));
                }
            } else if cached.is_some() {
                return Err(organization_error(
                    "The server did not return the vault's wrapped key",
                ));
            }
            if access.role == "owner"
                && local_role != "owner"
                && access
                    .wrapped_key
                    .as_deref()
                    .map(decode_envelope)
                    .transpose()?
                    .is_none_or(|envelope| envelope.scope.is_none())
            {
                return Err(VaultError::OrganizationRotationUpgradeRequired);
            }
            let connection = vault.conn()?;
            let tx = connection
                .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
                .await?;
            write_organization_access(&tx, &access.role, access.wrapped_key.as_deref()).await?;
            write_rotation_pending(&tx, access.rotation_required).await?;
            tx.commit().await?;
            return self.remember_organization_grant(id, access.managed_grant()).await;
        }
        let (master, header, _) = self
            .fetch_organization_key(target, &access, authorized)
            .await?;
        let libsql::Value::Text(owner_wrap) = &header[6] else {
            return Err(organization_error("Invalid organization vault header"));
        };
        self.vaults
            .get_mut(id)
            .ok_or(VaultError::Locked)?
            .adopt_organization_key(&access, master, owner_wrap)
            .await?;
        self.remember_organization_grant(id, access.managed_grant()).await
    }

    async fn remember_organization_grant(&mut self, id: &str, grant: Option<Uuid>) -> Result<(), VaultError> {
        let Some(entry) = self.user_vaults.iter_mut().find(|entry| entry.id == id) else {
            return Ok(());
        };
        if entry.managed_grant != grant {
            entry.managed_grant = grant;
            self.save_identity_current().await?;
        }
        Ok(())
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
        &mut self,
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
        self.refresh_organization_key(id, &target).await?;
        match role {
            Some(role @ ("viewer" | "editor" | "owner")) => {
                let vault = self.vaults.get(id).ok_or(VaultError::Locked)?;
                if role == "owner" {
                    let conn = vault.conn()?;
                    if sync_local::state(&conn, ROTATION_PENDING_STATE)
                        .await?
                        .as_deref()
                        == Some("1")
                    {
                        return Err(organization_error(
                            "Finish the pending key rotation before transferring this vault",
                        ));
                    }
                    let sync = self
                        .sync_targets(Some(id))?
                        .pop()
                        .ok_or(VaultError::Locked)?;
                    sync_engine::sync(&sync).await?;
                }
                let master = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;
                let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
                let epoch = vault.organization_key_epoch().await?;
                let key = wrap_for_member(
                    identity,
                    master,
                    public_key,
                    &target,
                    EnvelopeScope {
                        epoch,
                        role: envelope_role(role).into(),
                    },
                )?;
                let proof = seal_recipient_proof(master, &target, &username, public_key)?;
                let body = serde_json::json!({
                    "role": role, "wrapped_key": key, "recipient_proof": proof, "key_epoch": epoch
                });
                let url = target.api(&format!("{}/members/{username}", target.vault_path()));
                let response = target
                    .request(|http| http.put(url.clone()).json(&body))
                    .await?;
                if response.status() != StatusCode::NO_CONTENT {
                    return Err(organization_error("The server refused the vault access change. Check that you own the vault and the recipient belongs to the organization"));
                }
                if role == "owner" {
                    let (_, envelope) = vault.organization_access().await?;
                    vault
                        .save_organization_access("viewer", envelope.as_deref())
                        .await?;
                }
                Ok(())
            }
            None => self.rotate_organization_key(id, &target, &username).await,
            _ => Err(organization_error("Invalid vault sharing role")),
        }
    }

    pub(crate) async fn finish_organization_rotation(
        &mut self,
        id: &str,
    ) -> Result<(), VaultError> {
        let vault = self.vaults.get(id).ok_or(VaultError::Locked)?;
        let target = AccountTarget::parse(
            vault.sync_url.as_deref().ok_or(VaultError::Locked)?,
            vault.auth_token.as_deref().ok_or(VaultError::Locked)?,
        )?;
        if target.organization.is_none() {
            return Err(organization_error(
                "This vault is not an organization vault",
            ));
        }
        self.refresh_organization_key(id, &target).await?;
        self.rotate_organization_key(id, &target, "").await
    }

    async fn rotate_organization_key(
        &mut self,
        id: &str,
        target: &AccountTarget,
        username: &str,
    ) -> Result<(), VaultError> {
        let vault = self.vaults.get(id).ok_or(VaultError::Locked)?;
        let (role, _) = vault.organization_access().await?;
        if role != "owner" {
            return Err(organization_error(
                "Only the vault owner can rotate its key",
            ));
        }
        let url = target.api(&format!("{}/rotation", target.vault_path()));
        let state = target
            .request(|http| http.get(url.clone()))
            .await?
            .error_for_status()
            .map_err(|_| organization_error("The server could not start vault key rotation"))?
            .json::<RotationState>()
            .await
            .map_err(|_| organization_error("Invalid vault rotation state"))?;
        if username.is_empty() && !state.rotation_required {
            return Err(organization_error("This vault has no pending key rotation"));
        }
        let (bytes, revision) = target
            .get()
            .await?
            .ok_or_else(|| organization_error("This vault is unavailable"))?;
        if revision != state.revision || state.key_epoch != vault.organization_key_epoch().await? {
            return Err(organization_error(
                "The vault changed during key rotation. Try again.",
            ));
        }
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let previous = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;
        let next_epoch = state
            .key_epoch
            .checked_add(1)
            .ok_or_else(|| organization_error("The vault key epoch cannot advance"))?;
        let next = generate_dek();
        let mut grants = std::collections::BTreeMap::new();
        for grant in state
            .grants
            .iter()
            .filter(|grant| grant.username != username)
        {
            let public_key = verified_recipient(previous, target, grant)?;
            let wrapped_key = wrap_for_member(
                identity,
                &next,
                &public_key,
                target,
                EnvelopeScope {
                    epoch: next_epoch,
                    role: "member".into(),
                },
            )?;
            let recipient_proof =
                seal_recipient_proof(&next, target, &grant.username, &public_key)?;
            grants.insert(
                &grant.username,
                RotatedGrant {
                    wrapped_key,
                    recipient_proof,
                },
            );
        }
        let self_key = BASE64.encode(identity.public_key.as_bytes());
        let owner_key = wrap_for_member(
            identity,
            &next,
            &self_key,
            target,
            EnvelopeScope {
                epoch: next_epoch,
                role: "owner".into(),
            },
        )?;
        let rotated = crate::vault::sync_bundle::rotate(id, &bytes, previous, &next)?;
        let metadata = serde_json::to_vec(&serde_json::json!({
            "key_epoch": state.key_epoch,
            "acl_version": state.acl_version,
            "revoke": username,
            "complete": username.is_empty(),
            "owner_key": owner_key,
            "grants": grants,
        }))?;
        if metadata.len() > 1 << 20 {
            return Err(organization_error(
                "The vault has too many grants for key rotation",
            ));
        }
        let mut body = Vec::with_capacity(4 + metadata.len() + rotated.len());
        body.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
        body.extend_from_slice(&metadata);
        body.extend(rotated);
        let body = bytes::Bytes::from(body);
        let response = target
            .request(|http| {
                http.put(url.clone())
                    .header(reqwest::header::IF_MATCH, &revision)
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(body.clone())
            })
            .await?;
        match response.status() {
            StatusCode::NO_CONTENT => {}
            StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => {
                return Err(organization_error(
                    "The vault changed during key rotation. Try again.",
                ));
            }
            _ => {
                return Err(organization_error(
                    "The server could not commit vault key rotation",
                ))
            }
        }
        let vault = self.vaults.get_mut(id).ok_or(VaultError::Locked)?;
        let access = SharedAccess {
            id: target.vault_id,
            role: "owner".into(),
            wrapped_key: Some(owner_key),
            grant_id: None,
            key_epoch: next_epoch,
            rotation_required: false,
        };
        vault.adopt_organization_key(&access, next, "null").await?;
        request_sync();
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
    fn recipient_proof_rejects_key_substitution_and_stale_epochs() {
        let target = AccountTarget::parse(
            &format!(
                "{ORG_PREFIX}https://sync.example.com/{}/{}",
                Uuid::new_v4(),
                Uuid::new_v4()
            ),
            "owner",
        )
        .unwrap();
        let key = identity();
        let master = generate_dek();
        let public = BASE64.encode(key.public_key.as_bytes());
        let mut grant = RotationRecipient {
            username: "recipient".into(),
            recipient_proof: seal_recipient_proof(&master, &target, "recipient", &public).unwrap(),
        };
        assert_eq!(
            verified_recipient(&master, &target, &grant).unwrap(),
            public
        );
        assert!(verified_recipient(&generate_dek(), &target, &grant).is_err());
        grant.username = "another-recipient".into();
        assert!(verified_recipient(&master, &target, &grant).is_err());
        grant.username = "recipient".into();
        let mut proof = BASE64.decode(&grant.recipient_proof).unwrap();
        proof[0] ^= 1;
        grant.recipient_proof = BASE64.encode(proof);
        assert!(verified_recipient(&master, &target, &grant).is_err());
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
        let scope = || EnvelopeScope {
            epoch: 2,
            role: "member".into(),
        };
        let issuer = BASE64.encode(recipient.public_key.as_bytes());
        let encoded = wrap_for_member(&owner, &master, &issuer, &target, scope()).unwrap();
        let envelope = decode_envelope(&encoded).unwrap();
        let unwrapped = envelope.open(&recipient, &target).unwrap();
        master.with_key(|expected| unwrapped.with_key(|key| assert_eq!(key, expected)));
        // Older clients ignore the scope field and use the original
        // wrapping context. They cannot decrypt authenticated envelopes.
        let legacy_context = wrapping_key(&recipient, &owner.public_key, &target, None).unwrap();
        assert!(unwrap_dek_with_key(&legacy_context, &envelope.wrapped).is_err());
        assert!(envelope.open(&stranger, &target).is_err());
        assert!(
            envelope
                .open(&recipient, &target_for(org, Uuid::new_v4()))
                .is_err()
        );
        assert!(
            envelope
                .open(&recipient, &target_for(Uuid::new_v4(), id))
                .is_err()
        );
        assert!(
            wrap_for_member(&owner, &master, &BASE64.encode([0; 32]), &target, scope()).is_err()
        );

        let mut access = SharedAccess {
            id,
            role: "editor".into(),
            wrapped_key: Some(encoded),
            grant_id: None,
            key_epoch: 2,
            rotation_required: false,
        };
        assert!(
            authorize_envelope(&recipient, &target, &access, Some(&owner.public_key), true).is_ok()
        );
        assert!(
            authorize_envelope(
                &recipient,
                &target,
                &access,
                Some(&stranger.public_key),
                true
            )
            .is_err()
        );
        assert!(authorize_envelope(&recipient, &target, &access, None, false).is_err());
        access.role = "owner".into();
        assert!(
            authorize_envelope(&recipient, &target, &access, Some(&owner.public_key), true)
                .is_err()
        );
        access.role = "editor".into();
        access.key_epoch = 3;
        assert!(
            authorize_envelope(&recipient, &target, &access, Some(&owner.public_key), true)
                .is_err()
        );

        let mut restamped = envelope;
        restamped.scope.as_mut().unwrap().epoch = access.key_epoch;
        access.wrapped_key = Some(BASE64.encode(serde_json::to_vec(&restamped).unwrap()));
        assert!(
            authorize_envelope(&recipient, &target, &access, Some(&owner.public_key), true).is_err()
        );

        // Previously issued envelopes remain readable at the already trusted
        // epoch, but can't authorize advancing to a new one.
        let legacy_key = wrapping_key(&owner, &recipient.public_key, &target, None).unwrap();
        let legacy = Envelope {
            issuer: BASE64.encode(owner.public_key.as_bytes()),
            wrapped: wrap_dek_with_key(&legacy_key, &master).unwrap(),
            scope: None,
        };
        access.wrapped_key = Some(BASE64.encode(serde_json::to_vec(&legacy).unwrap()));
        assert!(
            authorize_envelope(&recipient, &target, &access, Some(&owner.public_key), false)
                .is_ok()
        );
        assert!(matches!(
            authorize_envelope(&recipient, &target, &access, Some(&owner.public_key), true),
            Err(VaultError::OrganizationRotationUpgradeRequired)
        ));
    }
}
