//! Linking a second device to the same vaults.
//!
//! Every vault key is wrapped for one identity: its user id, X25519 key and
//! the salt its key-encryption key is derived with. A device that set up an
//! identity of its own can reach the same sync database but never open the
//! vault in it, so it made one of its own. A link code carries the first
//! device's identity and sync settings to the second, which then opens the
//! very vaults the first one syncs, as another device of the same user.

use super::*;

/// What a link code starts with, so a plain backup key is told apart.
pub const LINK_PREFIX: &str = "reachercm-link1:";
const ACCOUNT_LINK_PREFIX: &str = "reachercm-account-link1:";

#[derive(Serialize, Deserialize)]
struct DeviceLink {
    user_uuid: String,
    salt: String,
    public_key: String,
    secret_key: String,
    /// The master password's copy of the key, so the same password opens
    /// the linked device too. Empty when no password is set.
    #[serde(default)]
    encrypted_key: String,
    #[serde(default)]
    nonce: String,
    personal_sync_url: Option<String>,
    personal_sync_token: Option<String>,
    #[serde(default)]
    internal_vault_ids: HashMap<String, String>,
    /// Only the vaults that sync: a local one has nothing to open elsewhere.
    #[serde(default)]
    user_vaults: Vec<StoredVaultRef>,
}

/// Whether `text` is a link code rather than a plain backup key.
pub fn is_link_code(text: &str) -> bool {
    text.trim().starts_with(LINK_PREFIX) || text.trim().starts_with(ACCOUNT_LINK_PREFIX)
}

fn parse(code: &str) -> Result<DeviceLink, VaultError> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let body = code
        .trim()
        .strip_prefix(LINK_PREFIX)
        .or_else(|| code.trim().strip_prefix(ACCOUNT_LINK_PREFIX))
        .ok_or_else(|| VaultError::SerializationError("This is not a device link code".into()))?;
    // A code pasted from a chat may have been wrapped across lines.
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let json = URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| VaultError::SerializationError("The device link code is incomplete or damaged".into()))?;
    serde_json::from_slice(&json)
        .map_err(|_| VaultError::SerializationError("The device link code is incomplete or damaged".into()))
}

impl VaultManager {
    /// A code that links another device to this identity and the vaults it
    /// syncs. It holds the identity's secret key and the sync tokens, so it
    /// is as sensitive as the backup key.
    pub async fn export_device_link(&self) -> Result<String, VaultError> {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let user_uuid = self.user_uuid.clone().ok_or(VaultError::Locked)?;
        let data = tokio::fs::read_to_string(self.app_dir.join("vault_identity.json")).await?;
        let stored: StoredIdentity = serde_json::from_str(&data)?;

        let account = self.personal_sync_url.as_deref().is_some_and(|url| url.starts_with("reachercm-sync:"));
        let user_vaults: Vec<StoredVaultRef> = self
            .user_vaults
            .iter()
            .filter(|v| !account && v.sync_url.is_some() && v.sync_token.is_some())
            .cloned()
            .collect();
        let has_personal = self.personal_sync_url.is_some() && self.personal_sync_token.is_some();
        if !has_personal && user_vaults.is_empty() {
            return Err(VaultError::SyncError(
                "Nothing syncs yet. Turn on sync on this device first, then link the other one.".into(),
            ));
        }
        // Only the synced personal vault exists anywhere but here.
        let internal_vault_ids = match (has_personal, self.internal_vault_ids.get("__personal__")) {
            (true, Some(id)) => self
                .internal_vault_ids
                .iter()
                .filter(|(_, v)| *v == id)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            _ => HashMap::new(),
        };

        let link = DeviceLink {
            user_uuid,
            salt: stored.salt,
            public_key: BASE64.encode(identity.public_key.as_bytes()),
            secret_key: identity.with_secret(|s| BASE64.encode(s.as_bytes())),
            encrypted_key: stored.encrypted_key,
            nonce: stored.nonce,
            personal_sync_url: self.personal_sync_url.clone().filter(|_| has_personal),
            personal_sync_token: self.personal_sync_token.clone().filter(|_| has_personal),
            internal_vault_ids,
            user_vaults,
        };
        let json = zeroize::Zeroizing::new(serde_json::to_vec(&link)?);
        let prefix = if account { ACCOUNT_LINK_PREFIX } else { LINK_PREFIX };
        Ok(format!("{}{}", prefix, URL_SAFE_NO_PAD.encode(&*json)))
    }

    /// Make this device another device of the identity in `code` and open
    /// the vaults it syncs. Whatever identity this device had is replaced:
    /// its key stays in the OS keychain and its files stay on disk, but it
    /// no longer opens.
    pub async fn link_device(&mut self, code: &str) -> Result<String, VaultError> {
        let account = code.trim().starts_with(ACCOUNT_LINK_PREFIX);
        let link = parse(code)?;
        let secret = zeroize::Zeroizing::new(
            BASE64
                .decode(&link.secret_key)
                .map_err(|e| VaultError::SerializationError(e.to_string()))?,
        );
        if secret.len() != 32 {
            return Err(VaultError::InvalidKeyLength { expected: 32, got: secret.len() });
        }
        let mut sk = zeroize::Zeroizing::new([0u8; 32]);
        sk.copy_from_slice(&secret);
        let public = PublicKey::from(&StaticSecret::from(*sk));
        if BASE64.encode(public.as_bytes()) != link.public_key {
            return Err(VaultError::SerializationError("The device link code is incomplete or damaged".into()));
        }
        if let Some(url) = link.personal_sync_url.as_deref() {
            crate::vault::sync::check_sync_url(url)?;
        }

        if link.personal_sync_url.as_deref().is_some_and(|url| url.starts_with("reachercm-sync:")) != account {
            return Err(VaultError::SerializationError("Invalid Reacher CM Sync link".into()));
        }
        let path = self.app_dir.join("vault_identity.json");
        let prior = if account {
            #[cfg(not(desktop))]
            return Err(VaultError::SyncError("Reacher CM Sync vaults require the desktop app".into()));
            #[cfg(desktop)]
            {
                self.prepare_account_link(&link, &sk).await?;
                if path.exists() {
                    let data = tokio::fs::read(&path).await?;
                    let backup = self.app_dir.join(format!("vault_identity.before-sync-link-{}.json", uuid::Uuid::new_v4()));
                    private_fs::write_async(&backup, data.clone()).await?;
                    Some(data)
                } else {
                    None
                }
            }
        } else {
            None
        };

        if let Err(error) = store_key_in_keychain(&link.user_uuid, &sk[..]) {
            if account && link.encrypted_key.is_empty() {
                return Err(error);
            }
            tracing::warn!("Linked without the OS keychain ({error}); the master password opens it");
        }

        // Forget the identity this device had.
        self.vaults.clear();
        self.vault_names.clear();
        self.kek = None;
        self.identity = None;
        self.user_uuid = None;
        self.identity_public_key = None;
        self.personal_sync_url = None;
        self.personal_sync_token = None;
        self.internal_vault_ids.clear();
        self.user_vaults.clear();
        self.device_id = None;

        // The file never holds a token in the clear: it is written without
        // them, and the unlock below seals them into it.
        let stored = StoredIdentity {
            user_uuid: link.user_uuid.clone(),
            salt: link.salt,
            encrypted_key: link.encrypted_key,
            nonce: link.nonce,
            public_key: link.public_key,
            personal_sync_url: link.personal_sync_url,
            personal_sync_token: None,
            internal_vault_ids: link.internal_vault_ids,
            user_vaults: link.user_vaults.iter().map(|v| StoredVaultRef { sync_token: None, ..v.clone() }).collect(),
            sealed_tokens: None,
            // This is another device of the same user.
            device_id: None,
        };
        private_fs::private_dir(&self.app_dir)?;
        private_fs::write_async(&path, serde_json::to_string_pretty(&stored)?.into_bytes()).await?;

        let with_tokens = StoredIdentity {
            personal_sync_token: link.personal_sync_token,
            user_vaults: link.user_vaults,
            ..stored
        };
        let opened = async {
            self.open_with_secret_key(with_tokens, &sk[..]).await?;
            if account {
                let id = self.internal_vault_ids.get("__personal__").ok_or(VaultError::Locked)?;
                let target = self.sync_targets(Some(id))?.into_iter().next().ok_or(VaultError::Locked)?;
                sync_engine::sync(&target).await?;
            }
            Ok::<_, VaultError>(())
        }.await;
        if let Err(error) = opened {
            if account {
                if let Some(prior) = prior {
                    private_fs::write_async(&path, prior).await?;
                } else {
                    tokio::fs::remove_file(&path).await?;
                }
                // Force a normal unlock of the restored identity.
                self.vaults.clear();
                self.vault_names.clear();
                self.user_vaults.clear();
                self.internal_vault_ids.clear();
                self.personal_sync_url = None;
                self.personal_sync_token = None;
                self.user_uuid = None;
                self.identity_public_key = None;
                self.device_id = None;
                self.lock();
            }
            return Err(error);
        }
        self.held = false;
        Ok(link.user_uuid)
    }
}

#[cfg(desktop)]
impl VaultManager {
    async fn prepare_account_link(&self, link: &DeviceLink, secret: &[u8; 32]) -> Result<(), VaultError> {
        use crate::vault::sync_account_store::AccountTarget;
        let invalid = || VaultError::SerializationError("Invalid Reacher CM Sync vault link".into());
        if !link.user_vaults.is_empty() {
            return Err(invalid());
        }
        let target = AccountTarget::parse(
            link.personal_sync_url.as_deref().ok_or_else(invalid)?,
            link.personal_sync_token.as_deref().ok_or_else(invalid)?,
        )?;
        let id = target.vault_id.to_string();
        if link.internal_vault_ids.get("__personal__") != Some(&id)
            || link.internal_vault_ids.iter().any(|(name, value)| {
                (name != "__personal__" && !INTERNAL_VAULTS.contains(&name.as_str())) || value != &id
            })
        {
            return Err(invalid());
        }
        let salt: [u8; 32] = BASE64.decode(&link.salt).map_err(|_| invalid())?
            .try_into().map_err(|_| invalid())?;
        let (data, _) = target.get().await?.ok_or_else(|| VaultError::NotFound("personal vault on sync server".into()))?;
        let (header, _) = crate::vault::sync_bundle::read_header(&data)?;
        let (libsql::Value::Text(header_id), libsql::Value::Text(name), libsql::Value::Text(owner),
            libsql::Value::Text(kind), libsql::Value::Text(wrapped)) =
            (&header[0], &header[1], &header[3], &header[5], &header[6]) else { return Err(invalid()); };
        if header_id != &id || name != "__personal__" || owner != &link.user_uuid
            || !matches!(serde_json::from_str::<VaultType>(kind)?, VaultType::Private)
        {
            return Err(invalid());
        }
        let kek = derive_kek_from_secret_key(secret, &salt)?;
        let master = unwrap_dek(&kek, &serde_json::from_str::<WrappedDek>(wrapped)?)?;
        let key = sync_record::RecordKey::derive(&master, &id)?;
        let (header, members) = crate::vault::sync_bundle::verify(&key, &data)?;
        let dir = self.app_dir.join("vaults");
        private_fs::private_dir(&dir)?;
        let db = dir.join(format!("{id}.db"));
        if !db.exists() {
            Self::write_bootstrap_header(&db, &header, &members).await?;
        }
        Ok(())
    }
}
