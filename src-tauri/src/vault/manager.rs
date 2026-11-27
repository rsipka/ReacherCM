use std::collections::HashMap;
use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hkdf::Hkdf;
use libsql::{Connection, Database};
use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::vault::crypto::{
    decrypt_secret, encrypt_secret, generate_dek, unwrap_dek, wrap_dek, wrap_dek_with_key, unwrap_dek_with_key,
};
use crate::vault::error::{describe_db_error, VaultError};
use crate::vault::{biometric, private_fs, sync_engine, sync_local, sync_record, sync_tokens};
use crate::vault::schema::init_schema;
use crate::vault::sync::{create_replica, SyncConfig};
use crate::vault::types::{
    AppSettings, Dek, EncryptedPayload, InviteInfo, Kek, MemberInfo, MemberRole, ReceivedShare,
    SecretCategory, SecretMetadata, ShareItemResult, SharedItemInfo, SyncStatus, UserIdentity,
    VaultHeader, VaultInfo, VaultType, WrappedDek,
};

/// Row id of a secret copied from an internal vault into the unified personal
/// vault. The copy keeps its original id inside the plaintext; callers that
/// address it by that id also have to try this row id.
pub fn unified_secret_id(vault_name: &str, original_id: &str) -> String {
    format!("{vault_name}_{original_id}")
}

/// Internal vault names for encrypted app data.
pub const SESSIONS_VAULT: &str = "__sessions__";
pub const CREDENTIALS_VAULT: &str = "__credentials__";
pub const FOLDERS_VAULT: &str = "__folders__";
pub const PLAYBOOKS_VAULT: &str = "__playbooks__";
pub const SETTINGS_VAULT: &str = "__settings__";
pub const TOFU_PROJECTS_VAULT: &str = "__tofu_projects__";
pub const ANSIBLE_PROJECTS_VAULT: &str = "__ansible_projects__";
pub const SNIPPETS_VAULT: &str = "__snippets__";
pub const SSH_KEYS_VAULT: &str = "__ssh_keys__";
pub const DATABASES_VAULT: &str = "__databases__";
/// Saved container hosts and Kubernetes clusters (kubeconfigs).
pub const DEVOPS_VAULT: &str = "__devops__";
/// WireGuard profiles (src/wireguard), private keys included.
pub const WIREGUARD_VAULT: &str = "__wireguard__";

/// Every vault Reach keeps for itself. They are opened together, migrate
/// together and map together onto the unified vault when personal sync is on,
/// so the list lives in one place — a vault missing from one of those sites is
/// a vault whose data silently stops syncing.
pub const INTERNAL_VAULTS: [&str; 12] = [
    SESSIONS_VAULT,
    CREDENTIALS_VAULT,
    FOLDERS_VAULT,
    PLAYBOOKS_VAULT,
    SETTINGS_VAULT,
    TOFU_PROJECTS_VAULT,
    ANSIBLE_PROJECTS_VAULT,
    SNIPPETS_VAULT,
    SSH_KEYS_VAULT,
    DATABASES_VAULT,
    DEVOPS_VAULT,
    WIREGUARD_VAULT,
];

/// The id a secret had in its own internal vault, if `stored` is the
/// `{vault}_{id}` it was copied into the personal vault under (see
/// `VaultManager::restore_copied_ids`).
fn copied_id(stored: &str) -> Option<&str> {
    INTERNAL_VAULTS
        .iter()
        .find_map(|vault| stored.strip_prefix(vault)?.strip_prefix('_'))
        .filter(|id| !id.is_empty())
}

/// How long a remote connection may sit unused before Reach stops trusting
/// its stream. Turso expires a hrana stream that has been idle; a connection
/// in constant use is never at risk, so this only has to be shorter than the
/// server's patience, not short in absolute terms.
const STREAM_IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long any connection may be used at all, however busy it is.
///
/// Idleness is not the only way a connection dies, and remote ones are not
/// the only ones that die. A network blip, a token refresh or a server
/// restart kills a remote connection mid-use; an I/O error — a laptop waking
/// up, a synced or network folder, a disk hiccup — can leave a local SQLite
/// handle failing every query when a fresh one would work. Neither is
/// idleness, so an idle rule would never replace either: steady use keeps
/// refreshing the idle clock, and someone hitting retry every second would
/// pin the broken connection for as long as they kept trying. That is the
/// behaviour this whole change exists to remove, so the bound on age applies
/// to local and remote alike. A connection that has gone bad for any reason
/// is replaced within a minute, without restarting Reach.
const STREAM_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether a cached connection should be thrown away before the next
/// operation.
///
/// Only a remote connection can expire from sitting still, so only a remote
/// one is retired for being idle. Age applies to both: a local handle cannot
/// time out, but it can still be broken in a way that outlives every retry,
/// and nothing else would ever replace it.
fn should_reconnect(
    is_remote: bool,
    idle_for: std::time::Duration,
    age: std::time::Duration,
) -> bool {
    (is_remote && idle_for > STREAM_IDLE_GRACE) || age > STREAM_MAX_AGE
}

/// The connection a vault is currently using, when it was last used, when it
/// was opened, and whether an operation has already found it broken.
struct CachedConnection {
    conn: Connection,
    last_used: std::time::Instant,
    opened: std::time::Instant,
    /// Set when an operation failed on this connection. The next caller
    /// replaces it rather than inheriting the fault.
    retired: bool,
}

/// Vault connection state.
pub struct VaultConnection {
    pub db: Database,
    conn: std::sync::Mutex<CachedConnection>,
    pub header: VaultHeader,
    pub master_dek: Option<Dek>,
    pub sync_url: Option<String>,
    pub auth_token: Option<String>,
}

pub(crate) static SYNC_OPERATIONS: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Sync every synced vault, or only `only`, with its backend. The manager is
/// held to gather each target and install organization key changes. Bundle
/// sync runs without holding the manager. Returns the changed vaults and the
/// error of each vault that could not sync.
pub async fn sync_vaults(
    manager: &tokio::sync::Mutex<VaultManager>,
    only: Option<&str>,
    timed: bool,
) -> (Vec<String>, Vec<(String, VaultError)>) {
    let _operation = SYNC_OPERATIONS.write().await;
    let mut changed = Vec::new();
    if let Some((account, policy)) = crate::ipc::sync_account_commands::managed_vault_policy().await {
        changed = manager.lock().await.apply_managed_vault_policy(&account, policy).await;
    }
    let targets = match manager.lock().await.sync_targets(only) {
        Ok(t) => t,
        Err(e) => return (changed, vec![(only.unwrap_or_default().to_string(), e)]),
    };
    let mut failed = Vec::new();
    for target in targets {
        if timed && !sync_engine::due(&target).await {
            continue;
        }
        #[cfg(desktop)]
        let target = if let sync_engine::Backend::Account(account) = &target.backend {
            if account.organization.is_some() {
                let updated = async {
                    let access = VaultManager::organization_access_for(account).await?;
                    let mut manager = manager.lock().await;
                    manager.apply_organization_access(&target.vault_id, account, access).await?;
                    manager.sync_targets(Some(&target.vault_id))?.pop().ok_or(VaultError::Locked)
                }.await;
                match updated {
                    Ok(updated) => updated,
                    Err(err) => {
                        if let Some(vault) = manager.lock().await.vaults.get(&target.vault_id) {
                            if let Ok(conn) = vault.conn() {
                                let _ = sync_local::set_state(&conn, sync_engine::STATE_LAST_ERROR, &err.to_string()).await;
                            }
                        }
                        failed.push((target.vault_id.clone(), err));
                        continue;
                    }
                }
            } else {
                target
            }
        } else {
            target
        };
        match sync_engine::sync_with_receipt(&target).await {
            Ok(report) => {
                tracing::debug!(vault = %target.vault_id, ?report, "Vault synced");
                if report.changed() {
                    changed.push(target.vault_id.clone());
                }
            }
            Err(e) => {
                tracing::warn!(vault = %target.vault_id, "Vault did not sync: {}", e);
                failed.push((target.vault_id.clone(), e));
            }
        }
    }
    (changed, failed)
}

/// Move a vault on Turso's old format to sealed records, after syncing it.
pub async fn upgrade_sync_format(manager: &tokio::sync::Mutex<VaultManager>, vault_id: &str) -> Result<(), VaultError> {
    let target = manager
        .lock()
        .await
        .sync_targets(Some(vault_id))?
        .pop()
        .ok_or_else(|| VaultError::SyncError("this vault does not sync".into()))?;
    sync_engine::upgrade(&target).await
}

static SYNC_REQUESTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Ask for a sync soon, as after a vault unlocks or a synced vault changes.
/// Several requests before the sync runs count as one.
pub fn request_sync() {
    SYNC_REQUESTED.notify_one();
}

/// Wait until a sync has been asked for.
pub async fn sync_requested() {
    SYNC_REQUESTED.notified().await;
}

/// A shared vault's master key, as a member unwraps it: with the key agreed
/// between this identity and the inviter's public key.
fn unwrap_member_dek(
    identity: &UserIdentity,
    wrapped_master_dek_json: &str,
    inviter_public_key_b64: &str,
) -> Result<Dek, VaultError> {
    let inviter_pk_bytes = BASE64
        .decode(inviter_public_key_b64)
        .map_err(|e| VaultError::SerializationError(format!("Invalid inviter public key: {}", e)))?;
    let inviter_pk: [u8; 32] = inviter_pk_bytes
        .try_into()
        .map_err(|b: Vec<u8>| VaultError::InvalidKeyLength { expected: 32, got: b.len() })?;
    let shared_secret =
        identity.with_secret(|secret| secret.diffie_hellman(&x25519_dalek::PublicKey::from(inviter_pk)));

    // The same wrapping key the inviter derived.
    let mut wrapping_key = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, shared_secret.as_bytes())
        .expand(b"vault-member-dek", &mut wrapping_key)
        .map_err(|_| VaultError::CryptoError("HKDF expand failed".to_string()))?;

    let wrapped_dek: WrappedDek = serde_json::from_str(wrapped_master_dek_json)?;
    unwrap_dek_with_key(&wrapping_key, &wrapped_dek)
}

/// Decrypt one secret as the `secrets` table stores it: its nonce, its
/// ciphertext, and its data key wrapped by the vault's master key, as JSON.
fn decrypt_stored(
    master_dek: &Dek,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    wrapped_dek_json: String,
) -> Result<SecretBox<Vec<u8>>, VaultError> {
    let nonce: [u8; 24] = nonce.try_into().map_err(|n: Vec<u8>| VaultError::InvalidNonceLength {
        expected: 24,
        got: n.len(),
    })?;
    let wrapped_dek: WrappedDek = serde_json::from_str(&wrapped_dek_json)?;
    decrypt_secret(master_dek, &EncryptedPayload { nonce, ciphertext, wrapped_dek })
}

/// How long one request to Turso may take before it counts as failed. libsql's
/// remote client has no timeout of its own, so a request that is never
/// answered (a connection a firewall or proxy silently drops, a stalled TLS
/// handshake) waited forever, with the vault lock held: the session list sat
/// on "Loading sessions..." for good (issue #77). Turso answers in well under
/// a second, so this only ever ends a request that was never coming back.
const REMOTE_TIMEOUT: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_secs(2)
} else {
    std::time::Duration::from_secs(10)
};

/// Run one libsql operation; for a remote (Turso) vault, no longer than
/// [`REMOTE_TIMEOUT`]. A local database is never cut short.
async fn bounded<T>(
    remote: bool,
    op: impl std::future::Future<Output = Result<T, libsql::Error>>,
) -> Result<T, VaultError> {
    if !remote {
        return op.await.map_err(VaultError::from);
    }
    match tokio::time::timeout(REMOTE_TIMEOUT, op).await {
        Ok(result) => result.map_err(VaultError::from),
        Err(_) => Err(VaultError::SyncError(format!(
            "Turso did not answer within {} seconds",
            REMOTE_TIMEOUT.as_secs()
        ))),
    }
}

/// A connection to a vault's local file that waits for the sync engine,
/// which writes the same file from a connection of its own, rather than
/// failing at once while it does.
fn local_connect(db: &Database) -> Result<Connection, VaultError> {
    let conn = db.connect().map_err(|e| VaultError::DatabaseError(e.to_string()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| VaultError::DatabaseError(e.to_string()))?;
    Ok(conn)
}

impl VaultConnection {
    /// A connection that will actually answer right now.
    ///
    /// A remote vault talks to Turso over a hrana stream identified by a baton
    /// the connection carries. The server drops a stream that has been idle,
    /// and the reply to the next request is an HTTP error — which libsql
    /// returns before it reaches the code that would clear the baton, so the
    /// dead baton is kept and every later request fails the same way. That is
    /// why Reach would stop loading anything, from the server *or* the cache,
    /// until it was closed and reopened.
    ///
    /// So a remote connection is retired once it has been idle long enough to
    /// be in doubt, or once it is simply old — see [`STREAM_MAX_AGE`] for why
    /// idleness alone is not enough. A new one, with a new stream, takes its
    /// place. Back-to-back work reuses one connection and pays nothing; only
    /// the first operation after a pause reconnects. A local database has no
    /// stream to lose, so idleness never troubles it — but it is still
    /// replaced on age, because a broken local handle is just as permanent as
    /// a broken remote one.
    pub fn conn(&self) -> Result<Connection, VaultError> {
        let mut cached = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let now = std::time::Instant::now();
        let stale = cached.retired
            || should_reconnect(
                false,
                cached.last_used.elapsed(),
                cached.opened.elapsed(),
            );
        if stale {
            cached.conn = local_connect(&self.db)?;
            cached.opened = now;
            cached.retired = false;
        }
        cached.last_used = now;
        Ok(cached.conn.clone())
    }

    /// After a write: a synced vault asks for a sync, so the change leaves
    /// this device soon.
    fn changed(&self) {
        if self.sync_url.is_some() {
            request_sync();
        }
    }

    /// Mark the current connection as not to be used again.
    fn retire(&self) {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retired = true;
    }

    /// Run a query, and if it fails, run it once more on a new connection.
    ///
    /// This is the part that makes a broken connection invisible rather than
    /// merely short-lived. The age and idle rules above are prevention — they
    /// bound how long a fault can last. This is the cure: the first operation
    /// to meet a dead connection throws it away and retries, so the caller
    /// gets its answer instead of an error.
    ///
    /// It is the same bargain every mature pool strikes. sqlx pings a
    /// connection before handing it out (`test_before_acquire`, on by
    /// default); deadpool calls it recycling; bb8 calls it
    /// `test_on_check_out`. A probe costs a round trip on every single
    /// operation, which for a list of secrets read one at a time would double
    /// the work. Retrying costs nothing until something actually breaks.
    ///
    /// **Every statement routed through here must be safe to run twice.**
    /// That holds today because they are all reads, keyed upserts, keyed
    /// updates or keyed deletes — none of them accumulate. A statement that
    /// is not idempotent (`SET n = n + 1`, an unkeyed INSERT) must not use
    /// this path.
    pub async fn query<P>(&self, sql: &str, params: P) -> Result<libsql::Rows, VaultError>
    where
        P: libsql::params::IntoParams + Clone,
    {
        let remote = false; // every vault is a local file now
        match bounded(remote, self.conn()?.query(sql, params.clone())).await {
            Ok(rows) => Ok(rows),
            Err(first) => {
                tracing::warn!("Query failed ({}); retrying on a new connection", first);
                self.retire();
                bounded(remote, self.conn()?.query(sql, params)).await.inspect_err(|again| {
                    // The first error is the one that describes the fault; the
                    // second is what a healthy connection had to say about it.
                    tracing::error!("Retry also failed: {}", again);
                })
            }
        }
    }

    /// Run a statement, with the same retry and the same rule about being
    /// safe to run twice. See [`VaultConnection::query`].
    pub async fn execute<P>(&self, sql: &str, params: P) -> Result<u64, VaultError>
    where
        P: libsql::params::IntoParams + Clone,
    {
        let remote = false;
        match bounded(remote, self.conn()?.execute(sql, params.clone())).await {
            Ok(n) => Ok(n),
            Err(first) => {
                tracing::warn!("Statement failed ({}); retrying on a new connection", first);
                self.retire();
                bounded(remote, self.conn()?.execute(sql, params)).await.inspect_err(|again| {
                    tracing::error!("Retry also failed: {}", again);
                })
            }
        }
    }
}


/// Stored vault reference (for reopening after restart).
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredVaultRef {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) vault_type: String,
    pub(crate) sync_url: Option<String>,
    pub(crate) sync_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed_grant: Option<uuid::Uuid>,
}

/// Whether two sync URLs name the same database. The same Turso database
/// can be written `libsql://` or `https://`, and with or without a trailing
/// slash. A vault with no URL is local and is never the same as another.
pub(crate) fn same_database(a: Option<&str>, b: Option<&str>) -> bool {
    fn norm(url: &str) -> String {
        let url = url.trim().trim_end_matches('/').to_ascii_lowercase();
        for scheme in ["libsql://", "https://", "wss://", "http://", "ws://"] {
            if let Some(rest) = url.strip_prefix(scheme) {
                return rest.to_string();
            }
        }
        url
    }
    match (a, b) {
        (Some(a), Some(b)) if !a.trim().is_empty() => norm(a) == norm(b),
        _ => false,
    }
}

/// Entries that share a database, grouped, in the order they were stored.
/// Only groups of two or more.
pub(crate) fn duplicate_groups(vaults: &[StoredVaultRef]) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    for (i, v) in vaults.iter().enumerate() {
        if vaults[..i].iter().any(|o| same_database(o.sync_url.as_deref(), v.sync_url.as_deref())) {
            continue; // already in the group of the first one
        }
        let group: Vec<String> = vaults[i..]
            .iter()
            .filter(|o| same_database(o.sync_url.as_deref(), v.sync_url.as_deref()))
            .map(|o| o.id.clone())
            .collect();
        if group.len() > 1 {
            groups.push(group);
        }
    }
    groups
}

/// Stored identity (persisted to disk).
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredIdentity {
    pub(crate) user_uuid: String,
    pub(crate) salt: String,
    pub(crate) encrypted_key: String,
    pub(crate) nonce: String,
    pub(crate) public_key: String,
    #[serde(default)]
    pub(crate) personal_sync_url: Option<String>,
    #[serde(default)]
    pub(crate) personal_sync_token: Option<String>,
    #[serde(default)]
    pub(crate) internal_vault_ids: HashMap<String, String>,
    /// User-created vaults (shared, private) - persisted for reopening after restart
    #[serde(default)]
    pub(crate) user_vaults: Vec<StoredVaultRef>,
    /// Every sync token above, sealed under the identity key (see
    /// `sync_tokens`). Written in place of the plain token fields, which an
    /// older version's file may still have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sealed_tokens: Option<String>,
    /// This installation's id in sync records, made on first unlock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) device_id: Option<String>,
}

/// Vault manager - core state machine for encrypted vault operations.
pub struct VaultManager {
    /// Open vault connections: vault_id -> VaultConnection
    vaults: HashMap<String, VaultConnection>,

    /// Vault name -> vault_id mapping for O(1) lookup by name
    vault_names: HashMap<String, String>,

    /// User's derived KEK (from master password or keychain)
    kek: Option<Kek>,

    /// User's X25519 identity keypair
    identity: Option<UserIdentity>,

    /// App data directory for local storage
    app_dir: PathBuf,

    /// User's UUID
    user_uuid: Option<String>,

    /// User's public key bytes
    identity_public_key: Option<[u8; 32]>,

    /// Personal sync URL (for cloud backup of ALL user data)
    personal_sync_url: Option<String>,

    /// Personal sync token
    personal_sync_token: Option<String>,

    /// This installation's id in sync records.
    device_id: Option<String>,

    /// Stored internal vault IDs for persistence
    internal_vault_ids: HashMap<String, String>,

    /// User-created vaults (shared, private) - for reopening after restart
    user_vaults: Vec<StoredVaultRef>,

    vault_policy: managed_access::VaultPolicyState,

    /// Locked by the user, or by auto-lock, rather than never opened. While
    /// set, the silent keychain unlock is refused: opening again takes the
    /// user's own act (their password, the unlock button, or biometrics).
    /// Kept in memory only, so a restart opens as it always has.
    held: bool,
}

impl VaultManager {
    /// Create a new vault manager.
    pub fn new(app_dir: PathBuf) -> Self {
        Self {
            vaults: HashMap::new(),
            vault_names: HashMap::new(),
            kek: None,
            identity: None,
            vault_policy: managed_access::VaultPolicyState::load(&app_dir),
            app_dir,
            user_uuid: None,
            identity_public_key: None,
            personal_sync_url: None,
            personal_sync_token: None,
            device_id: None,
            internal_vault_ids: HashMap::new(),
            user_vaults: Vec::new(),
            held: false,
        }
    }

    // ==================== IDENTITY ====================

    /// Initialize a new identity with password.
    pub async fn init_identity(&mut self, password: &str) -> Result<String, VaultError> {
        let identity_path = self.app_dir.join("vault_identity.json");
        if identity_path.exists() {
            return Err(VaultError::IdentityAlreadyExists);
        }

        tracing::info!("init_identity: starting");

        // Generate X25519 keypair
        let secret_key = StaticSecret::random_from_rng(&mut rand::rng());
        let public_key = PublicKey::from(&secret_key);

        // Generate user UUID
        let user_uuid = uuid::Uuid::new_v4().to_string();

        // Generate salt for KDF
        let salt = generate_salt();

        // Derive KEK from secret key (TLS-style)
        let kek = derive_kek_from_secret_key(secret_key.as_bytes(), &salt)?;
        self.kek = Some(kek);

        // Store identity
        self.identity = Some(UserIdentity::new(user_uuid.clone(), secret_key.clone()));
        self.user_uuid = Some(user_uuid.clone());
        self.identity_public_key = Some(public_key.to_bytes());
        self.device_id = Some(uuid::Uuid::new_v4().to_string());

        // Store secret key in OS keychain
        if let Err(e) = store_key_in_keychain(&user_uuid, secret_key.as_bytes()) {
            tracing::error!(
                "Could not store the vault key in the OS keychain: {}. Auto-unlock will not work on the next launch; your password is the only way back in.",
                e
            );
        }

        // Encrypt secret key with password for backup and persist it —
        // without this, password unlock is impossible (issue #25).
        let password_kek = derive_kek_from_password(password.as_bytes(), &salt)?;
        let (encrypted_key, nonce) = encrypt_with_password(&password_kek, secret_key.as_bytes())?;

        self.save_identity(&salt, Some((encrypted_key, nonce))).await?;
        tracing::info!("init_identity: saved identity file");

        // Create internal vaults
        self.ensure_internal_vaults().await?;
        tracing::info!("init_identity: created internal vaults");

        // Migrate old sessions.json if exists
        self.migrate_legacy_data().await?;
        tracing::info!("init_identity: complete");

        Ok(user_uuid)
    }

    /// Unlock vault with password.
    pub async fn unlock(&mut self, password: &str) -> Result<bool, VaultError> {
        let opened = self.unlock_with_password(password).await?;
        if opened {
            self.held = false;
        }
        Ok(opened)
    }

    async fn unlock_with_password(&mut self, password: &str) -> Result<bool, VaultError> {
        let identity_path = self.app_dir.join("vault_identity.json");
        if !identity_path.exists() {
            return Err(VaultError::IdentityNotInitialized);
        }

        let data = tokio::fs::read_to_string(&identity_path).await?;
        let stored: StoredIdentity = serde_json::from_str(&data)?;

        // Decode salt
        let salt_bytes = BASE64
            .decode(&stored.salt)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;
        if salt_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: salt_bytes.len(),
            });
        }
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_bytes);

        // Decrypt secret key with password
        if stored.encrypted_key.is_empty() || stored.nonce.is_empty() {
            return Err(VaultError::PasswordNotSet);
        }
        let encrypted_key = BASE64
            .decode(&stored.encrypted_key)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;
        let nonce = BASE64
            .decode(&stored.nonce)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;

        if nonce.len() != 24 {
            return Err(VaultError::InvalidNonceLength {
                expected: 24,
                got: nonce.len(),
            });
        }

        let password_kek = derive_kek_from_password(password.as_bytes(), &salt)?;
        let secret_key_bytes =
            decrypt_with_password(&password_kek, &encrypted_key, &nonce)?;

        if secret_key_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: secret_key_bytes.len(),
            });
        }

        let mut sk_array = [0u8; 32];
        sk_array.copy_from_slice(&secret_key_bytes);
        let secret_key = StaticSecret::from(sk_array);
        let public_key = PublicKey::from(&secret_key);

        // Derive KEK from secret key
        let kek = derive_kek_from_secret_key(secret_key.as_bytes(), &salt)?;
        self.kek = Some(kek);

        self.identity = Some(UserIdentity::new(stored.user_uuid.clone(), secret_key.clone()));
        self.user_uuid = Some(stored.user_uuid.clone());
        self.identity_public_key = Some(public_key.to_bytes());

        // Store in keychain for auto-unlock, unless a device method guards the key
        if self.unlockers().is_some() {
            tracing::debug!("Device unlock is on; not putting the vault key in the keychain");
        } else if let Err(e) = store_key_in_keychain(&stored.user_uuid, secret_key.as_bytes()) {
            tracing::error!(
                "Could not store the vault key in the OS keychain: {}. Auto-unlock will not work on the next launch.",
                e
            );
        }

        // Load personal sync config, stored internal vault IDs and
        // user-created vaults; their tokens are sealed in the file.
        self.personal_sync_url = stored.personal_sync_url;
        self.internal_vault_ids = stored.internal_vault_ids;
        let plain_tokens = self.restore_sync_tokens(
            stored.personal_sync_token,
            stored.user_vaults,
            stored.sealed_tokens.as_deref(),
        );
        self.harden_app_dir();
        let new_device = stored.device_id.is_none();
        self.device_id = Some(stored.device_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()));
        if plain_tokens || new_device {
            // A file from before the tokens were sealed, or without a device
            // id yet: write it as it should be now.
            match self.save_identity_current().await {
                Ok(()) => tracing::info!("Identity file brought up to date"),
                Err(e) => tracing::warn!("Could not seal the sync tokens yet: {}", e),
            }
        }

        // Open and unlock internal vaults (will use personal sync if configured)
        self.ensure_internal_vaults().await?;

        // Reopen user-created vaults (shared, private)
        self.reopen_user_vaults().await?;

        Ok(true)
    }

    /// Change the password that protects the identity's secret key.
    ///
    /// Requires an unlocked manager. The salt is intentionally kept: it also
    /// feeds the secret-key-derived KEK that encrypts every vault, so a new
    /// salt would invalidate all existing vault data. Only the
    /// password-encrypted copy of the secret key is re-encrypted.
    ///
    /// This is also the recovery path for identities created before the fix
    /// for issue #25, where the password-encrypted key was never persisted:
    /// unlock via OS keychain, then set a password here.
    pub async fn change_password(&mut self, new_password: &str) -> Result<(), VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let secret_key_bytes = zeroize::Zeroizing::new(identity.with_secret(|secret| secret.to_bytes()));

        let identity_path = self.app_dir.join("vault_identity.json");
        if !identity_path.exists() {
            return Err(VaultError::IdentityNotInitialized);
        }
        let data = tokio::fs::read_to_string(&identity_path).await?;
        let stored: StoredIdentity = serde_json::from_str(&data)?;

        let salt_bytes = BASE64
            .decode(&stored.salt)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;
        if salt_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: salt_bytes.len(),
            });
        }
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_bytes);

        let password_kek = derive_kek_from_password(new_password.as_bytes(), &salt)?;
        let (encrypted_key, nonce) =
            encrypt_with_password(&password_kek, &secret_key_bytes[..])?;

        self.save_identity(&salt, Some((encrypted_key, nonce))).await?;
        tracing::info!("change_password: identity re-encrypted with new password");
        Ok(())
    }

    /// Auto-unlock using OS keychain (TLS-style, no password needed).
    pub async fn auto_unlock(&mut self) -> Result<bool, VaultError> {
        if self.held || self.unlockers().is_some() {
            return Ok(false);
        }
        let identity_path = self.app_dir.join("vault_identity.json");
        if !identity_path.exists() {
            return Err(VaultError::IdentityNotInitialized);
        }

        let data = tokio::fs::read_to_string(&identity_path).await?;
        let stored: StoredIdentity = serde_json::from_str(&data)?;

        // Get secret key from keychain
        match get_key_from_keychain(&stored.user_uuid) {
            Ok(key) => {
                let secret_key_bytes = zeroize::Zeroizing::new(key);
                self.open_with_secret_key(stored, &secret_key_bytes).await
            }
            // The keychain lost the key. On Linux the kernel keyring is
            // emptied by every reboot and expires after a few days, so this
            // is routine there (issue #87). An identity set up without a
            // password keeps its key in the identity file under the empty
            // password: open with that and put the key back, instead of
            // leaving every session behind a lock nobody can open. With a
            // real password this fails and the lock screen asks for it.
            Err(e @ (VaultError::KeychainKeyMissing | VaultError::KeychainUnavailable(_))) => {
                match self.unlock_with_password("").await {
                    Ok(true) => {
                        tracing::warn!("auto_unlock: the vault key was missing from the OS keychain ({e}); opened with the identity's stored key and put it back");
                        Ok(true)
                    }
                    _ => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Unlock with the identity's secret key, however it was obtained: from
    /// the keychain, or released by a biometric check.
    pub async fn unlock_with_secret_key(&mut self, secret_key: &[u8]) -> Result<bool, VaultError> {
        let identity_path = self.app_dir.join("vault_identity.json");
        if !identity_path.exists() {
            return Err(VaultError::IdentityNotInitialized);
        }
        let data = tokio::fs::read_to_string(&identity_path).await?;
        let stored: StoredIdentity = serde_json::from_str(&data)?;
        let opened = self.open_with_secret_key(stored, secret_key).await?;
        if opened {
            self.held = false;
        }
        Ok(opened)
    }

    async fn open_with_secret_key(&mut self, stored: StoredIdentity, secret_key_bytes: &[u8]) -> Result<bool, VaultError> {
        if secret_key_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: secret_key_bytes.len(),
            });
        }

        let mut sk_array = zeroize::Zeroizing::new([0u8; 32]);
        sk_array.copy_from_slice(secret_key_bytes);
        let secret_key = StaticSecret::from(*sk_array);
        let public_key = PublicKey::from(&secret_key);
        // A key from the keychain or a biometric seal must be this identity's.
        // Only a real mismatch fails: an identity saved without its public key
        // still opens as it always has.
        let recorded = BASE64.decode(&stored.public_key).unwrap_or_default();
        if recorded.len() == 32 && recorded != public_key.as_bytes() {
            return Err(VaultError::KeychainError("The stored key does not belong to this vault identity".into()));
        }

        // Decode salt
        let salt_bytes = BASE64
            .decode(&stored.salt)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;
        if salt_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: salt_bytes.len(),
            });
        }
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_bytes);

        // Derive KEK from secret key
        let kek = derive_kek_from_secret_key(secret_key.as_bytes(), &salt)?;
        self.kek = Some(kek);

        self.identity = Some(UserIdentity::new(stored.user_uuid.clone(), secret_key));
        self.user_uuid = Some(stored.user_uuid);
        self.identity_public_key = Some(public_key.to_bytes());

        // Load personal sync config, stored internal vault IDs and
        // user-created vaults; their tokens are sealed in the file.
        self.personal_sync_url = stored.personal_sync_url;
        self.internal_vault_ids = stored.internal_vault_ids;
        let plain_tokens = self.restore_sync_tokens(
            stored.personal_sync_token,
            stored.user_vaults,
            stored.sealed_tokens.as_deref(),
        );
        self.harden_app_dir();
        let new_device = stored.device_id.is_none();
        self.device_id = Some(stored.device_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()));
        if plain_tokens || new_device {
            // A file from before the tokens were sealed, or without a device
            // id yet: write it as it should be now.
            match self.save_identity_current().await {
                Ok(()) => tracing::info!("Identity file brought up to date"),
                Err(e) => tracing::warn!("Could not seal the sync tokens yet: {}", e),
            }
        }

        // Open internal vaults
        self.ensure_internal_vaults().await?;

        // Reopen user-created vaults (shared, private)
        self.reopen_user_vaults().await?;

        Ok(true)
    }

    /// Lock the vault manager.
    /// Lock, and keep it locked until the user opens it again: see `held`.
    pub fn hold(&mut self) {
        self.lock();
        self.held = true;
    }

    /// Whether the vault is being kept locked by [`VaultManager::hold`].
    pub fn is_held(&self) -> bool {
        self.held || (self.is_locked() && self.unlockers().is_some())
    }

    /// The app's data folder, where the identity and its files live.
    pub fn app_dir(&self) -> &std::path::Path {
        &self.app_dir
    }

    /// This identity's device unlock methods (Windows Hello, security keys),
    /// if any are on. A file left from another identity (after an import)
    /// does not count.
    pub fn unlockers(&self) -> Option<biometric::Unlockers> {
        let unlockers = biometric::load(&self.app_dir)?;
        let data = std::fs::read(self.app_dir.join("vault_identity.json")).ok()?;
        let stored: StoredIdentity = serde_json::from_slice(&data).ok()?;
        (stored.user_uuid == unlockers.user_uuid && !unlockers.is_empty()).then_some(unlockers)
    }

    /// What adding an unlock method needs, taken while the vault is open: the
    /// methods so far (or a fresh set) and a copy of the identity key. A master
    /// password has to be set, so there is always a way in that does not
    /// depend on the device.
    pub async fn unlock_enrolment(&self) -> Result<(biometric::Unlockers, zeroize::Zeroizing<[u8; 32]>), VaultError> {
        if !self.has_password().await {
            return Err(VaultError::PasswordNotSet);
        }
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let secret = identity.with_secret(|s| zeroize::Zeroizing::new(s.to_bytes()));
        let unlockers = biometric::load(&self.app_dir)
            .filter(|u| u.user_uuid == identity.uuid)
            .unwrap_or_else(|| biometric::Unlockers::new(&identity.uuid));
        Ok((unlockers, secret))
    }

    /// Keep the methods with a new seal added. The keychain copy stays until
    /// a method has proven itself.
    pub fn save_unlockers(&self, unlockers: &biometric::Unlockers) -> Result<(), VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        if unlockers.user_uuid != identity.uuid {
            return Err(VaultError::AccessDenied("unlock methods of another identity".into()));
        }
        biometric::save(&self.app_dir, unlockers)?;
        Ok(())
    }

    /// Open with a key an unlock method released. The first time that works,
    /// the plain keychain copy is no longer needed and is removed.
    pub async fn unlock_with_device(&mut self, secret: &[u8]) -> Result<bool, VaultError> {
        let opened = self.unlock_with_secret_key(secret).await?;
        if let Some(mut unlockers) = self.unlockers().filter(|u| opened && !u.proven) {
            match delete_key_from_keychain(&unlockers.user_uuid) {
                Ok(()) => {
                    unlockers.proven = true;
                    biometric::save(&self.app_dir, &unlockers)?;
                    tracing::info!("Device unlock proven; the keychain copy of the vault key was removed");
                }
                Err(e) => tracing::warn!("Could not remove the keychain copy of the vault key: {}", e),
            }
        }
        Ok(opened)
    }

    /// Remove one unlock method; returns it. Removing the last one puts
    /// the key back in the keychain first, and only then drops the file, so
    /// there is never a moment with neither.
    pub fn remove_unlocker(&self, id: &str) -> Result<Option<biometric::Seal>, VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let Some(mut unlockers) = self.unlockers() else {
            return Ok(None);
        };
        let Some(removed) = unlockers.remove(id) else {
            return Ok(None);
        };
        if unlockers.is_empty() {
            identity.with_secret(|s| store_key_in_keychain(&identity.uuid, s.as_bytes()))?;
            biometric::remove(&self.app_dir)?;
        } else {
            biometric::save(&self.app_dir, &unlockers)?;
        }
        Ok(Some(removed))
    }

    /// The unlock button on the lock screen: lift the hold and open with the
    /// keychain. Only while there is nothing better to ask for: once a master
    /// password is set, a lock asks for it (or biometrics), or anyone at the
    /// computer could undo the lock with one click.
    pub async fn resume(&mut self) -> Result<bool, VaultError> {
        if self.has_password().await {
            return Ok(false);
        }
        self.held = false;
        let opened = self.auto_unlock().await?;
        if !opened {
            self.held = true;
        }
        Ok(opened)
    }

    pub fn lock(&mut self) {
        self.kek = None;
        self.identity = None;
        for vault in self.vaults.values_mut() {
            vault.master_dek = None;
        }
    }

    /// Check if locked.
    pub fn is_locked(&self) -> bool {
        self.kek.is_none()
    }

    /// Check if identity exists.
    pub async fn has_identity(&self) -> bool {
        self.app_dir.join("vault_identity.json").exists()
    }

    /// Whether a password can actually open this identity.
    ///
    /// Distinct from [`has_identity`]: an identity created before the fix for
    /// issue #25 has the password-encrypted copy of the secret key missing,
    /// because it was computed at init and then discarded. Those vaults open
    /// only via the OS keychain, so the day the keychain entry goes away the
    /// data is unreachable — which is what issue #30 reports.
    ///
    /// Reports false for such an identity so the UI can offer to set a
    /// password while the vault is still open, rather than claiming one is
    /// already configured.
    pub async fn has_password(&self) -> bool {
        let path = self.app_dir.join("vault_identity.json");
        let Ok(data) = tokio::fs::read_to_string(&path).await else {
            return false;
        };
        let Ok(stored) = serde_json::from_str::<StoredIdentity>(&data) else {
            return false;
        };
        if stored.encrypted_key.is_empty() || stored.nonce.is_empty() {
            return false;
        }
        // Set up without a password, the key is sealed with an empty one: that
        // is no password. Saying otherwise made a lock ask for one nobody set.
        !Self::opens_with(&stored, "")
    }

    /// Whether `password` is the identity's current password.
    pub async fn check_password(&self, password: &str) -> bool {
        let path = self.app_dir.join("vault_identity.json");
        let Ok(data) = tokio::fs::read_to_string(&path).await else {
            return false;
        };
        serde_json::from_str::<StoredIdentity>(&data).is_ok_and(|stored| Self::opens_with(&stored, password))
    }

    /// Whether `password` opens the key sealed in `stored`.
    fn opens_with(stored: &StoredIdentity, password: &str) -> bool {
        let (Ok(salt), Ok(encrypted_key), Ok(nonce)) = (
            BASE64.decode(&stored.salt),
            BASE64.decode(&stored.encrypted_key),
            BASE64.decode(&stored.nonce),
        ) else {
            return false;
        };
        let Ok(salt) = <[u8; 32]>::try_from(salt.as_slice()) else {
            return false;
        };
        derive_kek_from_password(password.as_bytes(), &salt)
            .and_then(|kek| decrypt_with_password(&kek, &encrypted_key, &nonce))
            .is_ok()
    }

    /// Get public key (base64).
    pub fn get_public_key(&self) -> Option<String> {
        self.identity_public_key.map(|pk| BASE64.encode(pk))
    }

    /// Get user UUID.
    pub fn get_user_uuid(&self) -> Option<String> {
        self.user_uuid.clone()
    }

    /// A key only this identity has (its own devices share it, nobody it
    /// shares a vault with does), derived from the identity's secret for
    /// signing the user's approvals of a session's commands and weaker
    /// settings. `None` while locked.
    pub fn approval_key(&self) -> Option<zeroize::Zeroizing<[u8; 32]>> {
        let identity = self.identity.as_ref()?;
        let secret = zeroize::Zeroizing::new(identity.with_secret(|s| s.to_bytes()));
        let hk = Hkdf::<sha2::Sha256>::new(Some(b"reach ssh option approvals"), &secret[..]);
        let mut key = zeroize::Zeroizing::new([0u8; 32]);
        hk.expand(b"v1", &mut key[..]).ok()?;
        Some(key)
    }

    /// Export identity for backup.
    pub fn export_identity(&self) -> Result<String, VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        Ok(identity.with_secret(|secret| BASE64.encode(secret.as_bytes())))
    }

    /// Reset vault - delete all local data.
    pub async fn reset(&mut self) -> Result<(), VaultError> {
        // The identity's key goes with it: a reset that left the secret key
        // in the OS keychain left the old identity openable. Read from the
        // identity file too, since a locked vault has no user in memory.
        for uuid in self.user_uuid.clone().into_iter().chain(self.stored_user_uuid().await) {
            if let Err(e) = delete_key_from_keychain(&uuid) {
                tracing::warn!("Reset could not remove the vault key from the OS keychain: {e}");
            }
        }

        // Close all vaults
        self.vaults.clear();
        self.vault_names.clear();
        self.kek = None;
        self.identity = None;
        self.user_uuid = None;
        self.identity_public_key = None;
        self.personal_sync_url = None;
        self.personal_sync_token = None;
        self.internal_vault_ids.clear();

        // Delete identity file
        let identity_path = self.app_dir.join("vault_identity.json");
        if identity_path.exists() {
            tokio::fs::remove_file(&identity_path).await?;
        }

        // Delete vaults directory (best-effort on Windows due to file locks)
        let vault_dir = self.app_dir.join("vaults");
        if vault_dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&vault_dir).await {
                tracing::warn!("Could not delete vaults directory (files may be locked): {}", e);
                // Try deleting individual files instead
                if let Ok(mut entries) = tokio::fs::read_dir(&vault_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if let Err(e) = tokio::fs::remove_file(entry.path()).await {
                            tracing::warn!("Could not delete {}: {}", entry.path().display(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    // ==================== PERSONAL SYNC ====================

    /// Set personal sync config (for cloud backup of ALL user data).
    pub async fn set_personal_sync_config(
        &mut self,
        sync_url: Option<String>,
        sync_token: Option<String>,
    ) -> Result<(), VaultError> {
        // Refuse a clear-text URL before anything is closed or migrated.
        if let Some(url) = sync_url.as_deref() {
            crate::vault::sync::check_sync_url(url)?;
        }
        let was_sync_enabled =
            self.personal_sync_url.is_some() && self.personal_sync_token.is_some();

        self.personal_sync_url = sync_url.clone();
        self.personal_sync_token = sync_token.clone();

        // Read existing salt from identity file
        let identity_path = self.app_dir.join("vault_identity.json");
        if identity_path.exists() {
            let data = tokio::fs::read_to_string(&identity_path).await?;
            let stored: StoredIdentity = serde_json::from_str(&data)?;
            let salt_bytes = BASE64
                .decode(&stored.salt)
                .map_err(|e| VaultError::SerializationError(e.to_string()))?;
            if salt_bytes.len() == 32 {
                let mut salt = [0u8; 32];
                salt.copy_from_slice(&salt_bytes);
                self.save_identity(&salt, None).await?;
            }
        }

        // If sync is NOW enabled (wasn't before), migrate local data to cloud
        if !was_sync_enabled && sync_url.is_some() && sync_token.is_some() {
            tracing::info!("Personal sync enabled for first time - migrating local data to cloud");

            // Ensure we're unlocked to decrypt local secrets
            let _ = self.kek.as_ref().ok_or(VaultError::Locked)?;
            let internal_names = INTERNAL_VAULTS;

            // Step 1: Read and decrypt all secrets from local vaults
            // Vault name, id, name, category, plaintext.
            type Copied<'a> = (&'a str, String, String, SecretCategory, SecretBox<Vec<u8>>);
            let mut all_secrets: Vec<Copied> = Vec::new();

            for name in internal_names {
                if let Some(vault_id) = self.vault_names.get(name).cloned() {
                    if let Some(vault) = self.vaults.get(&vault_id) {
                        if let Some(ref master_dek) = vault.master_dek {
                            // Read all secrets from this vault
                            let mut rows = vault
                                
                                .query(
                                    "SELECT id, name, category, nonce, ciphertext, wrapped_dek FROM secrets",
                                    (),
                                )
                                .await?;

                            while let Some(row) = rows.next().await? {
                                let id: String = row.get(0)?;
                                let secret_name: String = row.get(1)?;
                                let category_str: String = row.get(2)?;
                                let nonce: Vec<u8> = row.get(3)?;
                                let ciphertext: Vec<u8> = row.get(4)?;
                                let wrapped_dek_json: String = row.get(5)?;

                                // Decrypt the secret
                                let wrapped_dek: WrappedDek =
                                    serde_json::from_str(&wrapped_dek_json)?;
                                let nonce_arr: [u8; 24] = nonce.try_into().map_err(|_| {
                                    VaultError::InvalidNonceLength {
                                        expected: 24,
                                        got: 0,
                                    }
                                })?;
                                let payload = EncryptedPayload {
                                    nonce: nonce_arr,
                                    ciphertext,
                                    wrapped_dek,
                                };

                                match decrypt_secret(master_dek, &payload) {
                                    Ok(plaintext) => {
                                        let category: SecretCategory = category_str
                                            .parse()
                                            .unwrap_or(SecretCategory::Custom("unknown".to_string()));
                                        all_secrets.push((name, id, secret_name, category, plaintext));
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to decrypt secret {} from {}: {}",
                                            id,
                                            name,
                                            e
                                        );
                                    }
                                }
                            }

                            tracing::info!(
                                "Read {} secrets from local vault {}",
                                all_secrets.len(),
                                name
                            );
                        }
                    }
                }
            }

            // Step 2: Close all local vaults
            let vault_ids: Vec<String> = self.vaults.keys().cloned().collect();
            for vault_id in vault_ids {
                let _ = self.close_vault(&vault_id).await;
            }
            self.vault_names.clear();

            // Step 3: Create the unified vault in the cloud
            tracing::info!(
                "Creating unified personal vault in cloud with {} secrets to migrate",
                all_secrets.len()
            );
            self.ensure_internal_vaults().await?;

            // Step 4: Re-encrypt and store all secrets in unified vault
            let unified_vault_id = self
                .vault_names
                .get(SETTINGS_VAULT)
                .cloned()
                .ok_or_else(|| VaultError::NotFound("Unified vault not created".to_string()))?;

            // Each secret keeps its own id: everything that reads one back
            // asks for it by that id, and a session stored as
            // `__sessions___<id>` still listed but could not be edited or
            // deleted (#148). Only an id that is already taken, by an
            // earlier secret here or by data already in the cloud, is
            // prefixed with its vault, so neither is overwritten.
            let mut taken = std::collections::HashSet::new();
            for (vault_name, id, name, category, plaintext) in all_secrets {
                let free = !taken.contains(&id)
                    && matches!(self.find_secret(&unified_vault_id, &id).await, Ok(false));
                let id = if free { id } else { unified_secret_id(vault_name, &id) };
                taken.insert(id.clone());
                match self
                    .create_secret_with_id(&unified_vault_id, &id, &name, category, plaintext)
                    .await
                {
                    Ok(_) => {
                        tracing::debug!("Migrated secret: {}", id);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to migrate secret {}: {}", id, e);
                    }
                }
            }

            tracing::info!("Migration to cloud complete");
            return Ok(());
        }

        // If sync config changed (but was already enabled), just reopen vaults
        if sync_url.is_some() && sync_token.is_some() {
            // Close existing vaults and reopen with new config
            let vault_ids: Vec<String> = self.vaults.keys().cloned().collect();
            for vault_id in vault_ids {
                let _ = self.close_vault(&vault_id).await;
            }
            self.vault_names.clear();
            self.ensure_internal_vaults().await?;
        }

        Ok(())
    }

    /// Get personal sync config.
    pub fn get_personal_sync_config(&self) -> (Option<String>, Option<String>) {
        (self.personal_sync_url.clone(), self.personal_sync_token.clone())
    }

    /// Ensure internal vaults exist, are open, and can be read.
    async fn ensure_internal_vaults(&mut self) -> Result<(), VaultError> {
        self.open_internal_vaults().await?;
        self.restore_vault_keys().await;
        if self.personal_sync_url.is_some() && self.personal_sync_token.is_some() {
            self.restore_copied_ids().await;
        }
        Ok(())
    }

    /// Turning personal sync on used to copy each internal vault's secrets
    /// into the personal vault as `{vault}_{id}`, where nothing that looks a
    /// secret up by its id finds it: a session that lists but cannot be
    /// edited or deleted (#148), a folder that cannot be renamed, settings
    /// that seem lost. Store each under its own id again. If that id is
    /// already in use, the secret using it is newer and is kept; the copy
    /// stays as it was.
    async fn restore_copied_ids(&self) {
        let Some(vault_id) = self.vault_names.get(SETTINGS_VAULT).cloned() else {
            return;
        };
        let secrets = match self.list_secrets(&vault_id).await {
            Ok(secrets) => secrets,
            Err(e) => {
                tracing::warn!("Could not list the personal vault to restore copied ids: {}", e);
                return;
            }
        };
        for meta in secrets {
            let Some(id) = copied_id(&meta.id) else { continue };
            match self.find_secret(&vault_id, id).await {
                Ok(false) => {}
                Ok(true) => {
                    tracing::warn!("Secret {} is already in use; {} stays as it is", id, meta.id);
                    continue;
                }
                Err(e) => {
                    tracing::warn!("Could not check for secret {}: {}; {} stays as it is", id, e, meta.id);
                    continue;
                }
            }
            let category = meta.category.parse().unwrap_or(SecretCategory::Custom("unknown".to_string()));
            // Written under its own id before the copy is deleted: a failure
            // in between leaves two copies, never none.
            let moved = async {
                let plaintext = self.read_secret(&vault_id, &meta.id).await?;
                self.create_secret_with_id(&vault_id, id, &meta.name, category, plaintext).await?;
                self.delete_secret(&vault_id, &meta.id).await
            }
            .await;
            match moved {
                Ok(()) => tracing::info!("Restored secret {} to its own id", meta.id),
                Err(e) => tracing::warn!("Could not restore secret {} to its own id: {}", meta.id, e),
            }
        }
    }

    /// A lock wipes every vault's key but leaves its connection open, and
    /// opening the vaults again skips one that is already open. Put the key
    /// back in every open vault that lost it, internal or the user's own, or
    /// it stays unreadable (an empty session list) until Reach restarts.
    async fn restore_vault_keys(&mut self) {
        if self.kek.is_none() {
            return;
        }
        let mut ids: Vec<String> = self
            .vaults
            .iter()
            .filter(|(_, v)| v.master_dek.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        for id in ids {
            if let Err(e) = self.unlock_vault(&id).await {
                tracing::error!("Could not unlock vault {} again: {}", id, e);
            }
        }
    }

    /// Open (or create) the internal vaults.
    /// Uses personal sync config if available for cloud backup of ALL user data.
    async fn open_internal_vaults(&mut self) -> Result<(), VaultError> {
        // Get personal sync config (if configured, ALL data syncs to cloud)
        let sync_url = self.personal_sync_url.clone();
        let sync_token = self.personal_sync_token.clone();
        let has_sync = sync_url.is_some() && sync_token.is_some();

        tracing::info!(
            "ensure_internal_vaults: has_sync={}, sync_url={:?}",
            has_sync,
            sync_url.as_ref().map(|_| "[redacted]")
        );

        if has_sync {
            tracing::info!("Personal sync enabled - using single vault for all internal data");

            // With personal sync, ALL internal vaults share ONE vault in the cloud
            // This avoids table conflicts since all data goes to the same database
            const UNIFIED_VAULT_NAME: &str = "__personal__";

            // Check if we already have the unified vault open
            if let Some(vault_id) = self.vault_names.get(UNIFIED_VAULT_NAME).filter(|id| self.vaults.contains_key(*id)).cloned() {
                tracing::info!(
                    "Unified vault {} already open, mapping all internal vaults",
                    vault_id
                );
                // Map all internal vault names to this unified vault
                for name in INTERNAL_VAULTS {
                    self.vault_names.insert(name.to_string(), vault_id.clone());
                }
                // Verify vault is unlocked
                if let Some(vault) = self.vaults.get(&vault_id) {
                    tracing::info!("Unified vault unlocked: {}", vault.master_dek.is_some());
                }
                return Ok(());
            }

            // Check if we have a stored vault ID for the unified vault
            let account_target = sync_url.as_deref().zip(sync_token.as_deref())
                .and_then(|(url, user)| crate::vault::sync_account_store::AccountTarget::parse(url, user).ok());
            let mut vault_id = self.internal_vault_ids.get(UNIFIED_VAULT_NAME).cloned()
                .or_else(|| account_target.as_ref().map(|target| target.vault_id.to_string()));
            tracing::info!("Stored unified vault ID: {:?}", vault_id);

            if let Some(ref id) = vault_id {
                tracing::info!("Opening stored unified vault with id {}", id);
                match self
                    .open_vault(id, sync_url.as_deref(), sync_token.as_deref())
                    .await
                {
                    Ok(info) => {
                        tracing::info!(
                            "Opened unified vault: {} (secrets: {})",
                            info.id,
                            info.secret_count
                        );
                        if !self.vaults.get(id).is_some_and(|vault| self.account_vault_suspended(id, vault)) {
                            self.unlock_vault(id).await?;
                        }
                    }
                    Err(e) if account_target.is_some() => {
                        // A failed account sync must not replace the configured
                        // personal vault or remap local data to a new identity.
                        tracing::warn!("Could not open the configured personal Sync vault: {}", e);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to open stored unified vault: {}, will recreate",
                            e
                        );
                        vault_id = None; // Will create new
                    }
                }
            }

            if vault_id.is_none() {
                // A database another device already set up holds the vault:
                // open that one instead of writing a second header into it.
                if let (Some(url), Some(token)) = (sync_url.as_deref(), sync_token.as_deref()) {
                    if let Some(existing) = self.remote_vault_id(url, token).await {
                        tracing::info!("Personal sync database already holds vault {}; opening it", existing);
                        self.open_vault(&existing, Some(url), Some(token)).await?;
                        self.unlock_vault(&existing).await?;
                        self.internal_vault_ids.insert(UNIFIED_VAULT_NAME.to_string(), existing.clone());
                        self.save_identity_current().await?;
                        vault_id = Some(existing);
                    }
                }
            }

            if vault_id.is_none() {
                // Create the unified vault
                tracing::info!("Creating unified personal vault");
                let vault = self
                    .create_vault(
                        UNIFIED_VAULT_NAME,
                        VaultType::Private,
                        sync_url.as_deref(),
                        sync_token.as_deref(),
                    )
                    .await?;
                tracing::info!("Created unified vault: {}", vault.id);
                self.unlock_vault(&vault.id).await?;
                tracing::info!("Unlocked new unified vault");
                vault_id = Some(vault.id.clone());

                // Save the unified vault ID
                self.internal_vault_ids
                    .insert(UNIFIED_VAULT_NAME.to_string(), vault.id);
                self.save_identity_current().await?;
            }

            // Map all internal vault names to the unified vault
            let unified_id = vault_id.unwrap();
            tracing::info!(
                "Mapping all internal vault names to unified vault: {}",
                unified_id
            );
            for name in INTERNAL_VAULTS {
                self.vault_names.insert(name.to_string(), unified_id.clone());
            }
            tracing::info!(
                "vault_names after mapping: {:?}",
                self.vault_names.keys().collect::<Vec<_>>()
            );

            return Ok(());
        }

        // Local-only mode: each internal vault is separate
        let mut ids_changed = false;

        for name in INTERNAL_VAULTS {
            if self.vault_names.contains_key(name) {
                continue; // Already open
            }

            // First check if we have a stored vault ID for this name
            // (e.g. from a backup import where multiple names map to one unified vault)
            if let Some(stored_id) = self.internal_vault_ids.get(name).cloned() {
                // Check if this vault is already open (shared with another internal name)
                if self.vaults.contains_key(&stored_id) {
                    tracing::info!("Mapping {} to already-open vault {}", name, stored_id);
                    self.vault_names.insert(name.to_string(), stored_id);
                    continue;
                }

                let vault_dir = self.app_dir.join("vaults");
                let db_path = vault_dir.join(format!("{}.db", stored_id));
                if db_path.exists() {
                    tracing::info!("Opening stored internal vault {} -> {}", name, stored_id);
                    self.open_vault(&stored_id, None, None).await?;
                    self.unlock_vault(&stored_id).await?;
                    continue;
                }
            }

            // Try to find in local files by scanning DB headers
            let vault_dir = self.app_dir.join("vaults");
            let mut found_id = None;

            if vault_dir.exists() {
                let mut entries = tokio::fs::read_dir(&vault_dir).await?;
                while let Some(entry) = entries.next_entry().await? {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "db") {
                        if let Some(stem) = path.file_stem() {
                            let vault_id = stem.to_string_lossy().to_string();
                            if let Ok(db) = create_replica(&path, None).await {
                                if let Ok(conn) = db.connect() {
                                    if let Ok(mut rows) =
                                        conn.query("SELECT name FROM vault_header LIMIT 1", ()).await
                                    {
                                        if let Ok(Some(row)) = rows.next().await {
                                            let db_name: String = row.get(0).unwrap_or_default();
                                            if db_name == name {
                                                found_id = Some(vault_id);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(vault_id) = found_id {
                tracing::info!("Found internal vault {} in local files: {}", name, vault_id);
                self.open_vault(&vault_id, None, None).await?;
                self.unlock_vault(&vault_id).await?;
                self.internal_vault_ids.insert(name.to_string(), vault_id);
                ids_changed = true;
            } else {
                // Create new local vault
                tracing::info!("Creating new internal vault: {}", name);
                let vault = self.create_vault(name, VaultType::Private, None, None).await?;
                self.unlock_vault(&vault.id).await?;
                self.internal_vault_ids.insert(name.to_string(), vault.id);
                ids_changed = true;
            }
        }

        if ids_changed {
            self.save_identity_current().await?;
        }

        Ok(())
    }

    /// Reopen user-created vaults (shared, private) after unlock.
    async fn reopen_user_vaults(&mut self) -> Result<(), VaultError> {
        let vaults_to_open = self.user_vaults.clone();

        for vault_ref in vaults_to_open {
            // Skip if already open
            if self.vaults.contains_key(&vault_ref.id) {
                continue;
            }

            tracing::info!("Reopening user vault: {} ({})", vault_ref.name, vault_ref.id);

            match self
                .open_vault(
                    &vault_ref.id,
                    vault_ref.sync_url.as_deref(),
                    vault_ref.sync_token.as_deref(),
                )
                .await
            {
                Ok(_) => {
                    if let Err(e) = self.unlock_vault(&vault_ref.id).await {
                        tracing::warn!("Failed to unlock user vault {}: {}", vault_ref.name, e);
                    } else {
                        tracing::info!("User vault {} reopened and unlocked", vault_ref.name);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to reopen user vault {}: {}", vault_ref.name, e);
                }
            }
        }

        // The ones skipped above as already open need their keys back too.
        self.restore_vault_keys().await;
        Ok(())
    }

    /// Save identity to file.
    /// Save identity. `password_key` carries fresh password-encrypted secret
    /// key material `(encrypted_key_b64, nonce_b64)` when setting or changing
    /// the password; otherwise the previously stored material is preserved.
    async fn save_identity(
        &self,
        salt: &[u8; 32],
        password_key: Option<(String, String)>,
    ) -> Result<(), VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let public_key = identity.public_key;

        // For encrypted_key and nonce, we need the password-encrypted version.
        // Use fresh material when provided; otherwise keep what is on disk.
        let identity_path = self.app_dir.join("vault_identity.json");
        let (encrypted_key, nonce) = if let Some((ek, n)) = password_key {
            (ek, n)
        } else if identity_path.exists() {
            let data = tokio::fs::read_to_string(&identity_path).await?;
            let stored: StoredIdentity = serde_json::from_str(&data)?;
            (stored.encrypted_key, stored.nonce)
        } else {
            // New identity without password material yet.
            (String::new(), String::new())
        };

        let stored = StoredIdentity {
            user_uuid: identity.uuid.clone(),
            salt: BASE64.encode(salt),
            encrypted_key,
            nonce,
            public_key: BASE64.encode(public_key.as_bytes()),
            personal_sync_url: self.personal_sync_url.clone(),
            personal_sync_token: None,
            internal_vault_ids: self.internal_vault_ids.clone(),
            user_vaults: self
                .user_vaults
                .iter()
                .map(|v| StoredVaultRef { sync_token: None, ..v.clone() })
                .collect(),
            sealed_tokens: self.seal_sync_tokens()?,
            device_id: self.device_id.clone(),
        };

        let json = serde_json::to_string_pretty(&stored)?;
        private_fs::private_dir(&self.app_dir)?;
        private_fs::write_async(&identity_path, json.into_bytes()).await?;

        Ok(())
    }

    /// Save identity with current salt.
    /// The identity's secret key, for keys derived from it.
    fn identity_secret(&self) -> Result<zeroize::Zeroizing<[u8; 32]>, VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        Ok(zeroize::Zeroizing::new(identity.with_secret(|s| s.to_bytes())))
    }

    /// Every sync token held in memory, sealed for the identity file; `None`
    /// when there is none.
    fn seal_sync_tokens(&self) -> Result<Option<String>, VaultError> {
        let tokens = sync_tokens::SyncTokens {
            personal: self.personal_sync_token.clone(),
            vaults: self
                .user_vaults
                .iter()
                .filter_map(|v| Some((v.id.clone(), v.sync_token.clone()?)))
                .collect(),
        };
        if tokens.is_empty() {
            return Ok(None);
        }
        let uuid = self.user_uuid.as_deref().ok_or(VaultError::Locked)?;
        Ok(Some(sync_tokens::seal(&*self.identity_secret()?, uuid, &tokens)?))
    }

    /// Put the sync tokens read from the identity file back in memory: plain
    /// ones from an older file as they are, sealed ones opened. Returns
    /// whether the file had any in plain text, so it can be saved sealed.
    fn restore_sync_tokens(
        &mut self,
        plain_personal: Option<String>,
        mut user_vaults: Vec<StoredVaultRef>,
        sealed: Option<&str>,
    ) -> bool {
        let had_plain = plain_personal.is_some() || user_vaults.iter().any(|v| v.sync_token.is_some());
        let mut personal = plain_personal;
        if let Some(sealed) = sealed {
            let opened = self
                .identity_secret()
                .and_then(|secret| {
                    let uuid = self.user_uuid.as_deref().ok_or(VaultError::Locked)?;
                    sync_tokens::open(&secret, uuid, sealed)
                });
            match opened {
                Ok(tokens) => {
                    if personal.is_none() {
                        personal = tokens.personal;
                    }
                    for v in &mut user_vaults {
                        if v.sync_token.is_none() {
                            v.sync_token = tokens.vaults.get(&v.id).cloned();
                        }
                    }
                }
                Err(e) => tracing::error!("The sealed sync tokens could not be opened: {}", e),
            }
        }
        self.personal_sync_token = personal;
        self.user_vaults = user_vaults;
        had_plain
    }

    /// Make the app's data directory, its vaults and their caches readable
    /// by this user only. Best effort: a failure is logged, never fatal.
    fn harden_app_dir(&self) {
        let vault_dir = self.app_dir.join("vaults");
        let mut result = private_fs::private_dir(&self.app_dir)
            .and_then(|()| private_fs::restrict_file(&self.app_dir.join("vault_identity.json")));
        if result.is_ok() && vault_dir.is_dir() {
            result = private_fs::private_dir(&vault_dir).and_then(|()| {
                for entry in std::fs::read_dir(&vault_dir)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|e| e == "cache") {
                        // The encrypted copies synced vaults used to keep;
                        // their local file has taken over.
                        let _ = std::fs::remove_file(&path);
                    } else if path.is_file() {
                        private_fs::restrict_file(&path)?;
                    }
                }
                Ok(())
            });
        }
        if let Err(e) = result {
            tracing::warn!("Could not make the vault files private to this user: {}", e);
        }
    }

    async fn save_identity_current(&self) -> Result<(), VaultError> {
        let identity_path = self.app_dir.join("vault_identity.json");
        if identity_path.exists() {
            let data = tokio::fs::read_to_string(&identity_path).await?;
            let stored: StoredIdentity = serde_json::from_str(&data)?;
            let salt_bytes = BASE64
                .decode(&stored.salt)
                .map_err(|e| VaultError::SerializationError(e.to_string()))?;
            if salt_bytes.len() == 32 {
                let mut salt = [0u8; 32];
                salt.copy_from_slice(&salt_bytes);
                self.save_identity(&salt, None).await?;
            }
        }
        Ok(())
    }

    /// Migrate legacy JSON data.
    async fn migrate_legacy_data(&mut self) -> Result<(), VaultError> {
        // Placeholder for migrating old sessions.json, etc.
        Ok(())
    }

    // ==================== VAULT MANAGEMENT ====================

    /// Create a new vault.
    pub async fn create_vault(
        &mut self,
        name: &str,
        vault_type: VaultType,
        sync_url: Option<&str>,
        sync_token: Option<&str>,
    ) -> Result<VaultInfo, VaultError> {
        let kek = self.kek.as_ref().ok_or(VaultError::Locked)?;
        let user_uuid = self
            .user_uuid
            .clone()
            .ok_or(VaultError::IdentityNotInitialized)?;

        let vault_id = uuid::Uuid::new_v4().to_string();
        let salt = generate_salt();
        let master_dek = generate_dek();
        let wrapped_master_dek = wrap_dek(kek, &master_dek)?;

        let header = VaultHeader {
            id: vault_id.clone(),
            name: name.to_string(),
            salt,
            user_uuid,
            created_at: now_timestamp(),
            vault_type: vault_type.clone(),
        };

        // Create vault directory
        let vault_dir = self.app_dir.join("vaults");
        private_fs::private_dir(&vault_dir)?;
        let db_path = vault_dir.join(format!("{}.db", vault_id));

        let sync_config = match (sync_url, sync_token) {
            (Some(url), Some(token)) => Some(SyncConfig {
                sync_url: url.to_string(),
                auth_token: token.to_string(),
            }),
            _ => None,
        };

        // Every vault is a local file; a synced one also gets its database
        // on the backend, in the sealed record format, with the same header.
        let db = create_replica(&db_path, None).await?;
        let conn = local_connect(&db)?;

        init_schema(&conn).await?;
        sync_local::init(&conn).await?;

        // Store header
        let wrapped_dek_json = serde_json::to_string(&wrapped_master_dek)?;
        let vault_type_json = serde_json::to_string(&header.vault_type)?;
        let header_row = (
            header.id.clone(),
            header.name.clone(),
            header.salt.to_vec(),
            header.user_uuid.clone(),
            header.created_at,
            vault_type_json,
            wrapped_dek_json,
        );
        const INSERT_HEADER: &str = "INSERT INTO vault_header (id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek) VALUES (?, ?, ?, ?, ?, ?, ?)";

        let turso = sync_config.as_ref().filter(|c| {
            matches!(
                sync_engine::Backend::parse(&c.sync_url, &c.auth_token, &self.app_dir),
                Ok(sync_engine::Backend::Turso { .. })
            )
        });
        // A file backend gets the vault on its first sync.
        if let Some(config) = turso {
            let remote = sync_engine::connect_turso(&config.sync_url, &config.auth_token).await?;
            bounded(true, remote.execute_batch(crate::vault::schema::SCHEMA)).await?;
            sync_engine::turso::init_records(&remote).await?;
            bounded(true, remote.execute(INSERT_HEADER, header_row.clone())).await?;
            sync_local::set_state(&conn, sync_engine::STATE_FORMAT, "2").await?;
        }
        conn.execute(INSERT_HEADER, header_row).await?;

        let member_count = match &vault_type {
            VaultType::Private => None,
            VaultType::Shared { members } => Some(members.len()),
        };

        // Store in memory (O(1))
        self.vault_names.insert(name.to_string(), vault_id.clone());
        self.vaults.insert(
            vault_id.clone(),
            VaultConnection {
                db,
                conn: std::sync::Mutex::new(CachedConnection {
                    conn,
                    last_used: std::time::Instant::now(),
                    opened: std::time::Instant::now(),
                    retired: false,
                }),
                header,
                master_dek: Some(master_dek),
                sync_url: sync_url.map(|s| s.to_string()),
                auth_token: sync_token.map(|s| s.to_string()),
            },
        );

        let vault_type_str = match &vault_type {
            VaultType::Private => "private".to_string(),
            VaultType::Shared { .. } => "shared".to_string(),
        };

        // Save user-created vaults (not internal ones) for persistence across restarts
        if !name.starts_with("__") {
            tracing::info!("Saving user vault {} to identity for persistence", name);
            self.user_vaults.push(StoredVaultRef {
                id: vault_id.clone(),
                name: name.to_string(),
                vault_type: vault_type_str.clone(),
                sync_url: sync_url.map(|s| s.to_string()),
                sync_token: sync_token.map(|s| s.to_string()),
                managed_grant: None,
            });
            // Persist to disk
            if let Err(e) = self.save_identity_current().await {
                tracing::warn!("Failed to persist user vault: {}", e);
            }
        }

        Ok(VaultInfo {
            id: vault_id,
            name: name.to_string(),
            vault_type: vault_type_str,
            member_count,
            secret_count: 0,
            last_sync: None,
            unreachable: false,
            sync_error: None,
        })
    }

    /// What each synced vault's sync needs, for `sync_vaults`: every unlocked
    /// vault with a backend, or only `only`.
    pub fn sync_targets(&self, only: Option<&str>) -> Result<Vec<sync_engine::Target>, VaultError> {
        let my_uuid = self.user_uuid.clone().ok_or(VaultError::Locked)?;
        let device = self.device_id.clone().ok_or(VaultError::Locked)?;
        let vault_dir = self.app_dir.join("vaults");
        let mut targets = Vec::new();
        for (id, vault) in &self.vaults {
            if only.is_some_and(|o| o != id) {
                continue;
            }
            let (Some(url), Some(token), Some(master)) = (&vault.sync_url, &vault.auth_token, &vault.master_dek) else {
                continue;
            };
            targets.push(sync_engine::Target {
                vault_id: id.clone(),
                local_path: vault_dir.join(format!("{id}.db")),
                backend: sync_engine::Backend::parse(url, token, &self.app_dir)?,
                master: master.with_key(|k| Dek::from_fn(|d| d.copy_from_slice(k))),
                // Bound to the vault's own id, which every member's file shares,
                // not to the id this device files it under.
                key: sync_record::RecordKey::derive(master, &vault.header.id)?,
                device: device.clone(),
                my_uuid: my_uuid.clone(),
            });
        }
        Ok(targets)
    }

    /// The backend a sync URL and token name, for this app's folders.
    pub fn sync_backend(&self, url: &str, token: &str) -> Result<sync_engine::Backend, VaultError> {
        sync_engine::Backend::parse(url, token, &self.app_dir)
    }

    /// The id of the vault that holds this device's own data when it syncs,
    /// once there is one.
    pub fn personal_vault_id(&self) -> Option<String> {
        let id = self.internal_vault_ids.get("__personal__")?;
        Some(self.vaults.get(id).map(|v| v.header.id.clone()).unwrap_or_else(|| id.clone()))
    }

    /// How each synced vault's sync stands, from its local file.
    pub async fn sync_statuses(&self) -> Vec<SyncStatus> {
        let mut out = Vec::new();
        for (id, vault) in &self.vaults {
            if vault.sync_url.is_none() {
                continue;
            }
            let Ok(conn) = vault.conn() else { continue };
            let backend = match (&vault.sync_url, &vault.auth_token) {
                (Some(url), Some(token)) => sync_engine::Backend::parse(url, token, &self.app_dir).map(|b| b.kind()).unwrap_or("turso"),
                _ => "turso",
            };
            let state = |key: &'static str| {
                let conn = conn.clone();
                async move { sync_local::state(&conn, key).await.ok().flatten() }
            };
            out.push(SyncStatus {
                vault_id: id.clone(),
                name: vault.header.name.clone(),
                backend: backend.into(),
                format: state(sync_engine::STATE_FORMAT).await,
                last_sync: state(sync_engine::STATE_LAST_SYNC).await.and_then(|v| v.parse().ok()),
                last_error: if self.account_vault_suspended(id, vault) {
                    Some(managed_access::SUSPENDED.into())
                } else {
                    state(sync_engine::STATE_LAST_ERROR).await
                },
                conflicts: state(sync_engine::STATE_CONFLICTS).await.and_then(|v| v.parse().ok()).unwrap_or(0),
                pending: sync_local::dirty_count(&conn).await.unwrap_or(0),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Forget the count of conflict copies made, once the user has seen them.
    pub async fn clear_sync_conflicts(&self, vault_id: &str) -> Result<(), VaultError> {
        let vault = self.vaults.get(vault_id).ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        sync_local::clear_state(&vault.conn()?, sync_engine::STATE_CONFLICTS).await
    }

    /// Make the local file of a synced vault seen here for the first time:
    /// its header and members from the backend, no secrets yet. The first
    /// sync brings those. Each request is bounded, so a backend that never
    /// answers fails the open instead of hanging it.
    async fn bootstrap_synced(db_path: &std::path::Path, backend: &sync_engine::Backend) -> Result<(), VaultError> {
        let (header, members) = match tokio::time::timeout(REMOTE_TIMEOUT, backend.read_header()).await {
            Ok(read) => read?.ok_or_else(|| VaultError::NotFound("vault header".into()))?,
            Err(_) => {
                return Err(VaultError::SyncError(format!(
                    "The sync backend did not answer within {} seconds",
                    REMOTE_TIMEOUT.as_secs()
                )))
            }
        };
        Self::write_bootstrap_header(db_path, &header, &members).await
    }

    async fn write_bootstrap_header(
        db_path: &std::path::Path,
        header: &[libsql::Value],
        members: &[Vec<libsql::Value>],
    ) -> Result<(), VaultError> {
        let tmp = db_path.with_extension("db.new");
        let _ = std::fs::remove_file(&tmp);
        {
            let db = create_replica(&tmp, None).await?;
            let conn = local_connect(&db)?;
            init_schema(&conn).await?;
            sync_local::init(&conn).await?;
            sync_engine::mirror_header(&conn, header, members).await?;
        }
        std::fs::rename(&tmp, db_path)?;
        private_fs::restrict_file(db_path)?;
        Ok(())
    }

    /// Open an existing vault: always its local file. A synced vault that has
    /// none yet on this device gets one from its backend first.
    pub async fn open_vault(
        &mut self,
        vault_id: &str,
        sync_url: Option<&str>,
        token: Option<&str>,
    ) -> Result<VaultInfo, VaultError> {
        if self.vaults.contains_key(vault_id) {
            // Already open, return info
            let vault = self.vaults.get(vault_id).unwrap();
            let member_count = match &vault.header.vault_type {
                VaultType::Private => None,
                VaultType::Shared { members } => Some(members.len()),
            };
            return Ok(VaultInfo {
                id: vault_id.to_string(),
                name: vault.header.name.clone(),
                vault_type: match &vault.header.vault_type {
                    VaultType::Private => "private".to_string(),
                    VaultType::Shared { .. } => "shared".to_string(),
                },
                member_count,
                secret_count: 0,
                last_sync: None,
                unreachable: false,
                sync_error: None,
            });
        }

        let vault_dir = self.app_dir.join("vaults");
        private_fs::private_dir(&vault_dir)?;
        let db_path = vault_dir.join(format!("{}.db", vault_id));

        let sync_config = match (sync_url, token) {
            (Some(url), Some(t)) => Some(SyncConfig {
                sync_url: url.to_string(),
                auth_token: t.to_string(),
            }),
            _ => None,
        };

        // A file left by an older version that never held this vault's
        // header (a backup restore wrote one) is completed from the backend
        // too: the first sync then replaces its rows with the backend's.
        let has_header = if db_path.exists() {
            let db = create_replica(&db_path, None).await?;
            let conn = local_connect(&db)?;
            init_schema(&conn).await?;
            let mut rows = conn.query("SELECT 1 FROM vault_header LIMIT 1", ()).await?;
            rows.next().await?.is_some()
        } else {
            false
        };
        if !has_header {
            match &sync_config {
                Some(config) => {
                    let backend = sync_engine::Backend::parse(&config.sync_url, &config.auth_token, &self.app_dir)?;
                    if matches!(&backend, sync_engine::Backend::Account(_)) && db_path.exists() {
                        return Err(VaultError::SyncError("The local Sync vault has no header. The local file was preserved.".into()));
                    }
                    let _ = std::fs::remove_file(&db_path);
                    Self::bootstrap_synced(&db_path, &backend).await?;
                }
                None => return Err(VaultError::NotFound(vault_id.to_string())),
            }
        }

        let db = create_replica(&db_path, None).await?;
        let conn = local_connect(&db)?;
        init_schema(&conn).await?;
        sync_local::init(&conn).await?;

        // Load header
        let (id, name, salt_blob, user_uuid, created_at, vault_type_json) = {
            let mut rows = conn
                .query("SELECT id, name, salt, user_uuid, created_at, vault_type FROM vault_header LIMIT 1", ())
                .await?;
            let row = rows.next().await?.ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
            let header: (String, String, Vec<u8>, String, i64, String) =
                (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?);
            header
        };

        if salt_blob.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: salt_blob.len(),
            });
        }
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_blob);

        let vault_type: VaultType = serde_json::from_str(&vault_type_json)?;

        let header = VaultHeader {
            id,
            name: name.clone(),
            salt,
            user_uuid,
            created_at,
            vault_type: vault_type.clone(),
        };

        let member_count = match &vault_type {
            VaultType::Private => None,
            VaultType::Shared { members } => Some(members.len()),
        };

        let secret_count = Self::count_secrets_on(&conn).await.unwrap_or(0);
        let sync_error = sync_local::state(&conn, sync_engine::STATE_LAST_ERROR).await.ok().flatten();

        // Store in memory (O(1))
        self.vault_names.insert(name.clone(), vault_id.to_string());
        self.vaults.insert(
            vault_id.to_string(),
            VaultConnection {
                db,
                conn: std::sync::Mutex::new(CachedConnection {
                    conn,
                    last_used: std::time::Instant::now(),
                    opened: std::time::Instant::now(),
                    retired: false,
                }),
                header,
                master_dek: None,
                sync_url: sync_url.map(|s| s.to_string()),
                auth_token: token.map(|s| s.to_string()),
            },
        );

        Ok(VaultInfo {
            id: vault_id.to_string(),
            name,
            vault_type: match vault_type {
                VaultType::Private => "private".to_string(),
                VaultType::Shared { .. } => "shared".to_string(),
            },
            member_count,
            secret_count,
            last_sync: None,
            unreachable: false,
            sync_error,
        })
    }

    /// Unlock a vault (derive master DEK).
    /// For vault owners: unwrap using KEK from vault_header.
    /// For invitees: unwrap using X25519 shared secret from vault_members.
    pub async fn unlock_vault(&mut self, vault_id: &str) -> Result<(), VaultError> {
        self.unlock_vault_keys(vault_id).await?;
        // Now that it can be read, bring it up to date with its backend.
        if self.vaults.get(vault_id).is_some_and(|v| v.sync_url.is_some()) {
            request_sync();
        }
        Ok(())
    }

    async fn unlock_vault_keys(&mut self, vault_id: &str) -> Result<(), VaultError> {
        if self.vaults.get(vault_id).is_some_and(|vault| self.account_vault_suspended(vault_id, vault)) {
            return Err(VaultError::AccessDenied(managed_access::SUSPENDED.into()));
        }
        let kek = self.kek.as_ref().ok_or(VaultError::Locked)?;
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let my_uuid = self.user_uuid.clone().ok_or(VaultError::IdentityNotInitialized)?;

        let vault = self
            .vaults
            .get_mut(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;

        if vault.master_dek.is_some() {
            return Ok(()); // Already unlocked
        }

        // Check if we're the vault owner
        let mut header_rows = vault
            
            .query("SELECT user_uuid, wrapped_master_dek FROM vault_header LIMIT 1", ())
            .await?;
        let header_row = header_rows
            .next()
            .await?
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        let owner_uuid: String = header_row.get(0)?;
        let owner_wrapped_dek_json: String = header_row.get(1)?;

        #[cfg(desktop)]
        if vault.is_organization_vault() && vault.organization_key_epoch().await? > 0 {
            vault.master_dek = Some(vault.organization_master(identity).await?);
            return Ok(());
        }

        if owner_uuid == my_uuid {
            // We're the owner - unwrap with KEK
            tracing::info!("Unlocking vault {} as owner", vault_id);
            let wrapped_dek: WrappedDek = serde_json::from_str(&owner_wrapped_dek_json)?;
            let master_dek = unwrap_dek(kek, &wrapped_dek)?;
            vault.master_dek = Some(master_dek);
            return Ok(());
        }

        #[cfg(desktop)]
        if vault.is_organization_vault() {
            vault.master_dek = Some(vault.organization_master(identity).await?);
            return Ok(());
        }

        // We're not the owner - check vault_members table
        tracing::info!("Unlocking vault {} as member (uuid: {})", vault_id, my_uuid);
        let mut member_rows = vault
            
            .query(
                "SELECT wrapped_master_dek, inviter_public_key FROM vault_members WHERE user_uuid = ?",
                [my_uuid.as_str()],
            )
            .await?;

        let member_row = member_rows.next().await?.ok_or_else(|| {
            tracing::error!("User {} not found in vault_members for vault {}", my_uuid, vault_id);
            VaultError::AccessDenied(format!("You are not a member of this vault: {}", vault_id))
        })?;

        let member_wrapped_dek_json: String = member_row.get(0)?;
        let inviter_pk_b64: String = member_row.get(1)?;
        let master_dek = unwrap_member_dek(identity, &member_wrapped_dek_json, &inviter_pk_b64)?;
        vault.master_dek = Some(master_dek);

        tracing::info!("Successfully unlocked vault {} as member", vault_id);
        Ok(())
    }

    /// Lock a vault.
    pub fn lock_vault(&mut self, vault_id: &str) {
        if let Some(vault) = self.vaults.get_mut(vault_id) {
            vault.master_dek = None;
        }
    }

    /// Close a vault.
    pub async fn close_vault(&mut self, vault_id: &str) -> Result<(), VaultError> {
        if self.vaults.contains_key(vault_id) {
            // Remove from name mapping
            self.vault_names.retain(|_, v| v != vault_id);
        }
        self.vaults.remove(vault_id);
        Ok(())
    }

    /// Shared vaults that are here more than once: entries over the same
    /// database, joined twice (issue #77). Each group lists every entry, in
    /// the order they were joined, as the vault panel shows them; an entry
    /// that did not connect is marked unreachable. Nothing is removed here:
    /// the user picks which entry stays, since the two may hold different
    /// tokens and only one may still work.
    pub async fn duplicate_vaults(&self) -> Vec<Vec<VaultInfo>> {
        let listed = self.list_vaults().await.unwrap_or_default();
        duplicate_groups(&self.user_vaults)
            .into_iter()
            .map(|group| {
                group
                    .into_iter()
                    .map(|id| {
                        listed.iter().find(|v| v.id == id).cloned().unwrap_or_else(|| {
                            let stored = self.user_vaults.iter().find(|v| v.id == id);
                            VaultInfo {
                                name: stored.map(|v| v.name.clone()).unwrap_or_default(),
                                vault_type: stored.map(|v| v.vault_type.clone()).unwrap_or_default(),
                                id,
                                member_count: None,
                                secret_count: 0,
                                last_sync: None,
                                unreachable: true,
                                sync_error: Some("did not connect".into()),
                            }
                        })
                    })
                    .collect()
            })
            .collect()
    }

    /// The user the identity file on disk belongs to, if there is one.
    async fn stored_user_uuid(&self) -> Option<String> {
        let raw = tokio::fs::read_to_string(self.app_dir.join("vault_identity.json")).await.ok()?;
        serde_json::from_str::<StoredIdentity>(&raw).ok().map(|s| s.user_uuid)
    }

    /// Delete a vault.
    pub async fn delete_vault(&mut self, vault_id: &str) -> Result<(), VaultError> {
        self.close_vault(vault_id).await?;

        // Remove from user_vaults if present
        self.user_vaults.retain(|v| v.id != vault_id);
        if let Err(e) = self.save_identity_current().await {
            tracing::warn!("Failed to persist user vault removal: {}", e);
        }

        // Delete local file
        let vault_dir = self.app_dir.join("vaults");
        let db_path = vault_dir.join(format!("{}.db", vault_id));
        if db_path.exists() {
            tokio::fs::remove_file(&db_path).await?;
        }
        let _ = std::fs::remove_file(vault_dir.join(format!("{vault_id}.cache")));

        Ok(())
    }

    /// List all vaults.
    /// Count the secrets in a vault. Split out because both `open_vault` and
    /// `list_vaults` need it and, on a shared vault, it is a remote call that
    /// can fail on its own.
    /// Takes the vault rather than a bare connection so that counting, which
    /// decides whether a vault is reported as reachable, gets the same retry
    /// as every other read. A vault called unreachable because of one dead
    /// connection is the wrong answer.
    async fn count_secrets(vault: &VaultConnection) -> Result<usize, VaultError> {
        Self::read_count(vault.query("SELECT COUNT(*) FROM secrets", ()).await?).await
    }

    /// The same count on a connection that has only just been opened, where
    /// there is no cached connection to retry against yet.
    async fn count_secrets_on(conn: &Connection) -> Result<usize, VaultError> {
        Self::read_count(conn.query("SELECT COUNT(*) FROM secrets", ()).await?).await
    }

    async fn read_count(mut rows: libsql::Rows) -> Result<usize, VaultError> {
        let count: i64 = match rows.next().await? {
            Some(row) => row.get(0)?,
            None => 0,
        };
        Ok(count as usize)
    }

    pub async fn list_vaults(&self) -> Result<Vec<VaultInfo>, VaultError> {
        let mut vaults = Vec::new();
        for (vault_id, vault) in &self.vaults {
            // Skip internal vaults
            if vault.header.name.starts_with("__") {
                continue;
            }

            let member_count = match &vault.header.vault_type {
                VaultType::Private => None,
                VaultType::Shared { members } => Some(members.len()),
            };

            // Count secrets. A shared vault is a remote-only Turso connection
            // (see `sync::create_replica`), so this one line is a network call:
            // an expired token, a paused database or a dead link makes it fail.
            // Propagating that would fail the *whole* listing and empty the
            // vault panel, including local vaults that are perfectly healthy —
            // so a vault that cannot answer is reported as unreachable instead.
            let (secret_count, sync_error) =
                match Self::count_secrets(vault).await {
                    Ok(n) => (n, None),
                    Err(e) => {
                        let reason = describe_db_error(&e.to_string());
                        tracing::warn!(
                            vault = %vault.header.name,
                            "vault unreachable while listing: {}",
                            reason
                        );
                        (0, Some(reason))
                    }
                };

            // The vault answers from its file; whether it reached its backend
            // the last time it tried is in the file too.
            let (last_sync, sync_error) = match (sync_error, vault.sync_url.is_some(), vault.conn()) {
                (None, true, Ok(conn)) => (
                    sync_local::state(&conn, sync_engine::STATE_LAST_SYNC).await.ok().flatten().and_then(|v| v.parse::<i64>().ok()).map(|ms| ms / 1000),
                    sync_local::state(&conn, sync_engine::STATE_LAST_ERROR).await.ok().flatten().map(|e| describe_db_error(&e)),
                ),
                (sync_error, _, _) => (None, sync_error),
            };
            let sync_error = if self.account_vault_suspended(vault_id, vault) {
                Some(managed_access::SUSPENDED.into())
            } else {
                sync_error
            };
            vaults.push(VaultInfo {
                id: vault_id.clone(),
                name: vault.header.name.clone(),
                vault_type: match &vault.header.vault_type {
                    VaultType::Private => "private".to_string(),
                    VaultType::Shared { .. } => "shared".to_string(),
                },
                member_count,
                secret_count,
                last_sync,
                unreachable: sync_error.is_some(),
                sync_error,
            });
        }
        Ok(vaults)
    }

    /// Check that a vault is open. The sync itself runs in
    /// [`sync_vaults`], without holding the manager.
    pub async fn sync_vault(&mut self, vault_id: &str) -> Result<(), VaultError> {
        self.vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        Ok(())
    }

    // ==================== SECRETS ====================

    /// Get internal vault ID helper.
    fn get_internal_vault_id(&self, name: &str) -> Result<String, VaultError> {
        self.vault_names
            .get(name)
            .cloned()
            .ok_or_else(|| VaultError::NotFound(name.to_string()))
    }

    pub fn sessions_vault_id(&self) -> Result<String, VaultError> {
        self.get_internal_vault_id(SESSIONS_VAULT)
    }

    pub fn credentials_vault_id(&self) -> Result<String, VaultError> {
        self.get_internal_vault_id(CREDENTIALS_VAULT)
    }

    pub fn folders_vault_id(&self) -> Result<String, VaultError> {
        self.get_internal_vault_id(FOLDERS_VAULT)
    }

    pub fn playbooks_vault_id(&self) -> Result<String, VaultError> {
        self.get_internal_vault_id(PLAYBOOKS_VAULT)
    }

    pub fn settings_vault_id(&self) -> Result<String, VaultError> {
        self.get_internal_vault_id(SETTINGS_VAULT)
    }

    /// Create a secret (O(1) by ID).
    pub async fn create_secret(
        &self,
        vault_id: &str,
        name: &str,
        category: SecretCategory,
        plaintext: SecretBox<Vec<u8>>,
    ) -> Result<String, VaultError> {
        let secret_id = uuid::Uuid::new_v4().to_string();
        self.create_secret_with_id(vault_id, &secret_id, name, category, plaintext)
            .await?;
        Ok(secret_id)
    }

    /// Create a secret with specific ID.
    pub async fn create_secret_with_id(
        &self,
        vault_id: &str,
        secret_id: &str,
        name: &str,
        category: SecretCategory,
        plaintext: SecretBox<Vec<u8>>,
    ) -> Result<(), VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        #[cfg(desktop)]
        vault.ensure_organization_writable().await?;
        let master_dek = vault
            .master_dek
            .as_ref()
            .ok_or_else(|| VaultError::NotUnlocked(vault_id.to_string()))?;

        let payload = encrypt_secret(master_dek, plaintext.expose_secret())?;
        let now = now_timestamp();
        let wrapped_dek_json = serde_json::to_string(&payload.wrapped_dek)?;

        vault.execute(
            "INSERT OR REPLACE INTO secrets (id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                secret_id,
                name,
                category.to_string(),
                payload.nonce.as_slice(),
                payload.ciphertext.as_slice(),
                wrapped_dek_json.clone(),
                now,
                now,
            ),
        ).await?;

        vault.changed();
        Ok(())
    }

    /// Read a secret (O(1) by ID).
    pub async fn read_secret(
        &self,
        vault_id: &str,
        secret_id: &str,
    ) -> Result<SecretBox<Vec<u8>>, VaultError> {
        self.read_secret_in_category(vault_id, secret_id, None).await
    }

    pub(crate) async fn read_secret_in_category(
        &self,
        vault_id: &str,
        secret_id: &str,
        category: Option<&str>,
    ) -> Result<SecretBox<Vec<u8>>, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        let master_dek = vault
            .master_dek
            .as_ref()
            .ok_or_else(|| VaultError::NotUnlocked(vault_id.to_string()))?;

        let mut rows = vault
            
            .query(
                "SELECT nonce, ciphertext, wrapped_dek FROM secrets WHERE id = ?1 AND (?2 IS NULL OR category = ?2)",
                (secret_id, category),
            )
            .await?;

        let row = rows
            .next()
            .await?
            .ok_or_else(|| VaultError::SecretNotFound(secret_id.to_string()))?;

        decrypt_stored(master_dek, row.get(0)?, row.get(1)?, row.get(2)?)
    }

    /// Every secret of the given categories, decrypted, in one query.
    ///
    /// Lists used to be built as `list_secrets` and then a `read_secret` per
    /// item. On a synced vault each query is a round trip to Turso, so a list
    /// of thirty sessions cost thirty-one of them in a row, several seconds.
    /// This is one. An item that does not decrypt comes back as its error,
    /// beside the others, so one bad row never hides the rest.
    pub async fn read_secrets_in(
        &self,
        vault_id: &str,
        categories: &[&str],
    ) -> Result<Vec<(SecretMetadata, Result<SecretBox<Vec<u8>>, VaultError>)>, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        let master_dek = vault
            .master_dek
            .as_ref()
            .ok_or_else(|| VaultError::NotUnlocked(vault_id.to_string()))?;
        if categories.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; categories.len()].join(", ");
        let sql = format!(
            "SELECT id, name, category, created_at, updated_at, nonce, ciphertext, wrapped_dek              FROM secrets WHERE category IN ({placeholders})"
        );
        let params: Vec<libsql::Value> = categories.iter().map(|c| libsql::Value::Text(c.to_string())).collect();
        let mut rows = vault.query(&sql, params).await?;

        let mut secrets = Vec::new();
        while let Some(row) = rows.next().await? {
            let meta = SecretMetadata {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            };
            let plaintext = decrypt_stored(master_dek, row.get(5)?, row.get(6)?, row.get(7)?);
            secrets.push((meta, plaintext));
        }
        Ok(secrets)
    }

    /// Update a secret.
    pub async fn update_secret(
        &self,
        vault_id: &str,
        secret_id: &str,
        plaintext: SecretBox<Vec<u8>>,
    ) -> Result<(), VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        #[cfg(desktop)]
        vault.ensure_organization_writable().await?;
        let master_dek = vault
            .master_dek
            .as_ref()
            .ok_or_else(|| VaultError::NotUnlocked(vault_id.to_string()))?;

        let payload = encrypt_secret(master_dek, plaintext.expose_secret())?;
        let now = now_timestamp();
        let wrapped_dek_json = serde_json::to_string(&payload.wrapped_dek)?;

        vault
            
            .execute(
                "UPDATE secrets SET nonce = ?, ciphertext = ?, wrapped_dek = ?, updated_at = ? WHERE id = ?",
                (
                    payload.nonce.as_slice(),
                    payload.ciphertext.as_slice(),
                    wrapped_dek_json.clone(),
                    now,
                    secret_id,
                ),
            )
            .await?;

        vault.changed();

        Ok(())
    }

    /// Change a secret's name, leaving its ciphertext alone. Renaming an
    /// imported key must not re-encrypt the key.
    pub async fn rename_secret(
        &self,
        vault_id: &str,
        secret_id: &str,
        name: &str,
    ) -> Result<(), VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        #[cfg(desktop)]
        vault.ensure_organization_writable().await?;

        let now = now_timestamp();
        vault
            
            .execute(
                "UPDATE secrets SET name = ?, updated_at = ? WHERE id = ?",
                (name, now, secret_id),
            )
            .await?;

        vault.changed();

        Ok(())
    }

    /// Delete a secret.
    pub async fn delete_secret(&self, vault_id: &str, secret_id: &str) -> Result<(), VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        #[cfg(desktop)]
        vault.ensure_organization_writable().await?;

        vault
            
            .execute("DELETE FROM secrets WHERE id = ?", [secret_id])
            .await?;

        vault.changed();

        Ok(())
    }

    /// Check if a secret exists. A failed check is logged and reads as no.
    pub async fn secret_exists(&self, vault_id: &str, secret_id: &str) -> bool {
        if !self.vaults.contains_key(vault_id) {
            return false;
        }
        self.find_secret(vault_id, secret_id).await.unwrap_or_else(|e| {
            tracing::error!("Could not check for secret {} in vault {}: {}", secret_id, vault_id, e);
            false
        })
    }

    /// Whether a secret exists, or why that could not be found out. Goes
    /// through [`VaultConnection::query`] like every other read, so a dead
    /// Turso stream is reconnected instead of reported as a missing secret
    /// (#148).
    pub async fn find_secret(&self, vault_id: &str, secret_id: &str) -> Result<bool, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        let mut rows = vault.query("SELECT 1 FROM secrets WHERE id = ?", [secret_id]).await?;
        Ok(rows.next().await?.is_some())
    }

    /// Get vault ID by name.
    pub fn get_vault_id_by_name(&self, name: &str) -> Option<String> {
        self.vault_names.get(name).cloned()
    }

    /// List secrets (metadata only).
    pub async fn list_secrets(&self, vault_id: &str) -> Result<Vec<SecretMetadata>, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;

        let mut rows = vault
            
            .query(
                "SELECT id, name, category, created_at, updated_at FROM secrets",
                (),
            )
            .await?;

        let mut secrets = Vec::new();
        while let Some(row) = rows.next().await? {
            secrets.push(SecretMetadata {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            });
        }

        Ok(secrets)
    }

    // ==================== SHARING ====================

    /// The id of the vault a backend database already holds, if any.
    async fn remote_vault_id(&self, url: &str, token: &str) -> Option<String> {
        let backend = sync_engine::Backend::parse(url, token, &self.app_dir).ok()?;
        let read = tokio::time::timeout(REMOTE_TIMEOUT, backend.read_header());
        let (header, _) = read.await.ok()?.ok()??;
        match header.first() {
            Some(libsql::Value::Text(id)) => Some(id.clone()),
            _ => None,
        }
    }

    /// Where a vault's member list and shares are kept: on its backend for a
    /// synced vault, which every member reads, otherwise in its file.
    async fn admin_conn(&self, vault: &VaultConnection) -> Result<Connection, VaultError> {
        match (&vault.sync_url, &vault.auth_token) {
            (Some(url), Some(token)) if matches!(
                sync_engine::Backend::parse(url, token, &self.app_dir),
                Ok(sync_engine::Backend::Turso { .. })
            ) => sync_engine::connect_turso(url, token).await,
            _ => vault.conn(),
        }
    }

    /// Invite member to shared vault.
    pub async fn invite_member(
        &self,
        vault_id: &str,
        invitee_public_key: &[u8; 32],
        invitee_uuid: &str,
        role: MemberRole,
    ) -> Result<InviteInfo, VaultError> {
        let identity = self.identity.as_ref().ok_or(VaultError::Locked)?;
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        #[cfg(desktop)]
        if vault.is_organization_vault() {
            return Err(VaultError::AccessDenied("Manage organization vault access in Settings → Account".into()));
        }


        let master_dek = vault.master_dek.as_ref().ok_or(VaultError::Locked)?;

        // Re-wrap master DEK for invitee using X25519
        let invitee_pk = x25519_dalek::PublicKey::from(*invitee_public_key);
        let shared_secret = identity.with_secret(|secret| secret.diffie_hellman(&invitee_pk));

        // Derive wrapping key from shared secret
        let mut wrapping_key = [0u8; 32];
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, shared_secret.as_bytes());
        hk.expand(b"vault-member-dek", &mut wrapping_key)
            .map_err(|_| VaultError::CryptoError("HKDF expand failed".to_string()))?;

        // Wrap DEK with derived key
        let wrapped_for_invitee = wrap_dek_with_key(&wrapping_key, master_dek)?;

        // Store in vault_members table
        let wrapped_dek_json = serde_json::to_string(&wrapped_for_invitee)?;
        let role_str = match role {
            MemberRole::Owner => "owner",
            MemberRole::Admin => "admin",
            MemberRole::Member => "member",
            MemberRole::ReadOnly => "readonly",
        };
        let inviter_pk_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            identity.public_key.as_bytes(),
        );

        let admin = self.admin_conn(vault).await?;
        admin
            .execute(
                "INSERT OR REPLACE INTO vault_members (user_uuid, public_key, wrapped_master_dek, role, added_at, inviter_public_key) VALUES (?, ?, ?, ?, ?, ?)",
                (
                    invitee_uuid,
                    invitee_public_key.as_slice(),
                    wrapped_dek_json.as_str(),
                    role_str,
                    now_timestamp(),
                    inviter_pk_b64.as_str(),
                ),
            )
            .await?;

        vault.changed(); // mirror the member list into the local file
        let sync_url = vault.sync_url.clone().unwrap_or_default();
        let token = vault.auth_token.clone().unwrap_or_default();

        tracing::info!("Invited {} to vault {} with role {}", invitee_uuid, vault_id, role_str);

        Ok(InviteInfo {
            vault_id: vault_id.to_string(),
            sync_url,
            token,
        })
    }

    /// Accept invite to shared vault.
    pub async fn accept_invite(
        &mut self,
        sync_url: &str,
        token: &str,
    ) -> Result<VaultInfo, VaultError> {
        // An invite to a vault already joined here is the same vault, not a
        // second one: two entries over one database list everything twice.
        // The newer token is kept, in case the old one was rotated.
        if let Some(existing) = self.user_vaults.iter_mut().find(|v| same_database(v.sync_url.as_deref(), Some(sync_url))) {
            tracing::info!("Invite is for vault {} ({}), which is already joined", existing.name, existing.id);
            let id = existing.id.clone();
            if existing.sync_token.as_deref() != Some(token) {
                existing.sync_token = Some(token.to_string());
                // The open connection still has the old token; reopen with the new one.
                self.close_vault(&id).await?;
                if let Err(e) = self.save_identity_current().await {
                    tracing::warn!("Failed to persist the new invite token: {}", e);
                }
            }
            if !self.vaults.contains_key(&id) {
                self.open_vault(&id, Some(sync_url), Some(token)).await?;
                self.unlock_vault(&id).await?;
            }
            return self
                .list_vaults()
                .await?
                .into_iter()
                .find(|v| v.id == id)
                .ok_or(VaultError::NotFound(id));
        }

        // Generate new vault ID for local tracking
        let vault_id = uuid::Uuid::new_v4().to_string();

        // Open the vault
        let vault_info = self.open_vault(&vault_id, Some(sync_url), Some(token)).await?;

        // Unlock it
        self.unlock_vault(&vault_id).await?;

        // Save to user_vaults for persistence across restarts
        tracing::info!("Saving accepted invite vault {} for persistence", vault_info.name);
        self.user_vaults.push(StoredVaultRef {
            id: vault_id.clone(),
            name: vault_info.name.clone(),
            vault_type: vault_info.vault_type.clone(),
            sync_url: Some(sync_url.to_string()),
            sync_token: Some(token.to_string()),
            managed_grant: None,
        });

        // Persist to identity file
        if let Err(e) = self.save_identity_current().await {
            tracing::warn!("Failed to persist accepted invite: {}", e);
        }

        Ok(vault_info)
    }

    /// Remove member from vault.
    pub async fn remove_member(&self, vault_id: &str, user_uuid: &str) -> Result<(), VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;
        #[cfg(desktop)]
        if vault.is_organization_vault() {
            return Err(VaultError::AccessDenied("Manage organization vault access in Settings → Account".into()));
        }


        let admin = self.admin_conn(vault).await?;
        admin
            .execute(
                "DELETE FROM vault_members WHERE user_uuid = ?",
                [user_uuid],
            )
            .await?;
        vault.changed();

        Ok(())
    }

    /// List vault members.
    pub async fn list_members(&self, vault_id: &str) -> Result<Vec<MemberInfo>, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;

        let admin = self.admin_conn(vault).await?;
        let mut rows = admin
            .query(
                "SELECT user_uuid, public_key, role, added_at FROM vault_members",
                (),
            )
            .await?;

        let mut members = Vec::new();
        while let Some(row) = rows.next().await? {
            let pk_blob: Vec<u8> = row.get(1)?;
            members.push(MemberInfo {
                user_uuid: row.get(0)?,
                public_key: BASE64.encode(&pk_blob),
                role: row.get(2)?,
                added_at: row.get(3)?,
            });
        }

        Ok(members)
    }

    // ==================== SHARE INDIVIDUAL ITEMS ====================

    /// Share a specific secret with another user.
    pub async fn share_item(
        &self,
        vault_id: &str,
        secret_id: &str,
        recipient_uuid: &str,
        recipient_public_key: &[u8; 32],
        expires_in_hours: Option<u64>,
    ) -> Result<ShareItemResult, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;

        let share_id = uuid::Uuid::new_v4().to_string();
        let now = now_timestamp();
        let expires_at = expires_in_hours.map(|h| now + (h as i64 * 3600));

        // TODO: Re-wrap DEK with recipient's public key

        let admin = self.admin_conn(vault).await?;
        admin.execute(
            "INSERT INTO shared_items (id, secret_id, recipient_uuid, recipient_public_key, wrapped_dek, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                share_id.as_str(),
                secret_id,
                recipient_uuid,
                recipient_public_key.as_slice(),
                "{}",  // TODO: Wrapped DEK
                expires_at,
                now,
            ),
        ).await?;

        Ok(ShareItemResult {
            share_id,
            secret_id: secret_id.to_string(),
            recipient_uuid: recipient_uuid.to_string(),
            sync_url: vault.sync_url.clone(),
            expires_at,
        })
    }

    /// List shared items from a vault.
    pub async fn list_shared_items(&self, vault_id: &str) -> Result<Vec<SharedItemInfo>, VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;

        let admin = self.admin_conn(vault).await?;
        let mut rows = admin
            .query(
                "SELECT id, secret_id, recipient_uuid, expires_at, created_at FROM shared_items",
                (),
            )
            .await?;

        let mut items = Vec::new();
        while let Some(row) = rows.next().await? {
            items.push(SharedItemInfo {
                id: row.get(0)?,
                secret_id: row.get(1)?,
                recipient_uuid: row.get(2)?,
                expires_at: row.get(3)?,
                created_at: row.get(4)?,
            });
        }

        Ok(items)
    }

    /// Revoke a shared item.
    pub async fn revoke_shared_item(&self, vault_id: &str, share_id: &str) -> Result<(), VaultError> {
        let vault = self
            .vaults
            .get(vault_id)
            .ok_or_else(|| VaultError::NotFound(vault_id.to_string()))?;

        let admin = self.admin_conn(vault).await?;
        admin
            .execute("DELETE FROM shared_items WHERE id = ?", [share_id])
            .await?;

        Ok(())
    }

    /// Accept a shared item.
    pub async fn accept_shared_item(
        &self,
        _source_vault_id: &str,
        share_id: &str,
        _target_vault_id: &str,
    ) -> Result<String, VaultError> {
        // TODO: Implement copying shared secret to target vault
        Ok(share_id.to_string())
    }

    /// List items shared with me.
    pub async fn list_received_shares(&self) -> Result<Vec<ReceivedShare>, VaultError> {
        // TODO: Implement querying across vaults
        Ok(Vec::new())
    }

    // ==================== APP SETTINGS ====================

    /// Import identity from backup.
    pub async fn import_identity(&mut self, secret_key_b64: &str) -> Result<String, VaultError> {
        let identity_path = self.app_dir.join("vault_identity.json");
        if identity_path.exists() {
            return Err(VaultError::IdentityAlreadyExists);
        }

        let secret_key_bytes = BASE64
            .decode(secret_key_b64)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;

        if secret_key_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: secret_key_bytes.len(),
            });
        }

        let mut sk_array = [0u8; 32];
        sk_array.copy_from_slice(&secret_key_bytes);
        let secret_key = StaticSecret::from(sk_array);
        let public_key = PublicKey::from(&secret_key);

        let user_uuid = uuid::Uuid::new_v4().to_string();
        let salt = generate_salt();

        let kek = derive_kek_from_secret_key(secret_key.as_bytes(), &salt)?;
        self.kek = Some(kek);

        self.identity = Some(UserIdentity::new(user_uuid.clone(), secret_key.clone()));
        self.user_uuid = Some(user_uuid.clone());
        self.identity_public_key = Some(public_key.to_bytes());

        self.device_id = Some(uuid::Uuid::new_v4().to_string());

        // Store in keychain
        if let Err(e) = store_key_in_keychain(&user_uuid, secret_key.as_bytes()) {
            tracing::error!(
                "Could not store the vault key in the OS keychain: {}. Auto-unlock will not work on the next launch; your password is the only way back in.",
                e
            );
        }

        self.save_identity(&salt, None).await?;

        // Create internal vaults
        self.ensure_internal_vaults().await?;

        Ok(user_uuid)
    }

    /// Get app settings from encrypted __settings__ vault.
    pub async fn get_settings(&self) -> Result<AppSettings, VaultError> {
        let vault_id = match self.settings_vault_id() {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Settings vault not available: {}. vault_names keys: {:?}",
                    e,
                    self.vault_names.keys().collect::<Vec<_>>()
                );
                return Ok(AppSettings::default());
            }
        };

        // Settings stored under key "app_settings"
        let settings_key = "app_settings";

        let vault = match self.vaults.get(&vault_id) {
            Some(v) => v,
            None => {
                tracing::warn!(
                    "Settings vault {} not in vaults HashMap. Available: {:?}",
                    vault_id,
                    self.vaults.keys().collect::<Vec<_>>()
                );
                return Ok(AppSettings::default());
            }
        };

        // Check if vault is unlocked
        if vault.master_dek.is_none() {
            tracing::warn!("Settings vault {} is not unlocked", vault_id);
            return Ok(AppSettings::default());
        }

        let mut rows = vault
            
            .query("SELECT id FROM secrets WHERE name = ?", [settings_key])
            .await?;

        if let Some(row) = rows.next().await? {
            let secret_id: String = row.get(0)?;
            let data = self.read_secret(&vault_id, &secret_id).await?;
            let settings: AppSettings = serde_json::from_slice(data.expose_secret())?;
            Ok(settings)
        } else {
            Ok(AppSettings::default())
        }
    }

    /// Save app settings to encrypted __settings__ vault.
    pub async fn save_settings(&self, settings: &AppSettings) -> Result<(), VaultError> {
        let vault_id = self.settings_vault_id().inspect_err(|_| {
            tracing::error!(
                "Cannot save settings: vault not available. vault_names keys: {:?}",
                self.vault_names.keys().collect::<Vec<_>>()
            );
        })?;
        let settings_key = "app_settings";

        let vault = self.vaults.get(&vault_id).ok_or_else(|| {
            tracing::error!(
                "Cannot save settings: vault {} not in vaults HashMap. Available: {:?}",
                vault_id,
                self.vaults.keys().collect::<Vec<_>>()
            );
            VaultError::NotFound(vault_id.clone())
        })?;

        // Check if vault is unlocked
        if vault.master_dek.is_none() {
            tracing::error!("Cannot save settings: vault {} is not unlocked", vault_id);
            return Err(VaultError::NotUnlocked(vault_id));
        }

        let data = serde_json::to_vec(settings)?;
        let secret_data = SecretBox::new(Box::new(data));

        // Check if exists
        let mut rows = vault
            
            .query("SELECT id FROM secrets WHERE name = ?", [settings_key])
            .await?;

        if let Some(row) = rows.next().await? {
            let secret_id: String = row.get(0)?;
            self.update_secret(&vault_id, &secret_id, secret_data)
                .await?;
        } else {
            self.create_secret(&vault_id, settings_key, SecretCategory::Setting, secret_data)
                .await?;
        }

        tracing::info!("Settings saved successfully to vault {}", vault_id);
        Ok(())
    }

    /// Get Turso config.
    pub async fn get_turso_config(&self) -> Result<(Option<String>, Option<String>), VaultError> {
        let settings = self.get_settings().await?;
        Ok((settings.turso_org, settings.turso_api_token))
    }

    /// Set Turso config.
    pub async fn set_turso_config(
        &self,
        org: Option<String>,
        token: Option<String>,
    ) -> Result<(), VaultError> {
        let mut settings = self.get_settings().await.unwrap_or_default();
        settings.turso_org = org;
        settings.turso_api_token = token;
        settings.sync_enabled = settings.turso_org.is_some() && settings.turso_api_token.is_some();
        self.save_settings(&settings).await
    }

    // ==================== FULL BACKUP EXPORT/IMPORT ====================

    /// Export a full backup, sealed with `export_password`, as the bytes of a
    /// `.reachbackup` file. Where it is written is the caller's business: a
    /// path, or on Android a link from the system's file picker.
    #[tracing::instrument(skip(self, export_password))]
    pub async fn export_full_backup(&self, export_password: &str) -> Result<Vec<u8>, VaultError> {
        use crate::vault::export::*;

        let _kek = self.kek.as_ref().ok_or(VaultError::Locked)?;

        // Read identity file
        let identity_path = self.app_dir.join("vault_identity.json");
        let identity_data = tokio::fs::read_to_string(&identity_path).await?;
        let stored: StoredIdentity = serde_json::from_str(&identity_data)?;

        // Include raw secret key in backup — on the target machine, the OS
        // keychain won't have it, so we need it in the bundle.  The bundle
        // itself is sealed with the export password (XChaCha20-Poly1305).
        let secret_key_b64 = {
            let id = self.identity.as_ref().ok_or(VaultError::Locked)?;
            id.with_secret(|secret| BASE64.encode(secret.as_bytes()))
        };

        let identity = ExportedIdentity {
            user_uuid: stored.user_uuid.clone(),
            salt: stored.salt.clone(),
            encrypted_key: stored.encrypted_key.clone(),
            nonce: stored.nonce.clone(),
            public_key: stored.public_key.clone(),
            secret_key: Some(secret_key_b64),
        };

        // Export all vaults (internal + user)
        let mut exported_vaults = Vec::new();

        for (vault_id, vault) in &self.vaults {
            let is_internal = vault.header.name.starts_with("__");
            let vault_type_json = serde_json::to_string(&vault.header.vault_type)?;

            // Read wrapped_master_dek from header
            let mut header_rows = vault
                
                .query("SELECT wrapped_master_dek FROM vault_header LIMIT 1", ())
                .await?;
            let wrapped_master_dek = if let Some(row) = header_rows.next().await? {
                let dek: String = row.get(0)?;
                dek
            } else {
                String::new()
            };

            let header = ExportedVaultHeader {
                id: vault.header.id.clone(),
                name: vault.header.name.clone(),
                salt: BASE64.encode(vault.header.salt),
                user_uuid: vault.header.user_uuid.clone(),
                created_at: vault.header.created_at,
                vault_type: vault_type_json.clone(),
                wrapped_master_dek,
            };

            // Export secrets (ciphertext only — never decrypted)
            let mut secrets = Vec::new();
            let mut secret_rows = vault
                
                .query(
                    "SELECT id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at FROM secrets",
                    (),
                )
                .await?;

            while let Some(row) = secret_rows.next().await? {
                let nonce_blob: Vec<u8> = row.get(3)?;
                let ct_blob: Vec<u8> = row.get(4)?;
                secrets.push(ExportedSecret {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    category: row.get(2)?,
                    nonce: BASE64.encode(&nonce_blob),
                    ciphertext: BASE64.encode(&ct_blob),
                    wrapped_dek_json: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                });
            }

            // Export members
            let mut members = Vec::new();
            let mut member_rows = vault
                
                .query(
                    "SELECT user_uuid, public_key, wrapped_master_dek, role, added_at, inviter_public_key FROM vault_members",
                    (),
                )
                .await?;

            while let Some(row) = member_rows.next().await? {
                let pk_blob: Vec<u8> = row.get(1)?;
                let inviter_pk: Option<String> = row.get(5).ok();
                members.push(ExportedMember {
                    user_uuid: row.get(0)?,
                    public_key: BASE64.encode(&pk_blob),
                    wrapped_master_dek_json: row.get(2)?,
                    role: row.get(3)?,
                    added_at: row.get(4)?,
                    inviter_public_key: inviter_pk,
                });
            }

            exported_vaults.push(ExportedVault {
                vault_id: vault_id.clone(),
                name: vault.header.name.clone(),
                vault_type: vault_type_json,
                is_internal,
                header,
                secrets,
                members,
            });
        }

        // Sync config
        // Tokens come from memory: the file holds them sealed.
        let sync_config = if self.personal_sync_url.is_some() || !self.user_vaults.is_empty() {
            Some(ExportedSyncConfig {
                personal_sync_url: self.personal_sync_url.clone(),
                personal_sync_token: self.personal_sync_token.clone(),
                user_vaults: self
                    .user_vaults
                    .iter()
                    .map(|v| ExportedUserVault {
                        id: v.id.clone(),
                        name: v.name.clone(),
                        vault_type: v.vault_type.clone(),
                        sync_url: v.sync_url.clone(),
                        sync_token: v.sync_token.clone(),
                    })
                    .collect(),
            })
        } else {
            None
        };

        // Read current app settings (decrypted) to include in backup
        let app_settings = match self.get_settings().await {
            Ok(s) => {
                tracing::info!("Including app settings in backup (turso_org={:?}, sync_enabled={})", s.turso_org.as_ref().map(|_| "[set]"), s.sync_enabled);
                Some(s)
            }
            Err(e) => {
                tracing::warn!("Could not read app settings for backup: {}", e);
                None
            }
        };

        let bundle = ExportBundle {
            version: 1,
            exported_at: now_timestamp(),
            identity,
            vaults: exported_vaults,
            sync_config,
            app_settings,
        };

        let sealed = seal_bundle(&bundle, export_password)?;
        tracing::info!("Full backup exported ({} bytes)", sealed.len());
        Ok(sealed)
    }

    /// Import a full backup.
    ///
    /// Simple approach:
    /// 1. Decrypt bundle with export password
    /// 2. Extract the secret key from the bundle
    /// 3. Store secret key in OS keychain
    /// 4. Write vault_identity.json with FULL sync config
    /// 5. Create local vault DBs as fallback
    /// 6. Return — frontend restarts the app
    ///
    /// On restart, `auto_unlock()` reads identity → gets key from keychain →
    /// connects to Turso (if sync enabled) → everything works.
    #[tracing::instrument(skip(self, data, export_password, _master_password))]
    pub async fn import_full_backup(
        &mut self,
        data: &[u8],
        export_password: &str,
        _master_password: &str,
    ) -> Result<String, VaultError> {
        use crate::vault::export::*;

        // Step 1: Decrypt bundle
        let bundle = unseal_bundle(data, export_password)?;
        tracing::info!("Bundle decrypted: {} vaults, sync={}", bundle.vaults.len(), bundle.sync_config.is_some());

        // Step 2: Extract secret key from bundle
        let secret_key_b64 = bundle.identity.secret_key.as_ref().ok_or_else(|| {
            VaultError::EncryptionError(
                "Backup does not contain the secret key. Please re-export from the source machine.".to_string(),
            )
        })?;
        let secret_key_bytes = BASE64
            .decode(secret_key_b64)
            .map_err(|e| VaultError::SerializationError(e.to_string()))?;
        if secret_key_bytes.len() != 32 {
            return Err(VaultError::InvalidKeyLength {
                expected: 32,
                got: secret_key_bytes.len(),
            });
        }
        tracing::info!("Secret key extracted from bundle (32 bytes)");

        // The identity being replaced keeps its key in the OS keychain: a
        // restore of the wrong backup must leave a way back to it. Only an
        // explicit reset removes a key.

        // Step 3: Clear in-memory state
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

        // Delete old identity file
        let identity_path = self.app_dir.join("vault_identity.json");
        if identity_path.exists() {
            tokio::fs::remove_file(&identity_path).await?;
        }

        // Step 4: Store secret key in OS keychain (for auto_unlock on restart).
        // Only a convenience, as everywhere else the key is stored: without
        // it Reach asks for the master password at the next start. A
        // credential store that refuses (Windows answered error 8 to a
        // background process) must not make a good backup unrestorable.
        match store_key_in_keychain(&bundle.identity.user_uuid, &secret_key_bytes) {
            Ok(()) => tracing::info!("Secret key stored in OS keychain for user {}", bundle.identity.user_uuid),
            Err(e) => tracing::warn!("Backup restored without the OS keychain ({e}); the master password opens it"),
        }

        // Step 5: Build internal_vault_ids from bundle
        let mut internal_vault_ids = HashMap::new();
        let mut unified_vault_id: Option<String> = None;
        for exported_vault in &bundle.vaults {
            if exported_vault.is_internal {
                if exported_vault.name == "__personal__" {
                    unified_vault_id = Some(exported_vault.vault_id.clone());
                }
                internal_vault_ids
                    .insert(exported_vault.name.clone(), exported_vault.vault_id.clone());
            }
        }
        // If backup had unified vault, map all internal names to it
        if let Some(ref uid) = unified_vault_id {
            for name in INTERNAL_VAULTS {
                internal_vault_ids.insert(name.to_string(), uid.clone());
            }
        }

        // Step 6: Build user_vaults WITH sync URLs
        let user_vaults: Vec<StoredVaultRef> = if let Some(ref sc) = bundle.sync_config {
            sc.user_vaults
                .iter()
                .map(|v| StoredVaultRef {
                    id: v.id.clone(),
                    name: v.name.clone(),
                    vault_type: v.vault_type.clone(),
                    sync_url: v.sync_url.clone(),
                    sync_token: v.sync_token.clone(),
                    managed_grant: None,
                })
                .collect()
        } else {
            bundle
                .vaults
                .iter()
                .filter(|v| !v.is_internal)
                .map(|v| StoredVaultRef {
                    id: v.vault_id.clone(),
                    name: v.name.clone(),
                    vault_type: v.vault_type.clone(),
                    sync_url: None,
                    sync_token: None,
                    managed_grant: None,
                })
                .collect()
        };

        // Step 7: Write vault_identity.json with FULL sync config, its
        // tokens sealed under the restored identity key.
        let tokens = sync_tokens::SyncTokens {
            personal: bundle.sync_config.as_ref().and_then(|sc| sc.personal_sync_token.clone()),
            vaults: user_vaults
                .iter()
                .filter_map(|v| Some((v.id.clone(), v.sync_token.clone()?)))
                .collect(),
        };
        let sealed_tokens = if tokens.is_empty() {
            None
        } else {
            let mut secret = zeroize::Zeroizing::new([0u8; 32]);
            secret.copy_from_slice(&secret_key_bytes);
            Some(sync_tokens::seal(&secret, &bundle.identity.user_uuid, &tokens)?)
        };
        let user_vaults: Vec<StoredVaultRef> = user_vaults
            .into_iter()
            .map(|v| StoredVaultRef { sync_token: None, managed_grant: None, ..v })
            .collect();
        let stored = StoredIdentity {
            user_uuid: bundle.identity.user_uuid.clone(),
            salt: bundle.identity.salt.clone(),
            encrypted_key: bundle.identity.encrypted_key.clone(),
            nonce: bundle.identity.nonce.clone(),
            public_key: bundle.identity.public_key.clone(),
            personal_sync_url: bundle.sync_config.as_ref().and_then(|sc| sc.personal_sync_url.clone()),
            personal_sync_token: None,
            internal_vault_ids,
            user_vaults,
            sealed_tokens,
            // A restored identity on another machine is another device.
            device_id: None,
        };

        let identity_json = serde_json::to_string_pretty(&stored)?;
        private_fs::private_dir(&self.app_dir)?;
        private_fs::write_async(&identity_path, identity_json.into_bytes()).await?;
        tracing::info!("Identity file written with full sync config");

        // Step 8: Create local vault DBs from bundle (fallback for offline / non-sync)
        let vault_dir = self.app_dir.join("vaults");
        private_fs::private_dir(&vault_dir)?;

        for exported_vault in &bundle.vaults {
            let db_path = vault_dir.join(format!("{}.db", exported_vault.vault_id));

            // If DB file already exists, remove it
            if db_path.exists() {
                let _ = tokio::fs::remove_file(&db_path).await;
                let _ = tokio::fs::remove_file(vault_dir.join(format!("{}.db-wal", exported_vault.vault_id))).await;
                let _ = tokio::fs::remove_file(vault_dir.join(format!("{}.db-shm", exported_vault.vault_id))).await;
            }

            let db = crate::vault::sync::create_replica(&db_path, None).await?;
            let conn = db.connect().map_err(|e| VaultError::DatabaseError(e.to_string()))?;
            crate::vault::schema::init_schema(&conn).await?;

            // Insert vault header
            let vault_salt = BASE64
                .decode(&exported_vault.header.salt)
                .map_err(|e| VaultError::SerializationError(e.to_string()))?;

            conn.execute(
                "INSERT INTO vault_header (id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek) VALUES (?, ?, ?, ?, ?, ?, ?)",
                (
                    exported_vault.header.id.as_str(),
                    exported_vault.header.name.as_str(),
                    vault_salt.as_slice(),
                    exported_vault.header.user_uuid.as_str(),
                    exported_vault.header.created_at,
                    exported_vault.header.vault_type.as_str(),
                    exported_vault.header.wrapped_master_dek.as_str(),
                ),
            ).await?;

            for secret in &exported_vault.secrets {
                let secret_nonce = BASE64
                    .decode(&secret.nonce)
                    .map_err(|e| VaultError::SerializationError(e.to_string()))?;
                let ct_bytes = BASE64
                    .decode(&secret.ciphertext)
                    .map_err(|e| VaultError::SerializationError(e.to_string()))?;

                conn.execute(
                    "INSERT INTO secrets (id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        secret.id.as_str(),
                        secret.name.as_str(),
                        secret.category.as_str(),
                        secret_nonce.as_slice(),
                        ct_bytes.as_slice(),
                        secret.wrapped_dek_json.as_str(),
                        secret.created_at,
                        secret.updated_at,
                    ),
                ).await?;
            }

            for member in &exported_vault.members {
                let pk_bytes = BASE64
                    .decode(&member.public_key)
                    .map_err(|e| VaultError::SerializationError(e.to_string()))?;
                let inviter_pk = member.inviter_public_key.as_deref().unwrap_or("");

                conn.execute(
                    "INSERT INTO vault_members (user_uuid, public_key, wrapped_master_dek, role, added_at, inviter_public_key) VALUES (?, ?, ?, ?, ?, ?)",
                    (
                        member.user_uuid.as_str(),
                        pk_bytes.as_slice(),
                        member.wrapped_master_dek_json.as_str(),
                        member.role.as_str(),
                        member.added_at,
                        inviter_pk,
                    ),
                ).await?;
            }

            tracing::info!(
                "Restored vault DB: {} ({}, {} secrets, {} members)",
                exported_vault.vault_id,
                exported_vault.name,
                exported_vault.secrets.len(),
                exported_vault.members.len(),
            );
        }

        tracing::info!("Backup import complete. App should restart now.");
        Ok(bundle.identity.user_uuid)
    }
}

// ==================== HELPER FUNCTIONS ====================

/// Generate a random 32-byte salt.
fn generate_salt() -> [u8; 32] {
    let mut salt = [0u8; 32];
    rand::fill(&mut salt[..]);
    salt
}

/// Get current timestamp.
fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Derive KEK from X25519 secret key using HKDF (TLS-style).
fn derive_kek_from_secret_key(secret_key: &[u8], salt: &[u8; 32]) -> Result<Kek, VaultError> {
    let hk = Hkdf::<Sha256>::new(Some(salt), secret_key);
    Kek::try_from_fn(|kek| hk.expand(b"reach-vault-kek", kek).map_err(|_| VaultError::KeyDerivationFailed))
}

/// Derive KEK from password using Argon2id.
fn derive_kek_from_password(password: &[u8], salt: &[u8; 32]) -> Result<Kek, VaultError> {
    use argon2::{Algorithm, Argon2, Params, Version};

    let params = Params::new(65536, 3, 4, Some(32)).map_err(|e| VaultError::KdfError(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    Kek::try_from_fn(|kek| {
        argon2
            .hash_password_into(password, salt, kek)
            .map_err(|e| VaultError::KdfError(e.to_string()))
    })
}

/// Encrypt data with password-derived KEK.
fn encrypt_with_password(kek: &Kek, plaintext: &[u8]) -> Result<(String, String), VaultError> {
    use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};

    let cipher = kek.with_key(|k| XChaCha20Poly1305::new(k.into()));
    let mut nonce = [0u8; 24];
    rand::fill(&mut nonce[..]);
    let xnonce = &XNonce::from(nonce);

    let ciphertext = cipher
        .encrypt(xnonce, plaintext)
        .map_err(|e| VaultError::EncryptionError(e.to_string()))?;

    Ok((BASE64.encode(ciphertext), BASE64.encode(nonce)))
}

/// Decrypt data with password-derived KEK.
fn decrypt_with_password(kek: &Kek, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, VaultError> {
    use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};

    let cipher = kek.with_key(|k| XChaCha20Poly1305::new(k.into()));
    let xnonce = &XNonce::try_from(nonce)
        .map_err(|_| VaultError::DecryptionError("Invalid nonce length".to_string()))?;

    cipher
        .decrypt(xnonce, ciphertext)
        .map_err(|e| VaultError::DecryptionError(e.to_string()))
}

/// The vault's entry in the OS credential store, which is set up on first use.
/// The names match what keyring 3 wrote, so a key saved by an earlier Reach is
/// still found: `{user}.{service}` in the Windows Credential Manager, service
/// and account in the macOS login keychain, `keyring-rs:{user}@{service}` in
/// the Linux kernel keyring. Android has none of these: see `android_keychain`.
// In tests every platform, Android too, keeps keys in the in-memory store of
// keychain_entry_in: there is no app around to reach the Android Keystore.
#[cfg(any(not(target_os = "android"), test))]
fn keychain_entry(user_uuid: &str) -> Result<keyring_core::Entry, VaultError> {
    keychain_entry_in("reachercm-vault", user_uuid)
}

/// An entry under `service` in the same OS credential store.
pub(crate) fn keychain_entry_in(service: &str, user: &str) -> Result<keyring_core::Entry, VaultError> {
    static STORE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    STORE
        .get_or_init(|| {
            // Tests never touch the real OS keychain: every test identity
            // left its key in the Windows Credential Manager, until its
            // storage was full (853 entries) and the user's own Reach could
            // no longer store its key. keyring-core's sample store is made
            // for exactly this: in memory, kept by service and user.
            #[cfg(test)]
            let store = keyring_core::sample::Store::new();
            #[cfg(all(not(test), target_os = "windows"))]
            let store = windows_native_keyring_store::Store::new();
            #[cfg(all(not(test), target_os = "macos"))]
            let store = apple_native_keyring_store::keychain::Store::new();
            #[cfg(all(not(test), target_os = "linux"))]
            let store = super::linux_keychain::Store::new();
            // iOS has one keychain, the data protection one; items stay on
            // this device (no iCloud sync) in the app's own access group.
            #[cfg(all(not(test), target_os = "ios"))]
            let store = apple_native_keyring_store::protected::Store::new();
            #[cfg(all(not(test), not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "ios"))))]
            let store = keyring_core::mock::Store::new();
            store.map(|s| keyring_core::set_default_store(s)).map_err(|e| e.to_string())
        })
        .clone()
        .map_err(VaultError::KeychainUnavailable)?;
    keyring_core::Entry::new(service, user).map_err(|e| VaultError::KeychainError(e.to_string()))
}

/// Android's keychain: the Android Keystore, through Reach's plugin. Android
/// has no store the keyring crates can use, so the key used to live in memory
/// only: after a restart the vault could not open without its password, and
/// not at all if it had none. Now an AES-256-GCM key in the Keystore, which
/// never leaves it, seals the vault key, and the sealed copy lives in Reach's
/// private app folder. Like a desktop keychain it asks for no user check;
/// that is the fingerprint's job.
#[cfg(all(target_os = "android", not(test)))]
pub(crate) mod android_keychain {
    use super::{VaultError, BASE64};
    use base64::Engine;
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::path::PathBuf;
    use tauri::Manager;
    use tauri_plugin_reach_unlock::Unlock;

    #[derive(Serialize, Deserialize)]
    struct Sealed {
        iv: String,
        ciphertext: String,
    }

    #[derive(Deserialize)]
    struct Opened {
        secret: String,
    }

    fn alias(user_uuid: &str) -> String {
        format!("reach-vault-key-{user_uuid}")
    }

    fn path(user_uuid: &str) -> PathBuf {
        crate::app_data_dir().join("keystore").join(format!("{user_uuid}.json"))
    }

    fn call<T: serde::de::DeserializeOwned>(method: &str, payload: serde_json::Value) -> Result<T, VaultError> {
        let app = crate::android_app().ok_or_else(|| VaultError::KeychainUnavailable("Reacher CM is still starting".into()))?;
        app.state::<Unlock<tauri::Wry>>()
            .call_blocking(method, payload)
            .map_err(VaultError::KeychainUnavailable)
    }

    pub fn store(user_uuid: &str, key: &[u8]) -> Result<(), VaultError> {
        let sealed: Sealed = call("keystoreSeal", json!({ "alias": alias(user_uuid), "secret": BASE64.encode(key) }))?;
        let path = path(user_uuid);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&sealed)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn get(user_uuid: &str) -> Result<Vec<u8>, VaultError> {
        let data = match std::fs::read(path(user_uuid)) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(VaultError::KeychainKeyMissing),
            Err(e) => return Err(e.into()),
        };
        let sealed: Sealed = serde_json::from_slice(&data)?;
        let opened: Opened = call(
            "keystoreOpen",
            json!({ "alias": alias(user_uuid), "iv": sealed.iv, "ciphertext": sealed.ciphertext }),
        )?;
        BASE64
            .decode(opened.secret)
            .map_err(|e| VaultError::SerializationError(e.to_string()))
    }

    pub fn delete(user_uuid: &str) -> Result<(), VaultError> {
        match std::fs::remove_file(path(user_uuid)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let _ = call::<serde_json::Value>("keystoreForget", json!({ "alias": alias(user_uuid) }));
        Ok(())
    }
}

/// Store key in OS keychain.
fn store_key_in_keychain(user_uuid: &str, key: &[u8]) -> Result<(), VaultError> {
    #[cfg(all(target_os = "android", not(test)))]
    {
        android_keychain::store(user_uuid, key)
    }
    #[cfg(any(not(target_os = "android"), test))]
    {
        let entry = keychain_entry(user_uuid)?;
        entry
            .set_password(&BASE64.encode(key))
            .map_err(|e| VaultError::KeychainError(e.to_string()))?;
        Ok(())
    }
}

/// Remove the key from the OS keychain; a key already gone is fine.
fn delete_key_from_keychain(user_uuid: &str) -> Result<(), VaultError> {
    #[cfg(all(target_os = "android", not(test)))]
    {
        android_keychain::delete(user_uuid)
    }
    #[cfg(any(not(target_os = "android"), test))]
    match keychain_entry(user_uuid)?.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(e) => Err(VaultError::KeychainError(e.to_string())),
    }
}

/// Get key from OS keychain.
#[cfg(all(target_os = "android", not(test)))]
fn get_key_from_keychain(user_uuid: &str) -> Result<Vec<u8>, VaultError> {
    android_keychain::get(user_uuid)
}

/// Get key from OS keychain.
#[cfg(any(not(target_os = "android"), test))]
fn get_key_from_keychain(user_uuid: &str) -> Result<Vec<u8>, VaultError> {
    let entry = keychain_entry(user_uuid)?;
    let password = entry.get_password().map_err(|e| match e {
        keyring_core::Error::NoEntry => VaultError::KeychainKeyMissing,
        keyring_core::Error::NoStorageAccess(ref inner) => {
            VaultError::KeychainUnavailable(inner.to_string())
        }
        keyring_core::Error::PlatformFailure(ref inner) => {
            VaultError::KeychainUnavailable(inner.to_string())
        }
        other => VaultError::KeychainError(other.to_string()),
    })?;
    BASE64
        .decode(&password)
        .map_err(|e| VaultError::SerializationError(e.to_string()))
}

#[cfg(test)]
#[path = "manager_connection_tests.rs"]
mod connection_tests;

#[cfg(test)]
#[path = "manager_password_tests.rs"]
mod password_tests;

#[cfg(test)]
#[path = "manager_duplicate_tests.rs"]
mod duplicate_tests;

#[cfg(test)]
#[path = "manager_backup_tests.rs"]
mod backup_tests;

#[cfg(test)]
#[path = "manager_keychain_tests.rs"]
mod keychain_tests;

#[cfg(test)]
#[path = "manager_copied_ids_tests.rs"]
mod copied_ids_tests;

#[cfg(test)]
#[path = "manager_sync_tokens_tests.rs"]
mod sync_tokens_tests;

#[path = "manager_device_link.rs"]
pub mod device_link;

#[cfg(test)]
#[path = "manager_device_link_tests.rs"]
mod device_link_tests;

#[cfg(desktop)]
#[path = "manager_managed_access.rs"]
mod managed_access;
#[path = "manager_account_sync.rs"]
mod account_sync;

#[cfg(desktop)]
#[path = "manager_org_sync.rs"]
mod organization_sync;
#[cfg(desktop)]
pub use organization_sync::{Organization, OrganizationVaultGrant, SharedOrganizationVault};
#[cfg(desktop)]
pub(super) use organization_sync::valid_organization_vault_name;
