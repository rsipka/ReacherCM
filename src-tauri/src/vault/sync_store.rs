//! Places that hold a vault as one file: a folder (on this disk, a mounted
//! share or a UNC path), an SMB share reached directly, an SFTP server, a
//! Git repository, or an S3 bucket (see `sync_s3`).
//!
//! Each store gives the file with a revision, and takes a new one only if the
//! revision is still the one read: two devices writing at once cannot both
//! win, the second reads again and merges. A folder and an SFTP server do
//! that with a lock file beside the vault file and a write that replaces it
//! whole; a Git repository with a push that names the commit it replaces.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::vault::error::VaultError;

/// The vault file's name in a folder, on a server or in a repository.
pub const FILE_NAME: &str = "reach-vault.reach";

/// A lock older than this was left by a device that stopped mid-write.
const STALE_LOCK_MS: i64 = 120_000;

/// How long one network step may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(45);

fn sync_error(msg: impl Into<String>) -> VaultError {
    VaultError::SyncError(msg.into())
}

fn revision_of(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ms() -> i64 {
    crate::vault::sync_engine::now_ms()
}

async fn bounded<T>(what: &str, f: impl std::future::Future<Output = Result<T, VaultError>>) -> Result<T, VaultError> {
    match tokio::time::timeout(STEP_TIMEOUT, f).await {
        Ok(r) => r,
        Err(_) => Err(sync_error(format!("{what} did not finish within {} seconds", STEP_TIMEOUT.as_secs()))),
    }
}

/// How to reach an SFTP server.
#[derive(Clone)]
pub struct SftpTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The folder on the server.
    pub path: String,
    /// The server's host key, as `SHA256:…`. Without it nothing is sent.
    pub host_key: Option<String>,
    pub password: Option<String>,
    /// A private key, as the text of its file.
    pub key: Option<String>,
    pub passphrase: Option<String>,
}

/// How to reach an SMB share, without mounting it.
#[derive(Clone)]
pub struct SmbTarget {
    pub server: String,
    pub port: u16,
    pub share: String,
    /// The folder in the share, `\`-separated, empty for its root.
    pub path: String,
    /// `user`, `DOMAIN\user` or `user@domain`.
    pub user: String,
    pub password: String,
}

/// How to reach a Git repository.
#[derive(Clone)]
pub struct GitTarget {
    pub remote: String,
    pub branch: String,
    /// A token for an https remote; otherwise Git's own credentials are used.
    pub token: Option<String>,
    /// Where this device keeps its copy of the repository.
    pub work_dir: PathBuf,
}

/// An open store.
pub enum Store {
    Files(Files),
    Git(GitStore),
    S3(crate::vault::sync_s3::S3Store),
    #[cfg(desktop)]
    Account(crate::vault::sync_account_store::AccountTarget),
}

impl Store {
    pub async fn folder(dir: &Path) -> Result<Store, VaultError> {
        Ok(Store::Files(Files::Local(dir.to_path_buf())))
    }

    pub async fn sftp(target: &SftpTarget) -> Result<Store, VaultError> {
        Ok(Store::Files(Files::Sftp(Box::new(bounded("The SFTP connection", SftpFiles::connect(target)).await?))))
    }

    pub async fn smb(target: &SmbTarget) -> Result<Store, VaultError> {
        Ok(Store::Files(Files::Smb(Box::new(bounded("The SMB connection", SmbFiles::connect(target)).await?))))
    }

    pub async fn git(target: &GitTarget) -> Result<Store, VaultError> {
        Ok(Store::Git(GitStore::open(target).await?))
    }

    /// The vault file and its revision, or `None` if there is none yet.
    pub async fn get(&mut self) -> Result<Option<(Vec<u8>, String)>, VaultError> {
        match self {
            Store::Files(f) => f.get().await,
            Store::Git(g) => g.get().await,
            Store::S3(s) => s.get().await,
            #[cfg(desktop)]
            Store::Account(s) => s.get().await,
        }
    }

    /// Write the vault file, provided it is still at `expected` (`None`:
    /// still absent). Returns false if another device wrote first.
    pub async fn put(&mut self, data: &[u8], expected: Option<&str>) -> Result<bool, VaultError> {
        match self {
            Store::Files(f) => f.put(data, expected).await,
            Store::Git(g) => g.put(data, expected).await,
            Store::S3(s) => s.put(data, expected).await,
            #[cfg(desktop)]
            Store::Account(s) => s.put(data, expected).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Folders and SFTP: one file, a lock beside it.

/// A folder that holds the vault file, here or on an SFTP server.
pub enum Files {
    Local(PathBuf),
    Sftp(Box<SftpFiles>),
    Smb(Box<SmbFiles>),
}

impl Files {
    fn lock_name() -> String {
        format!("{FILE_NAME}.lock")
    }

    fn new_name() -> String {
        format!("{FILE_NAME}.new")
    }

    fn old_name() -> String {
        format!("{FILE_NAME}.old")
    }

    async fn get(&mut self) -> Result<Option<(Vec<u8>, String)>, VaultError> {
        self.check_dir().await?;
        // A write that stopped between moving the old file aside and putting
        // the new one in its place leaves only the old one.
        let data = match self.read(FILE_NAME).await? {
            Some(d) => Some(d),
            None => self.read(&Self::old_name()).await?,
        };
        Ok(data.map(|d| {
            let rev = revision_of(&d);
            (d, rev)
        }))
    }

    async fn put(&mut self, data: &[u8], expected: Option<&str>) -> Result<bool, VaultError> {
        self.check_dir().await?;
        self.lock().await?;
        let result = self.put_locked(data, expected).await;
        if let Err(e) = self.remove(&Self::lock_name()).await {
            tracing::warn!("Could not remove the sync lock file: {}", e);
        }
        result
    }

    async fn put_locked(&mut self, data: &[u8], expected: Option<&str>) -> Result<bool, VaultError> {
        let current = match self.read(FILE_NAME).await? {
            Some(d) => Some(d),
            None => self.read(&Self::old_name()).await?,
        };
        if current.as_deref().map(revision_of).as_deref() != expected {
            return Ok(false);
        }
        self.write(&Self::new_name(), data).await?;
        self.replace(&Self::new_name(), FILE_NAME).await?;
        Ok(true)
    }

    /// Take the lock: a file only one device can create. One left by a
    /// device that stopped mid-write is taken over once it is old.
    async fn lock(&mut self) -> Result<(), VaultError> {
        let stamp = now_ms().to_string();
        for _ in 0..2 {
            if self.create_new(&Self::lock_name(), stamp.as_bytes()).await? {
                return Ok(());
            }
            let held_since = self
                .read(&Self::lock_name())
                .await?
                .and_then(|d| String::from_utf8(d).ok())
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(0);
            if now_ms() - held_since < STALE_LOCK_MS {
                return Err(sync_error("Another device is writing to the sync location; trying again shortly"));
            }
            tracing::warn!("Taking over a sync lock left {} s ago", (now_ms() - held_since) / 1000);
            self.remove(&Self::lock_name()).await?;
        }
        Err(sync_error("Could not take the sync lock"))
    }

    async fn check_dir(&mut self) -> Result<(), VaultError> {
        match self {
            // Never created here: a share that is not connected must not be
            // replaced by an empty folder on this disk.
            Files::Local(dir) => match tokio::fs::metadata(&dir).await {
                Ok(m) if m.is_dir() => Ok(()),
                _ => Err(sync_error(format!(
                    "The sync folder {} is not there. Is the drive or share connected?",
                    dir.display()
                ))),
            },
            Files::Sftp(s) => s.check_dir().await,
            Files::Smb(s) => s.check_dir().await,
        }
    }

    async fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, VaultError> {
        match self {
            Files::Local(dir) => match tokio::fs::read(dir.join(name)).await {
                Ok(d) => Ok(Some(d)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(sync_error(format!("Could not read {}: {e}", dir.join(name).display()))),
            },
            Files::Sftp(s) => s.read(name).await,
            Files::Smb(s) => s.read(name).await,
        }
    }

    async fn write(&mut self, name: &str, data: &[u8]) -> Result<(), VaultError> {
        match self {
            Files::Local(dir) => {
                let path = dir.join(name);
                let data = data.to_vec();
                tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                    use std::io::Write;
                    let mut f = private_options().create(true).truncate(true).open(&path)?;
                    f.write_all(&data)?;
                    f.sync_all()
                })
                .await
                .map_err(|e| sync_error(e.to_string()))?
                .map_err(|e| sync_error(format!("Could not write to the sync folder: {e}")))
            }
            Files::Sftp(s) => s.write(name, data).await,
            Files::Smb(s) => s.write(name, data).await,
        }
    }

    async fn create_new(&mut self, name: &str, data: &[u8]) -> Result<bool, VaultError> {
        match self {
            Files::Local(dir) => {
                let path = dir.join(name);
                let data = data.to_vec();
                tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
                    use std::io::Write;
                    match private_options().create_new(true).open(&path) {
                        Ok(mut f) => {
                            f.write_all(&data)?;
                            Ok(true)
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                        Err(e) => Err(e),
                    }
                })
                .await
                .map_err(|e| sync_error(e.to_string()))?
                .map_err(|e| sync_error(format!("Could not write to the sync folder: {e}")))
            }
            Files::Sftp(s) => s.create_new(name, data).await,
            Files::Smb(s) => s.create_new(name, data).await,
        }
    }

    async fn remove(&mut self, name: &str) -> Result<(), VaultError> {
        match self {
            Files::Local(dir) => match tokio::fs::remove_file(dir.join(name)).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(sync_error(format!("Could not remove {name}: {e}"))),
            },
            Files::Sftp(s) => s.remove(name).await,
            Files::Smb(s) => s.remove(name).await,
        }
    }

    /// Put `from` in the place of `to`, whole.
    async fn replace(&mut self, from: &str, to: &str) -> Result<(), VaultError> {
        match self {
            // Replaces the target in one step, on Windows shares too.
            Files::Local(dir) => tokio::fs::rename(dir.join(from), dir.join(to))
                .await
                .map_err(|e| sync_error(format!("Could not replace the vault file: {e}"))),
            Files::Sftp(s) => s.replace(from, to).await,
            Files::Smb(s) => s.replace(from, to).await,
        }
    }
}

/// Opens a file for writing, readable by its owner only where the system
/// has such modes.
fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o
}

/// A folder on an SFTP server.
pub struct SftpFiles {
    sftp: russh_sftp::client::SftpSession,
    dir: String,
    // Holds the connection open for as long as the session is used.
    _ssh: russh::client::Handle<PinnedHost>,
}

/// Accepts only the host key the user confirmed, and notes the one offered.
pub struct PinnedHost {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl russh::client::Handler for PinnedHost {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fingerprint = key.public_key().fingerprint(russh::keys::HashAlg::Sha256).to_string();
        let ok = self.expected.as_deref().is_some_and(|e| e.trim() == fingerprint);
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint);
        Ok(ok)
    }
}

/// The error for a host key that is not the one confirmed; carries the one
/// the server offered, for the user to check.
pub fn unconfirmed_host_key(fingerprint: &str) -> VaultError {
    sync_error(format!("HOSTKEY:{fingerprint}"))
}

/// The host key in an error from [`unconfirmed_host_key`].
pub fn host_key_in(e: &VaultError) -> Option<String> {
    match e {
        VaultError::SyncError(m) => m.strip_prefix("HOSTKEY:").map(str::to_string),
        _ => None,
    }
}

/// Readable by the owner only, as the vault files on this device are.
fn private() -> russh_sftp::protocol::FileAttributes {
    russh_sftp::protocol::FileAttributes { permissions: Some(0o600), ..russh_sftp::protocol::FileAttributes::empty() }
}

fn sftp_error(what: &str, e: impl std::fmt::Display) -> VaultError {
    sync_error(format!("SFTP: {what}: {e}"))
}

fn is_no_such_file(e: &russh_sftp::client::error::Error) -> bool {
    matches!(e, russh_sftp::client::error::Error::Status(s) if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile)
}

impl SftpFiles {
    async fn connect(t: &SftpTarget) -> Result<SftpFiles, VaultError> {
        use crate::ssh::client::{cascade_authenticate, AuthParams, KeyAuth, KeySource};

        let seen = Arc::new(Mutex::new(None));
        let handler = PinnedHost { expected: t.host_key.clone(), seen: seen.clone() };
        let config = Arc::new(russh::client::Config::default());
        let mut ssh = match russh::client::connect(config, (t.host.as_str(), t.port), handler).await {
            Ok(h) => h,
            Err(e) => {
                let offered = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
                return Err(match offered {
                    Some(fp) if t.host_key.as_deref().map(str::trim) != Some(fp.as_str()) => {
                        if t.host_key.is_some() {
                            sync_error(format!(
                                "The SFTP server's host key changed (now {fp}). If you expected that, confirm the new key in Settings > Sync."
                            ))
                        } else {
                            unconfirmed_host_key(&fp)
                        }
                    }
                    _ => sftp_error(&format!("could not connect to {}:{}", t.host, t.port), e),
                });
            }
        };
        let auth = AuthParams {
            key: t.key.as_ref().filter(|k| !k.trim().is_empty()).map(|k| KeyAuth {
                source: KeySource::Material(k.clone()),
                passphrase: t.passphrase.clone().filter(|p| !p.is_empty()),
            }),
            password: t.password.clone().filter(|p| !p.is_empty()),
            // A background sync must never wait on an agent's confirmation.
            allow_agent: false,
        };
        cascade_authenticate(&mut ssh, &t.user, &auth)
            .await
            .map_err(|e| sftp_error("login failed", e))?
            .into_result()
            .map_err(|e| sftp_error("login failed", e))?;
        let channel = ssh.channel_open_session().await.map_err(|e| sftp_error("no session", e))?;
        channel.request_subsystem(true, "sftp").await.map_err(|e| sftp_error("the server offers no SFTP", e))?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| sftp_error("the server offers no SFTP", e))?;
        sftp.set_timeout(STEP_TIMEOUT.as_secs());
        let dir = if t.path.is_empty() { ".".to_string() } else { t.path.clone() };
        Ok(SftpFiles { sftp, dir, _ssh: ssh })
    }

    fn path(&self, name: &str) -> String {
        format!("{}/{}", self.dir.trim_end_matches('/'), name)
    }

    async fn check_dir(&self) -> Result<(), VaultError> {
        match self.sftp.metadata(self.dir.clone()).await {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(sync_error(format!("SFTP: {} is not a folder", self.dir))),
            Err(e) if is_no_such_file(&e) => Err(sync_error(format!("SFTP: the folder {} is not there", self.dir))),
            Err(e) => Err(sftp_error(&self.dir, e)),
        }
    }

    async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, VaultError> {
        match self.sftp.read(self.path(name)).await {
            Ok(d) => Ok(Some(d)),
            Err(e) if is_no_such_file(&e) => Ok(None),
            Err(e) => Err(sftp_error(&format!("reading {name}"), e)),
        }
    }

    async fn write(&self, name: &str, data: &[u8]) -> Result<(), VaultError> {
        use russh_sftp::protocol::OpenFlags;
        use tokio::io::AsyncWriteExt;
        let mut f = self
            .sftp
            .open_with_flags_and_attributes(self.path(name), OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE, private())
            .await
            .map_err(|e| sftp_error(&format!("writing {name}"), e))?;
        f.write_all(data).await.map_err(|e| sftp_error(&format!("writing {name}"), e))?;
        f.shutdown().await.map_err(|e| sftp_error(&format!("writing {name}"), e))?;
        Ok(())
    }

    async fn create_new(&self, name: &str, data: &[u8]) -> Result<bool, VaultError> {
        use russh_sftp::protocol::{OpenFlags, StatusCode};
        use tokio::io::AsyncWriteExt;
        let opened = self
            .sftp
            .open_with_flags_and_attributes(self.path(name), OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE, private())
            .await;
        match opened {
            Ok(mut f) => {
                f.write_all(data).await.map_err(|e| sftp_error(&format!("writing {name}"), e))?;
                f.shutdown().await.map_err(|e| sftp_error(&format!("writing {name}"), e))?;
                Ok(true)
            }
            // SFTP v3 reports an existing file only as a failure.
            Err(russh_sftp::client::error::Error::Status(s)) if s.status_code == StatusCode::Failure => Ok(false),
            Err(e) => Err(sftp_error(&format!("creating {name}"), e)),
        }
    }

    async fn remove(&self, name: &str) -> Result<(), VaultError> {
        match self.sftp.remove_file(self.path(name)).await {
            Ok(()) => Ok(()),
            Err(e) if is_no_such_file(&e) => Ok(()),
            Err(e) => Err(sftp_error(&format!("removing {name}"), e)),
        }
    }

    async fn replace(&self, from: &str, to: &str) -> Result<(), VaultError> {
        if self.sftp.rename(self.path(from), self.path(to)).await.is_ok() {
            return Ok(());
        }
        // SFTP v3 does not rename over a file: move it aside first. A read
        // between the two steps finds the old one under its `.old` name.
        let old = Files::old_name();
        self.remove(&old).await?;
        if self.read(to).await?.is_some() {
            self.sftp.rename(self.path(to), self.path(&old)).await.map_err(|e| sftp_error("replacing the vault file", e))?;
        }
        self.sftp.rename(self.path(from), self.path(to)).await.map_err(|e| sftp_error("replacing the vault file", e))?;
        self.remove(&old).await
    }
}

/// A folder on an SMB share, through Reach's own SMB client: no mount, and
/// the same on every system.
pub struct SmbFiles {
    client: smb::Client,
    root: smb::UncPath,
    dir: String,
}

fn smb_error(what: &str, e: impl std::fmt::Display) -> VaultError {
    sync_error(format!("SMB: {what}: {e}"))
}

/// The NT status an SMB error carries, if any.
fn smb_status(e: &smb::Error) -> Option<u32> {
    match e {
        smb::Error::UnexpectedMessageStatus(s) | smb::Error::ReceivedErrorMessage(s, _) => Some(*s),
        _ => None,
    }
}

const STATUS_OBJECT_NAME_NOT_FOUND: u32 = 0xC000_0034;
const STATUS_OBJECT_NAME_COLLISION: u32 = 0xC000_0035;
const STATUS_OBJECT_PATH_NOT_FOUND: u32 = 0xC000_003A;

fn smb_not_found(e: &smb::Error) -> bool {
    matches!(smb_status(e), Some(STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND))
}

impl SmbFiles {
    async fn connect(t: &SmbTarget) -> Result<SmbFiles, VaultError> {
        use std::str::FromStr;
        let config = smb::ClientConfig {
            connection: smb::ConnectionConfig {
                port: Some(t.port),
                timeout: Some(STEP_TIMEOUT),
                ..Default::default()
            },
            ..Default::default()
        };
        let client = smb::Client::new(config);
        let root = smb::UncPath::from_str(&format!(r"\\{}\{}", t.server, t.share))
            .map_err(|e| smb_error("the share's address", e))?;
        client
            .share_connect(&root, &t.user, t.password.clone())
            .await
            .map_err(|e| smb_error(&format!("could not connect to \\\\{}\\{}", t.server, t.share), e))?;
        Ok(SmbFiles { client, root, dir: t.path.trim_matches('\\').to_string() })
    }

    fn path(&self, name: &str) -> smb::UncPath {
        if self.dir.is_empty() {
            self.root.clone().with_path(name)
        } else {
            self.root.clone().with_path(&format!("{}\\{}", self.dir, name))
        }
    }

    async fn open(&self, path: &smb::UncPath, args: &smb::FileCreateArgs) -> Result<smb::Resource, smb::Error> {
        self.client.create_file(path, args).await
    }

    async fn check_dir(&self) -> Result<(), VaultError> {
        if self.dir.is_empty() {
            return Ok(());
        }
        let path = self.root.clone().with_path(&self.dir);
        let args = smb::FileCreateArgs::make_open_existing(smb::FileAccessMask::new().with_generic_read(true));
        match self.open(&path, &args).await {
            Ok(r) => {
                let is_dir = r.is_dir();
                if let Some(d) = r.as_dir() {
                    let _ = d.close().await;
                } else if let Some(f) = r.as_file() {
                    let _ = f.close().await;
                }
                if is_dir {
                    Ok(())
                } else {
                    Err(sync_error(format!("SMB: {} is not a folder", self.dir)))
                }
            }
            Err(e) if smb_not_found(&e) => Err(sync_error(format!("SMB: the folder {} is not on the share", self.dir))),
            Err(e) => Err(smb_error(&self.dir, e)),
        }
    }

    async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, VaultError> {
        use smb::resource::{GetLen, ReadAt};
        let args = smb::FileCreateArgs::make_open_existing(smb::FileAccessMask::new().with_generic_read(true));
        let file = match self.open(&self.path(name), &args).await {
            Ok(r) if r.is_file() => r.unwrap_file(),
            Ok(_) => return Err(sync_error(format!("SMB: {name} is not a file"))),
            Err(e) if smb_not_found(&e) => return Ok(None),
            Err(e) => return Err(smb_error(&format!("reading {name}"), e)),
        };
        let result = async {
            let len = file.get_len().await? as usize;
            let mut data = vec![0u8; len];
            let mut at = 0;
            while at < len {
                let n = file.read_at(&mut data[at..], at as u64).await?;
                if n == 0 {
                    break;
                }
                at += n;
            }
            data.truncate(at);
            Ok::<_, smb::Error>(data)
        }
        .await;
        let _ = file.close().await;
        result.map(Some).map_err(|e| smb_error(&format!("reading {name}"), e))
    }

    async fn write_all(file: &smb::File, data: &[u8]) -> Result<(), smb::Error> {
        use smb::resource::WriteAt;
        let mut at = 0;
        while at < data.len() {
            let n = file.write_at(&data[at..], at as u64).await?;
            if n == 0 {
                return Err(smb::Error::InvalidMessage("the server took no data".into()));
            }
            at += n;
        }
        Ok(())
    }

    async fn write(&self, name: &str, data: &[u8]) -> Result<(), VaultError> {
        let args = smb::FileCreateArgs::make_overwrite(Default::default(), Default::default());
        let file = self
            .open(&self.path(name), &args)
            .await
            .map_err(|e| smb_error(&format!("writing {name}"), e))?
            .unwrap_file();
        let result = Self::write_all(&file, data).await;
        let _ = file.flush().await;
        let closed = file.close().await;
        result.and(closed).map_err(|e| smb_error(&format!("writing {name}"), e))
    }

    async fn create_new(&self, name: &str, data: &[u8]) -> Result<bool, VaultError> {
        let args = smb::FileCreateArgs::make_create_new(Default::default(), Default::default());
        let file = match self.open(&self.path(name), &args).await {
            Ok(r) => r.unwrap_file(),
            Err(e) if smb_status(&e) == Some(STATUS_OBJECT_NAME_COLLISION) => return Ok(false),
            Err(e) => return Err(smb_error(&format!("creating {name}"), e)),
        };
        let result = Self::write_all(&file, data).await;
        let closed = file.close().await;
        result.and(closed).map_err(|e| smb_error(&format!("writing {name}"), e))?;
        Ok(true)
    }

    async fn remove(&self, name: &str) -> Result<(), VaultError> {
        let args = smb::FileCreateArgs::make_open_existing(smb::FileAccessMask::new().with_generic_all(true));
        let file = match self.open(&self.path(name), &args).await {
            Ok(r) => r.unwrap_file(),
            Err(e) if smb_not_found(&e) => return Ok(()),
            Err(e) => return Err(smb_error(&format!("removing {name}"), e)),
        };
        let result = file.set_info(smb::FileDispositionInformation::default()).await;
        let closed = file.close().await;
        result.and(closed).map_err(|e| smb_error(&format!("removing {name}"), e))
    }

    async fn replace(&self, from: &str, to: &str) -> Result<(), VaultError> {
        let args = smb::FileCreateArgs::make_open_existing(smb::FileAccessMask::new().with_generic_all(true));
        let file = self
            .open(&self.path(from), &args)
            .await
            .map_err(|e| smb_error("replacing the vault file", e))?
            .unwrap_file();
        let target = if self.dir.is_empty() { to.to_string() } else { format!("{}\\{}", self.dir, to) };
        let result = file
            .set_info(smb::FileRenameInformation {
                replace_if_exists: true.into(),
                root_directory: 0,
                file_name: target.as_str().into(),
            })
            .await;
        let closed = file.close().await;
        result.and(closed).map_err(|e| smb_error("replacing the vault file", e))
    }
}

// ---------------------------------------------------------------------------
// Git: the vault file is the only file of a branch that holds one commit.

/// A Git repository, through the `git` command, so the user's own SSH keys
/// and credential helpers work as they do in a terminal.
///
/// Each write is a new commit with no parent, pushed only if the branch still
/// points at the commit that was read (`--force-with-lease`). The branch never
/// grows a history of old vault files: the repository stays the size of one.
pub struct GitStore {
    target: GitTarget,
}

struct GitOutput {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

impl GitStore {
    async fn open(target: &GitTarget) -> Result<GitStore, VaultError> {
        let store = GitStore { target: target.clone() };
        if !target.work_dir.join("HEAD").exists() {
            crate::vault::private_fs::private_dir(&target.work_dir)?;
            store.must(&["init", "--bare", "--quiet"], None).await?;
        }
        Ok(store)
    }

    fn remote_ref(&self) -> String {
        format!("refs/reach/{}", self.target.branch)
    }

    async fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<GitOutput, VaultError> {
        use tokio::io::AsyncWriteExt;
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("--git-dir").arg(&self.target.work_dir).args(args);
        // Only transports that move data: never `ext::`, which runs a command.
        cmd.env("GIT_ALLOW_PROTOCOL", "https:http:ssh:git:file")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_AUTHOR_NAME", "Reach")
            .env("GIT_AUTHOR_EMAIL", "sync@reach.invalid")
            .env("GIT_COMMITTER_NAME", "Reach")
            .env("GIT_COMMITTER_EMAIL", "sync@reach.invalid");
        if std::env::var_os("GIT_SSH_COMMAND").is_none() {
            // Never stop to ask: a host or key ssh would ask about fails instead.
            cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        if let Some(token) = self.target.token.as_deref().filter(|t| !t.is_empty()) {
            // Through the environment, not the command line, where other
            // users of the machine could read it.
            let basic = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("x-access-token:{token}"));
            cmd.env("GIT_CONFIG_COUNT", "2")
                .env("GIT_CONFIG_KEY_0", "http.extraHeader")
                .env("GIT_CONFIG_VALUE_0", format!("Authorization: Basic {basic}"))
                .env("GIT_CONFIG_KEY_1", "credential.helper")
                .env("GIT_CONFIG_VALUE_1", "");
        }
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd.stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                sync_error("Git is not installed, or not on the PATH")
            } else {
                sync_error(format!("Could not run git: {e}"))
            }
        })?;
        if let Some(data) = stdin {
            let mut pipe = child.stdin.take().expect("piped");
            pipe.write_all(data).await.map_err(|e| sync_error(format!("git: {e}")))?;
            drop(pipe);
        }
        let out = match tokio::time::timeout(STEP_TIMEOUT, child.wait_with_output()).await {
            Ok(r) => r.map_err(|e| sync_error(format!("git: {e}")))?,
            Err(_) => return Err(sync_error(format!("git {} did not finish within {} seconds", args[0], STEP_TIMEOUT.as_secs()))),
        };
        Ok(GitOutput {
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }

    async fn must(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>, VaultError> {
        let out = self.run(args, stdin).await?;
        if !out.ok {
            return Err(sync_error(format!("git {}: {}", args[0], redact(&out.stderr))));
        }
        Ok(out.stdout)
    }

    async fn get(&mut self) -> Result<Option<(Vec<u8>, String)>, VaultError> {
        let refspec = format!("+refs/heads/{}:{}", self.target.branch, self.remote_ref());
        let out = self
            .run(&["fetch", "--quiet", "--no-tags", "--depth", "1", self.target.remote.as_str(), refspec.as_str()], None)
            .await?;
        if !out.ok {
            let err = out.stderr.to_ascii_lowercase();
            if err.contains("couldn't find remote ref") || err.contains("could not find remote ref") {
                // A new repository, or a branch not made yet.
                return Ok(None);
            }
            return Err(sync_error(format!("git fetch: {}", redact(&out.stderr))));
        }
        let commit = String::from_utf8_lossy(&self.must(&["rev-parse", &self.remote_ref()], None).await?).trim().to_string();
        let spec = format!("{commit}:{FILE_NAME}");
        let out = self.run(&["cat-file", "blob", &spec], None).await?;
        if !out.ok {
            return Err(sync_error(format!(
                "The branch {} of this repository has no {FILE_NAME}. Use an empty repository or another branch.",
                self.target.branch
            )));
        }
        Ok(Some((out.stdout, commit)))
    }

    async fn put(&mut self, data: &[u8], expected: Option<&str>) -> Result<bool, VaultError> {
        let blob = text(self.must(&["hash-object", "-w", "--stdin"], Some(data)).await?);
        let tree_line = format!("100644 blob {blob}\t{FILE_NAME}\n");
        let tree = text(self.must(&["mktree"], Some(tree_line.as_bytes())).await?);
        let commit = text(self.must(&["commit-tree", &tree, "-m", "Reach vault"], None).await?);
        let lease = format!("--force-with-lease=refs/heads/{}:{}", self.target.branch, expected.unwrap_or(""));
        let dest = format!("{commit}:refs/heads/{}", self.target.branch);
        let out = self.run(&["push", "--quiet", "--no-verify", &lease, self.target.remote.as_str(), &dest], None).await?;
        if out.ok {
            return Ok(true);
        }
        let err = out.stderr.to_ascii_lowercase();
        if err.contains("stale info") || err.contains("rejected") || err.contains("fetch first") {
            return Ok(false);
        }
        Err(sync_error(format!("git push: {}", redact(&out.stderr))))
    }
}

fn text(out: Vec<u8>) -> String {
    String::from_utf8_lossy(&out).trim().to_string()
}

/// Git's message with any `user:password@` in a URL taken out.
fn redact(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| match (w.find("://"), w.find('@')) {
            (Some(s), Some(at)) if at > s => format!("{}***{}", &w[..s + 3], &w[at..]),
            _ => w.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reach-store-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn compare_and_swap(store: &mut Store) {
        assert!(store.get().await.unwrap().is_none());
        assert!(store.put(b"one", None).await.unwrap());
        let (data, rev) = store.get().await.unwrap().unwrap();
        assert_eq!(data, b"one");
        assert!(!store.put(b"late", None).await.unwrap(), "a write that thinks it is first loses");
        assert!(store.put(b"two", Some(&rev)).await.unwrap());
        assert!(!store.put(b"stale", Some(&rev)).await.unwrap(), "a write from an old revision loses");
        assert_eq!(store.get().await.unwrap().unwrap().0, b"two");
    }

    #[tokio::test]
    async fn a_folder_takes_a_write_only_from_the_current_revision() {
        let dir = temp_dir("folder");
        let mut store = Store::folder(&dir).await.unwrap();
        compare_and_swap(&mut store).await;
        assert!(!dir.join(Files::lock_name()).exists(), "the lock is released");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_folder_that_is_not_there_is_not_made() {
        let dir = temp_dir("gone").join("share-not-mounted");
        let mut store = Store::folder(&dir).await.unwrap();
        let err = store.get().await.unwrap_err().to_string();
        assert!(err.contains("not there"), "{err}");
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn a_lock_held_is_respected_and_a_stale_one_taken_over() {
        let dir = temp_dir("lock");
        let mut store = Store::folder(&dir).await.unwrap();
        std::fs::write(dir.join(Files::lock_name()), now_ms().to_string()).unwrap();
        assert!(store.put(b"x", None).await.is_err());
        std::fs::write(dir.join(Files::lock_name()), (now_ms() - STALE_LOCK_MS - 1).to_string()).unwrap();
        assert!(store.put(b"x", None).await.unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_write_cut_short_leaves_the_old_file_readable() {
        let dir = temp_dir("old");
        std::fs::write(dir.join(Files::old_name()), b"before").unwrap();
        let mut store = Store::folder(&dir).await.unwrap();
        let (data, rev) = store.get().await.unwrap().unwrap();
        assert_eq!(data, b"before");
        assert!(store.put(b"after", Some(&rev)).await.unwrap());
        assert_eq!(store.get().await.unwrap().unwrap().0, b"after");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn git_available() -> bool {
        std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[tokio::test]
    async fn a_git_branch_takes_a_write_only_from_the_current_commit() {
        if !git_available() {
            eprintln!("git not installed; skipped");
            return;
        }
        let dir = temp_dir("git");
        let remote = dir.join("remote.git");
        assert!(std::process::Command::new("git").args(["init", "--bare", "--quiet"]).arg(&remote).status().unwrap().success());
        let target = |device: &str| GitTarget {
            remote: remote.to_string_lossy().into_owned(),
            branch: "reach".into(),
            token: None,
            work_dir: dir.join(device),
        };
        let mut a = Store::git(&target("a")).await.unwrap();
        compare_and_swap(&mut a).await;

        // Another device sees it, and its write after a's wins only once it
        // has read a's.
        let mut b = Store::git(&target("b")).await.unwrap();
        let (data, rev) = b.get().await.unwrap().unwrap();
        assert_eq!(data, b"two");
        let (_, a_rev) = a.get().await.unwrap().unwrap();
        assert!(a.put(b"three", Some(&a_rev)).await.unwrap());
        assert!(!b.put(b"from b", Some(&rev)).await.unwrap());
        let (data, rev) = b.get().await.unwrap().unwrap();
        assert_eq!(data, b"three");
        assert!(b.put(b"from b", Some(&rev)).await.unwrap());
        assert_eq!(a.get().await.unwrap().unwrap().0, b"from b");

        // One commit, however many writes.
        let out = std::process::Command::new("git").arg("--git-dir").arg(&remote).args(["rev-list", "--count", "reach"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Against a real SFTP server: `REACH_SFTP_TEST=sftp://user@host:port/~/dir`
    /// (an empty folder), `REACH_SFTP_PASSWORD` and `REACH_SFTP_KEY` (a key
    /// file the server accepts), then
    /// `cargo test --lib sync_store::tests::live -- --ignored`.
    #[tokio::test]
    #[ignore = "needs an SFTP server"]
    async fn live_sftp() {
        let url = std::env::var("REACH_SFTP_TEST").expect("REACH_SFTP_TEST");
        let parse = |url: &str, token: &str| match crate::vault::sync_engine::Backend::parse(url, token, Path::new("/unused")).unwrap() {
            crate::vault::sync_engine::Backend::Sftp(t) => t,
            _ => panic!("not sftp"),
        };
        let password = std::env::var("REACH_SFTP_PASSWORD").unwrap();
        // No host key confirmed: nothing is sent, and the key is named.
        let err = match Store::sftp(&parse(&url, &password)).await {
            Err(e) => e,
            Ok(_) => panic!("connected without a confirmed host key"),
        };
        let fingerprint = host_key_in(&err).expect("the host key is named");
        assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
        let pinned = format!("{url}?hostkey={}", fingerprint.replace('+', "%2B").replace('/', "%2F"));
        let wrong = format!("{url}?hostkey=SHA256:AAAA");
        assert!(host_key_in(&Store::sftp(&parse(&wrong, &password)).await.err().unwrap()).is_none(), "a changed key is not offered for confirming");

        let mut store = Store::sftp(&parse(&pinned, &password)).await.unwrap();
        compare_and_swap(&mut store).await;
        assert!(store.get().await.unwrap().is_some());

        let key = std::fs::read_to_string(std::env::var("REACH_SFTP_KEY").unwrap()).unwrap();
        let token = serde_json::json!({ "key": key }).to_string();
        let mut by_key = Store::sftp(&parse(&pinned, &token)).await.unwrap();
        let (data, rev) = by_key.get().await.unwrap().unwrap();
        assert_eq!(data, b"two");
        assert!(by_key.put(b"three", Some(&rev)).await.unwrap(), "rename over an existing file");
        assert_eq!(store.get().await.unwrap().unwrap().0, b"three");
        let bad = serde_json::json!({ "password": "wrong" }).to_string();
        assert!(Store::sftp(&parse(&pinned, &bad)).await.is_err());
    }

    /// Against a real SMB share: `REACH_SMB_TEST=smb://user@host:port/share`
    /// (a share, or a folder in one, that is empty) and
    /// `REACH_SMB_PASSWORD`, then
    /// `cargo test --lib sync_store::tests::live -- --ignored`.
    #[tokio::test]
    #[ignore = "needs an SMB share"]
    async fn live_smb() {
        let url = std::env::var("REACH_SMB_TEST").expect("REACH_SMB_TEST");
        let password = std::env::var("REACH_SMB_PASSWORD").unwrap();
        let target = |pw: &str| match crate::vault::sync_engine::Backend::parse(&url, pw, Path::new("/unused")).unwrap() {
            crate::vault::sync_engine::Backend::Smb(t) => t,
            _ => panic!("not smb"),
        };
        let mut store = Store::smb(&target(&password)).await.unwrap();
        compare_and_swap(&mut store).await;
        let mut other = Store::smb(&target(&password)).await.unwrap();
        let (_, rev) = other.get().await.unwrap().unwrap();
        assert!(other.put(b"three", Some(&rev)).await.unwrap(), "replace an existing file");
        assert_eq!(store.get().await.unwrap().unwrap().0, b"three");
        assert!(Store::smb(&target("wrong")).await.is_err());
    }

    #[test]
    fn credentials_are_taken_out_of_git_messages() {
        assert_eq!(
            redact("fatal: unable to access 'https://me:tok@example.com/r.git/': denied"),
            "fatal: unable to access 'https://***@example.com/r.git/': denied"
        );
    }
}
