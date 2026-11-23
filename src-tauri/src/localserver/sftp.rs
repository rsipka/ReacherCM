//! SFTP over the app's own SSH library (russh, server side) and russh-sftp.
//! Password logins only, the "sftp" subsystem only: no shell, no commands,
//! no port forwarding. The host key is made once and kept in the app's data
//! folder, so clients that saved it are not warned on every start.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::{HashAlg, PrivateKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::fsroot::{io_reason, Root};
use super::Shared;

fn key_path() -> PathBuf {
    crate::app_data_dir().join("local-servers").join("sftp_host_ed25519")
}

/// The server's host key: the saved one, or a new Ed25519 key saved for next time.
pub fn host_key() -> Result<PrivateKey, String> {
    let path = key_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(key) = PrivateKey::from_openssh(text.trim()) {
            return Ok(key);
        }
    }
    let seed: [u8; 32] = rand::random();
    let pair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let key = PrivateKey::from(pair);
    let text = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .map_err(|e| format!("Could not encode the host key: {e}"))?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    write_private(&path, text.as_bytes());
    Ok(key)
}

/// Written readable by this user only. Failing to save is not fatal: the
/// key then lasts until the server stops.
fn write_private(path: &std::path::Path, bytes: &[u8]) {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut f| f.write_all(bytes));
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::write(path, bytes);
    }
}

pub fn fingerprint(key: &PrivateKey) -> String {
    key.public_key().fingerprint(HashAlg::Sha256).to_string()
}

pub async fn run(listener: TcpListener, key: PrivateKey, shared: Arc<Shared>, cancel: CancellationToken) {
    let mut methods = MethodSet::empty();
    methods.push(MethodKind::Password);
    let config = Arc::new(russh::server::Config {
        server_id: russh::SshId::Standard("SSH-2.0-ReacherCM_SFTP".into()),
        methods,
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::ZERO),
        max_auth_attempts: 3,
        inactivity_timeout: Some(Duration::from_secs(1800)),
        keys: vec![key],
        ..Default::default()
    });
    loop {
        let (stream, peer) = tokio::select! {
            _ = cancel.cancelled() => return,
            r = listener.accept() => match r {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("SFTP accept: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let _ = stream.set_nodelay(true);
        let shared = shared.clone();
        let config = config.clone();
        let cancel = cancel.child_token();
        tokio::spawn(async move {
            shared.log(Some(peer), "Connected");
            let handler = SshHandler { shared: shared.clone(), peer, user: None, channels: HashMap::new() };
            let session = match russh::server::run_stream(config, stream, handler).await {
                Ok(s) => s,
                Err(e) => {
                    shared.log(Some(peer), format!("Disconnected: {e}"));
                    return;
                }
            };
            let handle = session.handle();
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = handle
                        .disconnect(russh::Disconnect::ByApplication, "Server stopped".into(), "en".into())
                        .await;
                }
                r = session => match r {
                    Ok(()) => shared.log(Some(peer), "Disconnected"),
                    Err(e) => shared.log(Some(peer), format!("Disconnected: {e}")),
                },
            }
        });
    }
}

struct SshHandler {
    shared: Arc<Shared>,
    peer: SocketAddr,
    user: Option<String>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for SshHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if self.shared.check_login(user, password) {
            self.shared.log(Some(self.peer), format!("Logged in as {user}"));
            self.user = Some(user.to_string());
            Ok(Auth::Accept)
        } else {
            self.shared.log(Some(self.peer), format!("Login failed for {user}"));
            // Password again, as OpenSSH allows: a plain reject takes the
            // method off the list, so a mistyped password ended the login.
            // max_auth_attempts still caps the tries.
            let mut methods = MethodSet::empty();
            methods.push(MethodKind::Password);
            Ok(Auth::Reject { proceed_with_methods: Some(methods), partial_success: false })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&channel_id)) {
            ("sftp", Some(channel)) => {
                session.channel_success(channel_id)?;
                let sftp = SftpHandler::new(self.shared.clone(), self.peer);
                russh_sftp::server::run(channel.into_stream(), sftp).await;
            }
            _ => session.channel_failure(channel_id)?,
        }
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        self.shared.log(Some(self.peer), "Refused a shell: this server offers SFTP only");
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shared.log(Some(self.peer), "Refused a command: this server offers SFTP only");
        session.channel_failure(channel)?;
        Ok(())
    }
}

enum OpenHandle {
    File { file: fs::File, virt: String, written: u64, read: u64 },
    Dir { entries: Option<Vec<File>> },
}

struct SftpHandler {
    shared: Arc<Shared>,
    peer: SocketAddr,
    handles: HashMap<String, OpenHandle>,
    next: u64,
}

fn status_of(e: &std::io::Error) -> russh_sftp::server::StatusReply {
    let code = match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    };
    code.with_message(io_reason(e))
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "OK".into(), language_tag: "en-US".into() }
}

fn read_only() -> russh_sftp::server::StatusReply {
    StatusCode::PermissionDenied.with_message("This server is read only")
}

impl SftpHandler {
    fn new(shared: Arc<Shared>, peer: SocketAddr) -> Self {
        Self { shared, peer, handles: HashMap::new(), next: 0 }
    }

    fn real(&self, path: &str) -> Result<(String, PathBuf), russh_sftp::server::StatusReply> {
        let virt = Root::normalize("/", path).map_err(|e| status_of(&e))?;
        let real = self.shared.root.real(&virt).map_err(|e| status_of(&e))?;
        Ok((virt, real))
    }

    fn add(&mut self, handle: OpenHandle) -> String {
        self.next += 1;
        let id = self.next.to_string();
        self.handles.insert(id.clone(), handle);
        id
    }
}

impl russh_sftp::server::Handler for SftpHandler {
    type Error = russh_sftp::server::StatusReply;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported.into()
    }

    async fn init(&mut self, _version: u32, _ext: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, _attrs: FileAttributes) -> Result<Handle, Self::Error> {
        let (virt, real) = self.real(&filename)?;
        let write = pflags.intersects(OpenFlags::WRITE | OpenFlags::APPEND | OpenFlags::CREATE | OpenFlags::TRUNCATE);
        if write && !self.shared.allow_write {
            return Err(read_only());
        }
        let mut options = fs::OpenOptions::new();
        options
            .read(pflags.contains(OpenFlags::READ) || !write)
            .write(write)
            .append(pflags.contains(OpenFlags::APPEND))
            .truncate(pflags.contains(OpenFlags::TRUNCATE));
        if pflags.contains(OpenFlags::CREATE) {
            if pflags.contains(OpenFlags::EXCLUDE) {
                options.create_new(true);
            } else {
                options.create(true);
            }
        }
        let file = options.open(&real).await.map_err(|e| status_of(&e))?;
        if !write && file.metadata().await.map(|m| m.is_dir()).unwrap_or(false) {
            return Err(StatusCode::Failure.with_message("Is a folder"));
        }
        let handle = self.add(OpenHandle::File { file, virt, written: 0, read: 0 });
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if let Some(OpenHandle::File { mut file, virt, written, read }) = self.handles.remove(&handle) {
            let _ = file.flush().await;
            if written > 0 {
                self.shared.log(Some(self.peer), format!("Uploaded {virt} ({written} bytes)"));
            } else if read > 0 {
                self.shared.log(Some(self.peer), format!("Downloaded {virt} ({read} bytes)"));
            }
        }
        Ok(ok(id))
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let Some(OpenHandle::File { file, read, .. }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.with_message("Bad handle"));
        };
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| status_of(&e))?;
        let mut buf = vec![0u8; len.min(256 * 1024) as usize];
        let n = file.read(&mut buf).await.map_err(|e| status_of(&e))?;
        if n == 0 {
            return Err(StatusCode::Eof.into());
        }
        buf.truncate(n);
        *read += n as u64;
        Ok(Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        if !self.shared.allow_write {
            return Err(read_only());
        }
        let Some(OpenHandle::File { file, written, .. }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.with_message("Bad handle"));
        };
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| status_of(&e))?;
        file.write_all(&data).await.map_err(|e| status_of(&e))?;
        *written += data.len() as u64;
        Ok(ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let (_, real) = self.real(&path)?;
        let meta = fs::metadata(&real).await.map_err(|e| status_of(&e))?;
        Ok(Attrs { id, attrs: FileAttributes::from(&meta) })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        match self.handles.get(&handle) {
            Some(OpenHandle::File { file, .. }) => {
                let meta = file.metadata().await.map_err(|e| status_of(&e))?;
                Ok(Attrs { id, attrs: FileAttributes::from(&meta) })
            }
            _ => Err(StatusCode::Failure.with_message("Bad handle")),
        }
    }

    // Times and permissions are not changed on this computer's files; the
    // request succeeds so uploads that set them do not fail.
    async fn setstat(&mut self, id: u32, _path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn fsetstat(&mut self, id: u32, _handle: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let (virt, real) = self.real(&path)?;
        let mut dir = fs::read_dir(&real).await.map_err(|e| status_of(&e))?;
        let mut entries = Vec::new();
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            // A link out of the folder is left out: it could not be opened.
            let child = if virt == "/" { format!("/{name}") } else { format!("{virt}/{name}") };
            if self.shared.root.real(&child).is_err() {
                continue;
            }
            let Ok(meta) = fs::metadata(entry.path()).await else { continue };
            entries.push(File::new(name, FileAttributes::from(&meta)));
        }
        let handle = self.add(OpenHandle::Dir { entries: Some(entries) });
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(OpenHandle::Dir { entries }) => match entries.take() {
                Some(files) if !files.is_empty() => Ok(Name { id, files }),
                _ => Err(StatusCode::Eof.into()),
            },
            _ => Err(StatusCode::Failure.with_message("Bad handle")),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        if !self.shared.allow_write {
            return Err(read_only());
        }
        let (virt, real) = self.real(&filename)?;
        fs::remove_file(&real).await.map_err(|e| status_of(&e))?;
        self.shared.log(Some(self.peer), format!("Deleted {virt}"));
        Ok(ok(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        if !self.shared.allow_write {
            return Err(read_only());
        }
        let (virt, real) = self.real(&path)?;
        fs::create_dir(&real).await.map_err(|e| status_of(&e))?;
        self.shared.log(Some(self.peer), format!("Created folder {virt}"));
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        if !self.shared.allow_write {
            return Err(read_only());
        }
        let (virt, real) = self.real(&path)?;
        if virt == "/" {
            return Err(StatusCode::PermissionDenied.into());
        }
        fs::remove_dir(&real).await.map_err(|e| status_of(&e))?;
        self.shared.log(Some(self.peer), format!("Removed folder {virt}"));
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let virt = Root::normalize("/", &path).map_err(|e| status_of(&e))?;
        Ok(Name { id, files: vec![File::dummy(virt)] })
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        if !self.shared.allow_write {
            return Err(read_only());
        }
        let (old_virt, old) = self.real(&oldpath)?;
        let (new_virt, new) = self.real(&newpath)?;
        if old_virt == "/" {
            return Err(StatusCode::PermissionDenied.into());
        }
        fs::rename(&old, &new).await.map_err(|e| status_of(&e))?;
        self.shared.log(Some(self.peer), format!("Renamed {old_virt} to {new_virt}"));
        Ok(ok(id))
    }
}
