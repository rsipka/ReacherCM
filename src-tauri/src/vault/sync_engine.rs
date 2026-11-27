//! The sync engine: exchanges a vault's changes with its backend.
//!
//! A vault always lives in its local file. A sync reads what the backend has
//! that this device has not seen, merges it into the file record by record,
//! then sends this device's changes. Merging is the same for every backend:
//!
//! - a record changed on one side only takes that side's version;
//! - changed on both, the newer change wins everywhere (ties go to the device
//!   id); if a session or credential loses, the version that lost is kept as
//!   a "conflict copy" beside it, so no edit disappears without a trace;
//! - deleted on one side and edited on the other, the edit wins;
//! - a copy of a record older than the one already seen is ignored, so a
//!   backend cannot roll a record back.
//!
//! Turso is the first backend, in one of two formats. A database created by
//! an older Reach keeps its `secrets` table, readable by older versions, and
//! is synced as that table ("legacy"): Turso sees names and categories, as
//! before. Upgrading it moves every secret into `reach_records`, one sealed
//! record each (see [`crate::vault::sync_record`]), with a sequence number
//! that lets a device ask only for what changed since its last sync. The
//! upgrade is the user's choice, because older Reach versions cannot read
//! the new format.
//!
//! The other backends hold the vault as one file, in a folder, on an SFTP
//! server or in a Git repository; see [`crate::vault::sync_bundle`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use libsql::{Connection, TransactionBehavior};
use secrecy::ExposeSecret;

use crate::vault::crypto::{decrypt_secret, encrypt_secret};
use crate::vault::error::VaultError;
use crate::vault::sync::{create_replica, SyncConfig};
use crate::vault::sync_local::{self, Dirty, Row};
use crate::vault::sync_record::{Record, RecordKey};
use crate::vault::sync_store::{GitTarget, SftpTarget, SmbTarget, Store};
use crate::vault::types::{Dek, EncryptedPayload, WrappedDek};

/// How long one sync of one vault may take in all.
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

/// How often a push is retried when another device pushed first.
const PUSH_ATTEMPTS: usize = 4;

/// The device id `legacy` rows carry: they have no device of their own.
const LEGACY_DEVICE: &str = "legacy";

/// `sync_state` keys.
pub const STATE_FORMAT: &str = "format";
pub const STATE_CURSOR: &str = "cursor";
pub const STATE_LAST_SYNC: &str = "last_sync";
pub const STATE_LAST_ERROR: &str = "last_error";
pub const STATE_CONFLICTS: &str = "conflicts";
pub const STATE_BACKEND: &str = "backend";

/// How often a vault on a file backend is read when nothing changed here.
/// Each read is a connection to a server or a fetch; Turso is read every
/// minute.
const FILE_POLL_MS: i64 = 5 * 60 * 1000;

/// The Git branch a vault goes to when none is named.
pub const DEFAULT_GIT_BRANCH: &str = "reach-vault";

/// Where a vault syncs to.
#[derive(Clone)]
pub enum Backend {
    /// A Turso (libsql) database.
    Turso { url: String, token: String },
    /// A folder: on this disk, a mounted share, a UNC path.
    Folder { path: PathBuf },
    Sftp(SftpTarget),
    /// An SMB share, reached by Reach's own client.
    Smb(SmbTarget),
    Git(GitTarget),
    /// An S3 bucket, on Amazon or any S3-compatible store.
    S3(crate::vault::sync_s3::S3Target),
    Account(crate::vault::sync_account_store::AccountTarget),
}

/// What an SFTP backend's token holds: whatever logs in.
#[derive(serde::Deserialize, Default)]
struct SftpSecret {
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    passphrase: Option<String>,
}

impl Backend {
    /// The backend a vault's sync URL and token name:
    ///
    /// - `folder:<path>`
    /// - `sftp://user@host[:port]/path?hostkey=SHA256:…`, where `/~/path` is
    ///   under the login folder, with a token of JSON `{password, key,
    ///   passphrase}` (or just the password);
    /// - `smb://[domain;]user@server[:port]/share/folder`, with the password
    ///   as token;
    /// - `git:<remote>[#branch]`, with an optional token for https;
    /// - `s3://host[:port]/bucket/folder[?region=..&style=path|virtual]`
    ///   (`s3+http://` on this computer only), with a token of JSON
    ///   `{access_key, secret_key}`;
    /// - anything else is a Turso URL.
    pub fn parse(url: &str, token: &str, app_dir: &Path) -> Result<Backend, VaultError> {
        let url = url.trim();
        if url.starts_with(crate::vault::sync_account_store::PREFIX)
            || url.starts_with(crate::vault::sync_account_store::ORG_PREFIX)
        {
            return crate::vault::sync_account_store::AccountTarget::parse(url, token).map(Backend::Account);
        }
        if let Some(path) = url.strip_prefix("folder:") {
            let path = PathBuf::from(path.trim());
            if !path.is_absolute() {
                return Err(VaultError::SyncError("The sync folder must be a full path".into()));
            }
            return Ok(Backend::Folder { path });
        }
        if url.len() >= 7 && url[..7].eq_ignore_ascii_case("sftp://") {
            let parsed = url::Url::parse(url).map_err(|e| VaultError::SyncError(format!("SFTP address: {e}")))?;
            let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or_else(|| VaultError::SyncError("SFTP address has no host".into()))?;
            let user = percent(parsed.username());
            if user.is_empty() {
                return Err(VaultError::SyncError("SFTP address has no user name".into()));
            }
            let raw_path = percent(parsed.path());
            let path = match raw_path.strip_prefix("/~") {
                Some(rest) => rest.trim_start_matches('/').to_string(),
                None => raw_path.clone(),
            };
            let host_key = parsed.query_pairs().find(|(k, _)| k == "hostkey").map(|(_, v)| v.into_owned());
            let secret = if token.trim_start().starts_with('{') {
                serde_json::from_str::<SftpSecret>(token).map_err(|_| VaultError::SyncError("SFTP credentials are damaged".into()))?
            } else {
                SftpSecret { password: Some(token.to_string()).filter(|t| !t.is_empty()), ..Default::default() }
            };
            return Ok(Backend::Sftp(SftpTarget {
                host: host.trim_start_matches('[').trim_end_matches(']').to_string(),
                port: parsed.port().unwrap_or(22),
                user,
                path,
                host_key,
                password: secret.password,
                key: secret.key,
                passphrase: secret.passphrase,
            }));
        }
        if url.len() >= 6 && url[..6].eq_ignore_ascii_case("smb://") {
            let parsed = url::Url::parse(url).map_err(|e| VaultError::SyncError(format!("SMB address: {e}")))?;
            let server = parsed.host_str().filter(|h| !h.is_empty()).ok_or_else(|| VaultError::SyncError("SMB address has no server".into()))?;
            let raw_user = percent(parsed.username());
            let user = match raw_user.split_once(';') {
                Some((domain, user)) => format!("{domain}\\{user}"),
                None => raw_user,
            };
            if user.is_empty() {
                return Err(VaultError::SyncError("SMB address has no user name".into()));
            }
            let path = percent(parsed.path());
            let mut parts = path.split('/').filter(|p| !p.is_empty());
            let share = parts.next().ok_or_else(|| VaultError::SyncError("SMB address names no share".into()))?.to_string();
            let folder = parts.collect::<Vec<_>>().join("\\");
            return Ok(Backend::Smb(SmbTarget {
                server: server.trim_start_matches('[').trim_end_matches(']').to_string(),
                port: parsed.port().unwrap_or(445),
                share,
                path: folder,
                user,
                password: token.to_string(),
            }));
        }
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("s3://") || lower.starts_with("s3+http://") {
            return Ok(Backend::S3(crate::vault::sync_s3::S3Target::parse(url, token)?));
        }
        if let Some(rest) = url.strip_prefix("git:") {
            let (remote, branch) = match rest.rsplit_once('#') {
                Some((r, b)) if !b.trim().is_empty() => (r.trim(), b.trim()),
                _ => (rest.trim().trim_end_matches('#'), DEFAULT_GIT_BRANCH),
            };
            if remote.is_empty() {
                return Err(VaultError::SyncError("The Git repository address is empty".into()));
            }
            // Git would read a leading `-` as an option, and a `name::`
            // prefix (`ext::`, `fd::`) as a helper to run.
            let helper = remote
                .split_once("::")
                .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)));
            if remote.starts_with('-') || helper {
                return Err(VaultError::SyncError("That Git repository address is not valid".into()));
            }
            if branch.starts_with('-') || branch.contains("..") || branch.contains(char::is_whitespace) {
                return Err(VaultError::SyncError("That Git branch name is not valid".into()));
            }
            if !token.is_empty() {
                // The token goes with every request.
                crate::vault::sync::check_sync_url(remote)?;
            }
            let id: String = sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(format!("{remote}#{branch}")))
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect();
            return Ok(Backend::Git(GitTarget {
                remote: remote.to_string(),
                branch: branch.to_string(),
                token: Some(token.to_string()).filter(|t| !t.is_empty()),
                work_dir: app_dir.join("sync-git").join(id),
            }));
        }
        crate::vault::sync::check_sync_url(url)?;
        Ok(Backend::Turso { url: url.to_string(), token: token.to_string() })
    }

    /// What kind of backend this is, as the interface names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Backend::Turso { .. } => "turso",
            Backend::Folder { .. } => "folder",
            Backend::Sftp(_) => "sftp",
            Backend::Smb(_) => "smb",
            Backend::Git(_) => "git",
            Backend::S3(_) => "s3",
            Backend::Account(_) => "account",
        }
    }

    /// What names this backend, to notice when a vault is pointed at
    /// another one. Never holds a credential.
    pub fn identity(&self) -> String {
        match self {
            Backend::Turso { url, .. } => {
                let url = url.trim().trim_end_matches('/').to_ascii_lowercase();
                let host = ["libsql://", "https://", "wss://", "http://", "ws://"]
                    .iter()
                    .find_map(|p| url.strip_prefix(p))
                    .unwrap_or(&url)
                    .to_string();
                format!("turso:{host}")
            }
            Backend::Folder { path } => format!("folder:{}", path.display()),
            Backend::Sftp(t) => format!("sftp:{}@{}:{}/{}", t.user, t.host.to_ascii_lowercase(), t.port, t.path),
            Backend::Smb(t) => format!("smb:{}@{}:{}/{}/{}", t.user.to_ascii_lowercase(), t.server.to_ascii_lowercase(), t.port, t.share.to_ascii_lowercase(), t.path.to_ascii_lowercase()),
            Backend::Git(t) => format!("git:{}#{}", t.remote, t.branch),
            Backend::S3(t) => t.identity(),
            Backend::Account(t) => t.identity(),
        }
    }

    /// The store of a backend that holds a file; `None` for Turso.
    pub async fn store(&self) -> Result<Option<Store>, VaultError> {
        Ok(Some(match self {
            Backend::Turso { .. } => return Ok(None),
            Backend::Folder { path } => Store::folder(path).await?,
            Backend::Sftp(t) => Store::sftp(t).await?,
            Backend::Smb(t) => Store::smb(t).await?,
            Backend::Git(t) => Store::git(t).await?,
            Backend::S3(t) => Store::S3(crate::vault::sync_s3::S3Store::open(t)?),
            Backend::Account(t) => Store::Account(t.clone()),
        }))
    }

    /// The vault header and members the backend holds, if it holds a vault.
    pub async fn read_header(&self) -> Result<Option<crate::vault::sync_bundle::HeaderRows>, VaultError> {
        match self {
            Backend::Turso { url, token } => {
                let remote = connect_turso(url, token).await?;
                match turso::read_header(&remote).await {
                    Ok(rows) => Ok(Some(rows)),
                    Err(_) if !turso::has_header(&remote).await => Ok(None),
                    Err(e) => Err(e),
                }
            }
            _ => {
                let mut store = self.store().await?.expect("a file backend");
                match store.get().await? {
                    Some((data, _)) => Ok(Some(crate::vault::sync_bundle::read_header(&data)?)),
                    None => Ok(None),
                }
            }
        }
    }
}

/// `%xx` in part of a URL, decoded.
fn percent(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// Everything one sync of one vault needs, gathered while the vault manager
/// is held, so the sync itself runs without it.
pub struct Target {
    pub vault_id: String,
    pub local_path: PathBuf,
    pub backend: Backend,
    pub master: Dek,
    pub key: RecordKey,
    pub device: String,
    pub my_uuid: String,
}

/// What a sync did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Records from the backend applied here.
    pub pulled: usize,
    /// Changes of this device sent.
    pub pushed: usize,
    /// Conflict copies made.
    pub conflicts: usize,
    /// Whether the backend is still in the legacy format.
    pub legacy: bool,
    pub(super) applied_revision: Option<String>,
}

impl Report {
    pub fn changed(&self) -> bool {
        self.pulled > 0 || self.conflicts > 0
    }
}

/// Open the vault's local file for the engine, apart from the connection the
/// app uses, and willing to wait for it.
pub async fn open_local(path: &std::path::Path) -> Result<Connection, VaultError> {
    let db = libsql::Builder::new_local(path)
        .build()
        .await
        .map_err(|e| VaultError::DatabaseError(e.to_string()))?;
    let conn = db.connect().map_err(|e| VaultError::DatabaseError(e.to_string()))?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|e| VaultError::DatabaseError(e.to_string()))?;
    Ok(conn)
}

/// A connection to a Turso database.
pub async fn connect_turso(url: &str, token: &str) -> Result<Connection, VaultError> {
    // Tests stand a local file in for the server: the SQL is the same.
    #[cfg(test)]
    if let Some(path) = url.strip_prefix("file:") {
        return open_local(std::path::Path::new(path)).await;
    }
    let config = SyncConfig { sync_url: url.to_string(), auth_token: token.to_string() };
    let db = create_replica(std::path::Path::new(""), Some(&config)).await?;
    db.connect().map_err(|e| VaultError::SyncError(e.to_string()))
}

/// Sync one vault, and note the result in its file either way.
pub async fn sync(target: &Target) -> Result<Report, VaultError> {
    let local = open_local(&target.local_path).await?;
    // Pointed at another backend than last time: what was exchanged with
    // the old one says nothing about the new one. Read it in full, and offer
    // it everything here.
    let backend = target.backend.identity();
    if sync_local::state(&local, STATE_BACKEND).await?.as_deref() != Some(backend.as_str()) {
        if sync_local::state(&local, STATE_BACKEND).await?.is_some() {
            tracing::info!(vault = %target.vault_id, "Vault moved to another backend; syncing it in full");
            sync_local::reset(&local).await?;
            sync_local::mark_all_dirty(&local).await?;
        }
        sync_local::set_state(&local, STATE_BACKEND, &backend).await?;
    }
    let result = match tokio::time::timeout(SYNC_TIMEOUT, sync_inner(target, &local)).await {
        Ok(result) => result,
        Err(_) => Err(VaultError::SyncError(format!("sync took longer than {} seconds", SYNC_TIMEOUT.as_secs()))),
    };
    match &result {
        Ok(report) => {
            sync_local::set_state(&local, STATE_LAST_SYNC, &now_ms().to_string()).await?;
            sync_local::clear_state(&local, STATE_LAST_ERROR).await?;
            if report.conflicts > 0 {
                let before = sync_local::state(&local, STATE_CONFLICTS).await?.and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                sync_local::set_state(&local, STATE_CONFLICTS, &(before + report.conflicts).to_string()).await?;
            }
        }
        Err(e) => {
            sync_local::set_state(&local, STATE_LAST_ERROR, &e.to_string()).await?;
        }
    }
    result
}

/// Sync and report the applied revision for an active account vault.
pub async fn sync_with_receipt(target: &Target) -> Result<Report, VaultError> {
    let report = sync(target).await?;
    let report = report_applied(target, report).await;
    Ok(report)
}

pub(super) async fn report_applied(target: &Target, mut report: Report) -> Report {
    if let (Backend::Account(account), Some(revision)) = (&target.backend, report.applied_revision.take()) {
        account.report_applied(&revision).await;
    }
    report
}

async fn sync_inner(target: &Target, local: &Connection) -> Result<Report, VaultError> {
    match &target.backend {
        Backend::Turso { url, token } => {
            let remote = connect_turso(url, token).await?;
            turso::sync(target, local, &remote).await
        }
        backend => {
            let mut store = backend.store().await?.expect("a file backend");
            crate::vault::sync_bundle::sync(target, local, &mut store).await
        }
    }
}

/// Whether a timed sync should run now. A file backend is read every few
/// minutes, or as soon as this device has a change to send.
pub async fn due(target: &Target) -> bool {
    if matches!(target.backend, Backend::Turso { .. }) {
        return true;
    }
    let Ok(local) = open_local(&target.local_path).await else { return true };
    if sync_local::dirty_count(&local).await.unwrap_or(1) > 0 {
        return true;
    }
    if sync_local::state(&local, STATE_BACKEND).await.ok().flatten().as_deref() != Some(target.backend.identity().as_str()) {
        return true;
    }
    let last = sync_local::state(&local, STATE_LAST_SYNC).await.ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let failed = sync_local::state(&local, STATE_LAST_ERROR).await.ok().flatten().is_some();
    failed || now_ms() - last >= FILE_POLL_MS
}

/// Try a backend before it is used: whether it can be reached, and whether
/// it already holds a vault (and whose).
pub async fn probe(backend: &Backend) -> Result<Option<String>, VaultError> {
    let header = tokio::time::timeout(SYNC_TIMEOUT, backend.read_header())
        .await
        .map_err(|_| VaultError::SyncError(format!("no answer within {} seconds", SYNC_TIMEOUT.as_secs())))??;
    Ok(header.and_then(|(h, _)| match h.first() {
        Some(libsql::Value::Text(id)) => Some(id.clone()),
        _ => None,
    }))
}

/// Move a legacy Turso vault to the sealed record format. Syncs first, so
/// nothing is lost, then moves the rows over in one transaction and keeps the
/// old table under another name.
pub async fn upgrade(target: &Target) -> Result<(), VaultError> {
    let Backend::Turso { url, token } = &target.backend else {
        return Ok(()); // only Turso has an old format
    };
    sync(target).await?;
    let local = open_local(&target.local_path).await?;
    let remote = connect_turso(url, token).await?;
    turso::upgrade(target, &local, &remote).await
}

/// Merge records from a backend into the local file. `full` says that
/// `incoming` is everything the backend holds, so a row here that is in none
/// of them, and was not changed here, was deleted elsewhere.
pub async fn merge(target: &Target, local: &Connection, incoming: Vec<Record>, full: bool) -> Result<Report, VaultError> {
    let dirty: HashMap<String, Dirty> = sync_local::dirty(local)
        .await?
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect();
    let mut report = Report::default();
    let mut seen = HashSet::new();

    for record in incoming {
        seen.insert(record.id.clone());
        if let Some(known) = sync_local::known(local, &record.id).await? {
            if record.version <= known {
                continue; // already here, or an older copy
            }
        }
        // A session's saved host keys are merged, not replaced: the copy
        // that wins may come from a device that did not have a key yet.
        let mine = match &record.row {
            Some(r) if is_session(&r.category) => sync_local::read_row(local, &record.id).await?,
            _ => None,
        };
        match dirty.get(&record.id) {
            None => {
                sync_local::apply(local, &record.id, record.row.as_ref()).await?;
                if let (Some(theirs), Some(mine)) = (&record.row, &mine) {
                    keep_host_keys(local, &target.master, theirs, mine).await?;
                }
                report.pulled += 1;
            }
            Some(change) => {
                let remote_wins = match (change.deleted, record.row.is_some()) {
                    (true, true) => true,   // deleted here, edited there: the edit wins
                    (false, false) => false, // edited here, deleted there: the edit wins
                    _ => record.newer_than(change.changed_at, &target.device),
                };
                if remote_wins {
                    if !change.deleted && record.row.is_some() {
                        if let Some(mine) = sync_local::read_row(local, &record.id).await? {
                            if keeps_conflict_copy(&mine.category) && conflict_copy(local, &target.master, &mine).await? {
                                report.conflicts += 1;
                            }
                        }
                    }
                    sync_local::apply(local, &record.id, record.row.as_ref()).await?;
                    local.execute("DELETE FROM sync_dirty WHERE id = ?", [record.id.as_str()]).await?;
                    if let (Some(theirs), Some(mine)) = (&record.row, &mine) {
                        keep_host_keys(local, &target.master, theirs, mine).await?;
                    }
                    report.pulled += 1;
                } else if let (Some(theirs), Some(mine)) = (&record.row, &mine) {
                    // This device's version wins; the keys of the other stay.
                    keep_host_keys(local, &target.master, mine, theirs).await?;
                }
            }
        }
        sync_local::set_known(local, &record.id, record.version).await?;
    }

    if full {
        // Read again: the conflict copies just made are changes of this
        // device, not rows deleted elsewhere.
        let dirty: HashSet<String> = sync_local::dirty(local).await?.into_iter().map(|d| d.id).collect();
        for row in sync_local::all_rows(local).await? {
            if !seen.contains(&row.id) && !dirty.contains(&row.id) {
                sync_local::apply(local, &row.id, None).await?;
                local.execute("DELETE FROM sync_known WHERE id = ?", [row.id.as_str()]).await?;
                report.pulled += 1;
            }
        }
    }
    Ok(report)
}

/// The changes waiting here, as records to send: each with a version newer
/// than any the backend has seen of it.
pub async fn outgoing(target: &Target, local: &Connection) -> Result<Vec<(Dirty, Record)>, VaultError> {
    let mut out = Vec::new();
    for change in sync_local::dirty(local).await? {
        let row = if change.deleted { None } else { sync_local::read_row(local, &change.id).await? };
        let known = sync_local::known(local, &change.id).await?.unwrap_or(0);
        let record = Record {
            id: change.id.clone(),
            version: change.changed_at.max(known + 1),
            device: target.device.clone(),
            row,
        };
        out.push((change, record));
    }
    Ok(out)
}

/// After a push: note what the backend now has, and forget the changes sent,
/// unless they changed again meanwhile.
pub async fn sent(local: &Connection, sent: &[(Dirty, Record)]) -> Result<(), VaultError> {
    for (change, record) in sent {
        sync_local::set_known(local, &record.id, record.version).await?;
        sync_local::clear_dirty(local, &change.id, change.changed_at).await?;
    }
    Ok(())
}

fn is_session(category: &str) -> bool {
    matches!(category, "session" | "custom:session")
}

fn open_row(master: &Dek, row: &Row) -> Option<serde_json::Value> {
    let nonce = <[u8; 24]>::try_from(row.nonce.clone()).ok()?;
    let wrapped_dek = serde_json::from_str::<WrappedDek>(&row.wrapped_dek).ok()?;
    let payload = EncryptedPayload { nonce, ciphertext: row.ciphertext.clone(), wrapped_dek };
    let plain = decrypt_secret(master, &payload).ok()?;
    serde_json::from_slice(plain.expose_secret()).ok()
}

/// `kept`, the version of a session now in the local file, with the saved
/// host keys of `other`, another version of it, merged in (see
/// [`crate::ssh::session_hostkeys::merge_keys`]). When that changes it, it
/// is written as a change of this device, so the merged keys go out to the
/// other devices with the next push. A version that does not open is left
/// as it is.
async fn keep_host_keys(local: &Connection, master: &Dek, kept: &Row, other: &Row) -> Result<(), VaultError> {
    let (Some(mut value), Some(theirs)) = (open_row(master, kept), open_row(master, other)) else { return Ok(()) };
    if !crate::ssh::session_hostkeys::merge_into(&mut value, &theirs) {
        return Ok(());
    }
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&value)?);
    let sealed = encrypt_secret(master, &bytes)?;
    local
        .execute(
            "UPDATE secrets SET nonce = ?, ciphertext = ?, wrapped_dek = ?, updated_at = ? WHERE id = ?",
            (
                sealed.nonce.as_slice(),
                sealed.ciphertext.as_slice(),
                serde_json::to_string(&sealed.wrapped_dek)?,
                now_ms() / 1000,
                kept.id.as_str(),
            ),
        )
        .await?;
    tracing::info!(record = %kept.id, "Sync: kept saved host keys of the other copy of a session");
    Ok(())
}

fn keeps_conflict_copy(category: &str) -> bool {
    matches!(category, "session" | "custom:session" | "credential")
}

/// Keep this device's losing version of a record as a copy beside the winner:
/// a new id, and "(conflict copy …)" after its name, in the row and in the
/// payload's own `id` and `name` where it has them. Returns whether a copy was
/// made; one that cannot be read is left alone.
async fn conflict_copy(local: &Connection, master: &Dek, mine: &Row) -> Result<bool, VaultError> {
    let Ok(nonce) = <[u8; 24]>::try_from(mine.nonce.clone()) else { return Ok(false) };
    let Ok(wrapped_dek) = serde_json::from_str::<WrappedDek>(&mine.wrapped_dek) else { return Ok(false) };
    let payload = EncryptedPayload { nonce, ciphertext: mine.ciphertext.clone(), wrapped_dek };
    let Ok(plain) = decrypt_secret(master, &payload) else { return Ok(false) };

    let new_id = uuid::Uuid::new_v4().to_string();
    let suffix = format!(" (conflict copy {})", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let mut bytes = zeroize::Zeroizing::new(plain.expose_secret().clone());
    if let Ok(serde_json::Value::Object(mut obj)) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        if obj.get("id").and_then(|v| v.as_str()) == Some(mine.id.as_str()) {
            obj.insert("id".into(), serde_json::Value::String(new_id.clone()));
        }
        if let Some(serde_json::Value::String(name)) = obj.get_mut("name") {
            name.push_str(&suffix);
        }
        *bytes = serde_json::to_vec(&obj)?;
    }
    let sealed = encrypt_secret(master, &bytes)?;
    let now = now_ms() / 1000;
    local
        .execute(
            "INSERT INTO secrets (id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                new_id.as_str(),
                format!("{}{}", mine.name, suffix),
                mine.category.as_str(),
                sealed.nonce.as_slice(),
                sealed.ciphertext.as_slice(),
                serde_json::to_string(&sealed.wrapped_dek)?,
                now,
                now,
            ),
        )
        .await?;
    tracing::warn!(record = %mine.id, copy = %new_id, "Sync conflict: this device's version kept as a copy");
    Ok(true)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Copy the vault's header and member list from the backend into the local
/// file, which needs them to unlock without the network.
pub async fn mirror_header(local: &Connection, header: &[libsql::Value], members: &[Vec<libsql::Value>]) -> Result<(), VaultError> {
    local.execute(
        "INSERT OR REPLACE INTO vault_header (id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek) VALUES (?, ?, ?, ?, ?, ?, ?)",
        header.to_vec(),
    )
    .await?;
    local.execute("DELETE FROM vault_header WHERE id <> ?", [header[0].clone()]).await?;
    local.execute("DELETE FROM vault_members", ()).await?;
    for m in members {
        local.execute(
            "INSERT OR REPLACE INTO vault_members (user_uuid, public_key, wrapped_master_dek, role, added_at, inviter_public_key) VALUES (?, ?, ?, ?, ?, ?)",
            m.clone(),
        )
        .await?;
    }
    Ok(())
}

/// Turso in both of its formats.
pub mod turso {
    use super::*;

    /// The tables a database in the record format has.
    pub const RECORD_SCHEMA: &str = r#"
        CREATE TABLE IF NOT EXISTS reach_sync_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS reach_records (
            id TEXT PRIMARY KEY,
            seq INTEGER NOT NULL,
            sealed BLOB NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS reach_records_seq ON reach_records(seq);
    "#;

    /// Make a new database in the record format.
    pub async fn init_records(remote: &Connection) -> Result<(), VaultError> {
        remote.execute_batch(RECORD_SCHEMA).await?;
        remote
            .execute("INSERT OR REPLACE INTO reach_sync_meta (key, value) VALUES ('format', '2')", ())
            .await?;
        Ok(())
    }

    /// Whether the database is in the record format.
    pub async fn is_record_format(remote: &Connection) -> Result<bool, VaultError> {
        let mut rows = remote
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'reach_sync_meta'", ())
            .await?;
        if rows.next().await?.is_none() {
            return Ok(false);
        }
        drop(rows);
        let mut rows = remote.query("SELECT value FROM reach_sync_meta WHERE key = 'format'", ()).await?;
        Ok(matches!(rows.next().await?, Some(row) if row.get::<String>(0)? == "2"))
    }

    /// Whether the database has a vault header at all.
    pub async fn has_header(remote: &Connection) -> bool {
        match remote.query("SELECT 1 FROM vault_header LIMIT 1", ()).await {
            Ok(mut rows) => matches!(rows.next().await, Ok(Some(_))),
            Err(_) => false,
        }
    }

    /// Set up an empty database for the vault in `local`: the tables, the
    /// record format, and the header and members.
    async fn seed(local: &Connection, remote: &Connection) -> Result<(), VaultError> {
        let (header, members) = read_header(local).await?;
        remote.execute_batch(crate::vault::schema::SCHEMA).await?;
        init_records(remote).await?;
        remote
            .execute(
                "INSERT INTO vault_header (id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek) VALUES (?, ?, ?, ?, ?, ?, ?)",
                header,
            )
            .await?;
        for m in members {
            remote
                .execute(
                    "INSERT OR REPLACE INTO vault_members (user_uuid, public_key, wrapped_master_dek, role, added_at, inviter_public_key) VALUES (?, ?, ?, ?, ?, ?)",
                    m,
                )
                .await?;
        }
        Ok(())
    }

    /// The header row and every member row, as stored.
    pub async fn read_header(remote: &Connection) -> Result<(Vec<libsql::Value>, Vec<Vec<libsql::Value>>), VaultError> {
        let mut rows = remote
            .query("SELECT id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek FROM vault_header LIMIT 1", ())
            .await?;
        let row = rows.next().await?.ok_or_else(|| VaultError::NotFound("vault header".into()))?;
        let header = (0..7).map(|i| row.get_value(i)).collect::<Result<Vec<_>, _>>()?;
        drop(rows);
        let mut rows = remote
            .query("SELECT user_uuid, public_key, wrapped_master_dek, role, added_at, inviter_public_key FROM vault_members", ())
            .await?;
        let mut members = Vec::new();
        while let Some(row) = rows.next().await? {
            members.push((0..6).map(|i| row.get_value(i)).collect::<Result<Vec<_>, _>>()?);
        }
        Ok((header, members))
    }

    pub async fn sync(target: &Target, local: &Connection, remote: &Connection) -> Result<Report, VaultError> {
        let (header, members) = match read_header(remote).await {
            Ok(found) => found,
            // An empty database, as when sync moved to a new one: set it up
            // from this file, in the record format, and send it everything.
            Err(VaultError::NotFound(_)) | Err(VaultError::DatabaseError(_)) if !has_header(remote).await => {
                seed(local, remote).await?;
                sync_local::reset(local).await?;
                sync_local::mark_all_dirty(local).await?;
                read_header(remote).await?
            }
            Err(e) => return Err(e),
        };
        mirror_header(local, &header, &members).await?;

        let records = is_record_format(remote).await?;
        let format = if records { "2" } else { "legacy" };
        if sync_local::state(local, STATE_FORMAT).await?.as_deref() != Some(format) {
            // Upgraded elsewhere (or first sync): read everything once.
            sync_local::clear_state(local, STATE_CURSOR).await?;
            sync_local::set_state(local, STATE_FORMAT, format).await?;
        }
        if records {
            sync_records(target, local, remote).await
        } else {
            sync_legacy(target, local, remote).await
        }
    }

    async fn cursor(local: &Connection) -> Result<i64, VaultError> {
        Ok(sync_local::state(local, STATE_CURSOR).await?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    async fn pull(target: &Target, remote: &Connection, since: i64) -> Result<(Vec<Record>, i64), VaultError> {
        let mut rows = remote
            .query("SELECT id, seq, sealed FROM reach_records WHERE seq > ? ORDER BY seq", [since])
            .await?;
        let mut records = Vec::new();
        let mut last = since;
        while let Some(row) = rows.next().await? {
            let remote_id: String = row.get(0)?;
            let seq: i64 = row.get(1)?;
            let sealed: Vec<u8> = row.get(2)?;
            last = last.max(seq);
            match target.key.open(&remote_id, &sealed) {
                Ok(record) => records.push(record),
                // One record that does not open must not stop the rest.
                Err(e) => tracing::error!(vault = %target.vault_id, "Skipping a record: {}", e),
            }
        }
        Ok((records, last))
    }

    async fn sync_records(target: &Target, local: &Connection, remote: &Connection) -> Result<Report, VaultError> {
        let mut report = Report::default();
        for _ in 0..PUSH_ATTEMPTS {
            let since = cursor(local).await?;
            let (records, last) = pull(target, remote, since).await?;
            let merged = merge(target, local, records, since == 0).await?;
            report.pulled += merged.pulled;
            report.conflicts += merged.conflicts;
            sync_local::set_state(local, STATE_CURSOR, &last.to_string()).await?;

            let out = outgoing(target, local).await?;
            if out.is_empty() {
                return Ok(report);
            }
            match push(target, remote, last, &out).await? {
                Some(new_cursor) => {
                    sent(local, &out).await?;
                    sync_local::set_state(local, STATE_CURSOR, &new_cursor.to_string()).await?;
                    report.pushed += out.len();
                    return Ok(report);
                }
                None => continue, // another device pushed first: pull again
            }
        }
        Err(VaultError::SyncError("other devices kept changing the vault; try again".into()))
    }

    /// Send records, provided nobody else has sent any since `base`. Returns
    /// the new cursor, or `None` if someone had.
    async fn push(target: &Target, remote: &Connection, base: i64, out: &[(Dirty, Record)]) -> Result<Option<i64>, VaultError> {
        let tx = remote.transaction_with_behavior(TransactionBehavior::Immediate).await?;
        let mut rows = tx.query("SELECT COALESCE(MAX(seq), 0) FROM reach_records", ()).await?;
        let max: i64 = rows.next().await?.map(|r| r.get(0)).transpose()?.unwrap_or(0);
        drop(rows);
        if max != base {
            tx.rollback().await?;
            return Ok(None);
        }
        let mut seq = base;
        for (_, record) in out {
            seq += 1;
            tx.execute(
                "INSERT INTO reach_records (id, seq, sealed) VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET seq = excluded.seq, sealed = excluded.sealed",
                (target.key.remote_id(&record.id), seq, target.key.seal(record)?),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(Some(seq))
    }

    async fn sync_legacy(target: &Target, local: &Connection, remote: &Connection) -> Result<Report, VaultError> {
        let mut rows = remote
            .query("SELECT id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at FROM secrets", ())
            .await?;
        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            let r = Row {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
                nonce: row.get(3)?,
                ciphertext: row.get(4)?,
                wrapped_dek: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            };
            records.push(Record { id: r.id.clone(), version: r.updated_at * 1000, device: LEGACY_DEVICE.into(), row: Some(r) });
        }
        drop(rows);
        let mut report = merge(target, local, records, true).await?;
        report.legacy = true;

        let out = outgoing(target, local).await?;
        for (_, record) in &out {
            match &record.row {
                Some(r) => {
                    remote
                        .execute(
                            "INSERT OR REPLACE INTO secrets (id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                            (r.id.as_str(), r.name.as_str(), r.category.as_str(), r.nonce.as_slice(), r.ciphertext.as_slice(), r.wrapped_dek.as_str(), r.created_at, r.updated_at),
                        )
                        .await?;
                }
                None => {
                    remote.execute("DELETE FROM secrets WHERE id = ?", [record.id.as_str()]).await?;
                }
            }
        }
        // What the legacy table now has of each, as the next full read will
        // see it.
        let legacy: Vec<(Dirty, Record)> = out
            .into_iter()
            .map(|(change, mut record)| {
                if let Some(r) = &record.row {
                    record.version = r.updated_at * 1000;
                }
                (change, record)
            })
            .collect();
        sent(local, &legacy).await?;
        report.pushed += legacy.len();
        Ok(report)
    }

    pub async fn upgrade(target: &Target, local: &Connection, remote: &Connection) -> Result<(), VaultError> {
        if is_record_format(remote).await? {
            return Ok(());
        }
        let rows = sync_local::all_rows(local).await?;
        let backup = format!("secrets_legacy_{}", now_ms() / 1000);
        let tx = remote.transaction_with_behavior(TransactionBehavior::Immediate).await?;
        tx.execute_batch(RECORD_SCHEMA).await?;
        let mut seq = 0i64;
        let mut records = Vec::new();
        for row in rows {
            let version = sync_local::known(local, &row.id).await?.unwrap_or(row.updated_at * 1000);
            seq += 1;
            let record = Record { id: row.id.clone(), version, device: target.device.clone(), row: Some(row) };
            tx.execute(
                "INSERT OR REPLACE INTO reach_records (id, seq, sealed) VALUES (?, ?, ?)",
                (target.key.remote_id(&record.id), seq, target.key.seal(&record)?),
            )
            .await?;
            records.push(record);
        }
        // Kept, not dropped: the old rows stay recoverable.
        tx.execute(&format!("ALTER TABLE secrets RENAME TO {backup}"), ()).await?;
        tx.execute("INSERT OR REPLACE INTO reach_sync_meta (key, value) VALUES ('format', '2')", ()).await?;
        tx.commit().await?;

        for record in &records {
            sync_local::set_known(local, &record.id, record.version).await?;
        }
        sync_local::set_state(local, STATE_FORMAT, "2").await?;
        sync_local::set_state(local, STATE_CURSOR, &seq.to_string()).await?;
        tracing::info!(vault = %target.vault_id, records = seq, "Vault moved to the sealed sync format");
        Ok(())
    }
}

#[cfg(test)]
#[path = "sync_engine_tests.rs"]
mod tests;
