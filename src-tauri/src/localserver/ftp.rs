//! FTP (RFC 959) with the extensions clients expect today: passive mode for
//! IPv4 and IPv6 (PASV, EPSV), SIZE, MDTM, REST, MLSD/MLST and UTF-8 names.
//! Plain FTP only: no TLS, which the page says.
//!
//! A data connection is accepted only from the address the control
//! connection came from, and active mode (PORT, EPRT) only connects back to
//! it, so the server cannot be used to reach a third machine (the "FTP
//! bounce" attack).

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::fsroot::{io_reason, Root};
use super::Shared;

const DATA_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_LOGIN_FAILURES: u32 = 3;

pub async fn run(listener: TcpListener, shared: Arc<Shared>, cancel: CancellationToken) {
    loop {
        let (stream, peer) = tokio::select! {
            _ = cancel.cancelled() => return,
            r = listener.accept() => match r {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("FTP accept: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let shared = shared.clone();
        let cancel = cancel.child_token();
        tokio::spawn(async move {
            shared.log(Some(peer), "Connected");
            let result = tokio::select! {
                _ = cancel.cancelled() => Ok(()),
                r = Session::new(stream, peer, shared.clone()).serve() => r,
            };
            match result {
                Ok(()) => shared.log(Some(peer), "Disconnected"),
                Err(e) => shared.log(Some(peer), format!("Disconnected: {e}")),
            }
        });
    }
}

enum Data {
    None,
    Passive(TcpListener),
    Active(SocketAddr),
}

struct Session {
    control: BufReader<TcpStream>,
    peer: SocketAddr,
    local: SocketAddr,
    shared: Arc<Shared>,
    user: Option<String>,
    logged_in: bool,
    failures: u32,
    cwd: String,
    data: Data,
    rest: u64,
    rename_from: Option<PathBuf>,
}

impl Session {
    fn new(stream: TcpStream, peer: SocketAddr, shared: Arc<Shared>) -> Self {
        let local = stream.local_addr().unwrap_or(peer);
        Self {
            control: BufReader::new(stream),
            peer,
            local,
            shared,
            user: None,
            logged_in: false,
            failures: 0,
            cwd: "/".into(),
            data: Data::None,
            rest: 0,
            rename_from: None,
        }
    }

    async fn reply(&mut self, code: u16, text: &str) -> std::io::Result<()> {
        let line = format!("{code} {text}\r\n");
        self.control.get_mut().write_all(line.as_bytes()).await
    }

    async fn serve(mut self) -> std::io::Result<()> {
        self.reply(220, "Reacher CM FTP server ready").await?;
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = tokio::time::timeout(IDLE_TIMEOUT, (&mut self.control).take(8192).read_until(b'\n', &mut line))
                .await
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "idle timeout"))??;
            if read == 0 {
                return Ok(());
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\r', '\n']);
            let (cmd, arg) = match text.split_once(' ') {
                Some((c, a)) => (c.to_ascii_uppercase(), a.to_string()),
                None => (text.to_ascii_uppercase(), String::new()),
            };
            if !self.command(&cmd, &arg).await? {
                return Ok(());
            }
        }
    }

    fn writable(&self) -> bool {
        self.shared.allow_write
    }

    fn resolve(&self, arg: &str) -> std::io::Result<(String, PathBuf)> {
        let virt = Root::normalize(&self.cwd, arg)?;
        let real = self.shared.root.real(&virt)?;
        Ok((virt, real))
    }

    /// Handles one command; false ends the session.
    async fn command(&mut self, cmd: &str, arg: &str) -> std::io::Result<bool> {
        // Commands allowed before logging in.
        match cmd {
            "USER" => {
                self.user = Some(arg.to_string());
                self.logged_in = false;
                if self.shared.is_anonymous_name(arg) {
                    self.reply(331, "Anonymous login, send any password").await?;
                } else {
                    self.reply(331, "Password required").await?;
                }
                return Ok(true);
            }
            "PASS" => {
                let Some(user) = self.user.clone() else {
                    self.reply(503, "Send USER first").await?;
                    return Ok(true);
                };
                if self.shared.is_anonymous_name(&user) || self.shared.check_login(&user, arg) {
                    self.logged_in = true;
                    self.shared.log(Some(self.peer), format!("Logged in as {user}"));
                    self.reply(230, "Logged in").await?;
                } else {
                    self.failures += 1;
                    self.shared.log(Some(self.peer), format!("Login failed for {user}"));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    self.reply(530, "Login incorrect").await?;
                    if self.failures >= MAX_LOGIN_FAILURES {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            "QUIT" => {
                self.reply(221, "Goodbye").await?;
                return Ok(false);
            }
            "AUTH" => {
                self.reply(502, "TLS is not available on this server").await?;
                return Ok(true);
            }
            "FEAT" => {
                let feat = "211-Features:\r\n EPSV\r\n PASV\r\n SIZE\r\n MDTM\r\n REST STREAM\r\n MLST type*;size*;modify*;\r\n UTF8\r\n211 End\r\n";
                self.control.get_mut().write_all(feat.as_bytes()).await?;
                return Ok(true);
            }
            "SYST" => {
                self.reply(215, "UNIX Type: L8").await?;
                return Ok(true);
            }
            "NOOP" => {
                self.reply(200, "OK").await?;
                return Ok(true);
            }
            "OPTS" => {
                if arg.to_ascii_uppercase().starts_with("UTF8") {
                    self.reply(200, "UTF-8 is always on").await?;
                } else {
                    self.reply(501, "Option not understood").await?;
                }
                return Ok(true);
            }
            _ => {}
        }
        if !self.logged_in {
            self.reply(530, "Please log in with USER and PASS").await?;
            return Ok(true);
        }
        match cmd {
            "PWD" | "XPWD" => {
                let text = format!("\"{}\" is the current folder", self.cwd.replace('"', "\"\""));
                self.reply(257, &text).await?;
            }
            "CWD" | "XCWD" | "CDUP" | "XCUP" => {
                let target = if cmd.ends_with("UP") { ".." } else { arg };
                match self.resolve(target) {
                    Ok((virt, real)) if real.is_dir() => {
                        self.cwd = virt;
                        self.reply(250, "Folder changed").await?;
                    }
                    Ok(_) => self.reply(550, "No such folder").await?,
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                }
            }
            "TYPE" => self.reply(200, "Type set").await?,
            "MODE" | "STRU" => {
                if matches!(arg.to_ascii_uppercase().as_str(), "S" | "F") {
                    self.reply(200, "OK").await?;
                } else {
                    self.reply(504, "Not supported").await?;
                }
            }
            "ALLO" => self.reply(202, "No storage allocation needed").await?,
            "PASV" => {
                let IpAddr::V4(ip) = self.local.ip() else {
                    self.reply(522, "Use EPSV over IPv6").await?;
                    return Ok(true);
                };
                let listener = TcpListener::bind(SocketAddr::new(self.local.ip(), 0)).await?;
                let port = listener.local_addr()?.port();
                self.data = Data::Passive(listener);
                let [a, b, c, d] = ip.octets();
                let text = format!("Entering Passive Mode ({a},{b},{c},{d},{},{})", port >> 8, port & 0xff);
                self.reply(227, &text).await?;
            }
            "EPSV" => {
                if arg.eq_ignore_ascii_case("ALL") {
                    self.reply(200, "EPSV ALL accepted").await?;
                    return Ok(true);
                }
                let listener = TcpListener::bind(SocketAddr::new(self.local.ip(), 0)).await?;
                let port = listener.local_addr()?.port();
                self.data = Data::Passive(listener);
                self.reply(229, &format!("Entering Extended Passive Mode (|||{port}|)")).await?;
            }
            "PORT" | "EPRT" => {
                let addr = if cmd == "PORT" { parse_port(arg) } else { parse_eprt(arg) };
                match addr {
                    Some(addr) if addr.ip() == self.peer.ip() => {
                        self.data = Data::Active(addr);
                        self.reply(200, "Active mode set").await?;
                    }
                    Some(_) => self.reply(504, "Data connections go to your own address only").await?,
                    None => self.reply(501, "Bad address").await?,
                }
            }
            "REST" => match arg.parse::<u64>() {
                Ok(offset) => {
                    self.rest = offset;
                    self.reply(350, &format!("Restarting at {offset}")).await?;
                }
                Err(_) => self.reply(501, "Bad offset").await?,
            },
            "LIST" | "NLST" | "MLSD" => self.list(cmd, arg).await?,
            "MLST" => {
                let target = if arg.is_empty() { "." } else { arg };
                match self.resolve(target) {
                    Ok((virt, real)) => match fs::metadata(&real).await {
                        Ok(meta) => {
                            let fact = mlsx_fact(&meta, &virt);
                            let text = format!("250-Listing {virt}\r\n {fact}\r\n250 End\r\n");
                            self.control.get_mut().write_all(text.as_bytes()).await?;
                        }
                        Err(e) => self.reply(550, io_reason(&e)).await?,
                    },
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                }
            }
            "SIZE" => match self.resolve(arg) {
                Ok((_, real)) => match fs::metadata(&real).await {
                    Ok(m) if m.is_file() => self.reply(213, &m.len().to_string()).await?,
                    Ok(_) => self.reply(550, "Not a file").await?,
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                },
                Err(e) => self.reply(550, io_reason(&e)).await?,
            },
            "MDTM" => match self.resolve(arg) {
                Ok((_, real)) => match fs::metadata(&real).await.and_then(|m| m.modified()) {
                    Ok(t) => self.reply(213, &mdtm(t)).await?,
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                },
                Err(e) => self.reply(550, io_reason(&e)).await?,
            },
            "RETR" => self.retrieve(arg).await?,
            "STOR" | "APPE" => self.store(arg, cmd == "APPE").await?,
            "DELE" | "RMD" | "XRMD" | "MKD" | "XMKD" | "RNFR" | "RNTO" if !self.writable() => {
                self.reply(550, "Permission denied: this server is read only").await?;
            }
            "DELE" => match self.resolve(arg) {
                Ok((virt, real)) => match fs::remove_file(&real).await {
                    Ok(()) => {
                        self.shared.log(Some(self.peer), format!("Deleted {virt}"));
                        self.reply(250, "Deleted").await?;
                    }
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                },
                Err(e) => self.reply(550, io_reason(&e)).await?,
            },
            "RMD" | "XRMD" => match self.resolve(arg) {
                Ok((virt, _)) if virt == "/" => self.reply(550, "Permission denied").await?,
                Ok((virt, real)) => match fs::remove_dir(&real).await {
                    Ok(()) => {
                        self.shared.log(Some(self.peer), format!("Removed folder {virt}"));
                        self.reply(250, "Folder removed").await?;
                    }
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                },
                Err(e) => self.reply(550, io_reason(&e)).await?,
            },
            "MKD" | "XMKD" => match self.resolve(arg) {
                Ok((virt, real)) => match fs::create_dir(&real).await {
                    Ok(()) => {
                        self.shared.log(Some(self.peer), format!("Created folder {virt}"));
                        self.reply(257, &format!("\"{}\" created", virt.replace('"', "\"\""))).await?;
                    }
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                },
                Err(e) => self.reply(550, io_reason(&e)).await?,
            },
            "RNFR" => match self.resolve(arg) {
                Ok((_, real)) if real.exists() => {
                    self.rename_from = Some(real);
                    self.reply(350, "Ready for the new name").await?;
                }
                Ok(_) => self.reply(550, "No such file or folder").await?,
                Err(e) => self.reply(550, io_reason(&e)).await?,
            },
            "RNTO" => {
                let Some(from) = self.rename_from.take() else {
                    self.reply(503, "Send RNFR first").await?;
                    return Ok(true);
                };
                match self.resolve(arg) {
                    Ok((virt, to)) => match fs::rename(&from, &to).await {
                        Ok(()) => {
                            let old = self.shared.root.virtual_of(&from);
                            self.shared.log(Some(self.peer), format!("Renamed {old} to {virt}"));
                            self.reply(250, "Renamed").await?;
                        }
                        Err(e) => self.reply(550, io_reason(&e)).await?,
                    },
                    Err(e) => self.reply(550, io_reason(&e)).await?,
                }
            }
            "ABOR" => {
                self.data = Data::None;
                self.reply(226, "Nothing to abort").await?;
            }
            "STAT" if arg.is_empty() => self.reply(211, "Reacher CM FTP server").await?,
            "HELP" => self.reply(214, "See RFC 959").await?,
            "SITE" => self.reply(502, "No SITE commands").await?,
            _ => self.reply(502, "Command not implemented").await?,
        }
        Ok(true)
    }

    /// Opens the data connection the client set up with PASV/EPSV or PORT.
    async fn open_data(&mut self) -> std::io::Result<Option<TcpStream>> {
        match std::mem::replace(&mut self.data, Data::None) {
            Data::None => {
                self.reply(425, "Use PASV or PORT first").await?;
                Ok(None)
            }
            Data::Passive(listener) => {
                self.reply(150, "Opening data connection").await?;
                let deadline = tokio::time::Instant::now() + DATA_TIMEOUT;
                loop {
                    match tokio::time::timeout_at(deadline, listener.accept()).await {
                        Ok(Ok((stream, from))) if from.ip() == self.peer.ip() => return Ok(Some(stream)),
                        Ok(Ok((_, from))) => {
                            self.shared.log(Some(self.peer), format!("Refused a data connection from {from}"));
                        }
                        Ok(Err(_)) | Err(_) => {
                            self.reply(425, "No data connection").await?;
                            return Ok(None);
                        }
                    }
                }
            }
            Data::Active(addr) => {
                self.reply(150, "Opening data connection").await?;
                match tokio::time::timeout(DATA_TIMEOUT, TcpStream::connect(addr)).await {
                    Ok(Ok(stream)) => Ok(Some(stream)),
                    _ => {
                        self.reply(425, "Could not connect to you").await?;
                        Ok(None)
                    }
                }
            }
        }
    }

    async fn list(&mut self, cmd: &str, arg: &str) -> std::io::Result<()> {
        // Options like "-la" some clients send are ls's, not paths.
        let target = arg
            .split_whitespace()
            .filter(|w| !w.starts_with('-'))
            .collect::<Vec<_>>()
            .join(" ");
        let target = if target.is_empty() { ".".to_string() } else { target };
        let (virt, real) = match self.resolve(&target) {
            Ok(r) => r,
            Err(e) => return self.reply(550, io_reason(&e)).await,
        };
        let meta = match fs::metadata(&real).await {
            Ok(m) => m,
            Err(e) => return self.reply(550, io_reason(&e)).await,
        };
        let mut lines = Vec::new();
        if meta.is_dir() {
            let mut dir = match fs::read_dir(&real).await {
                Ok(d) => d,
                Err(e) => return self.reply(550, io_reason(&e)).await,
            };
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                // A link out of the folder is not listed; it could not be opened.
                if self.shared.root.real(&format!("{virt}/{name}")).is_err() {
                    continue;
                }
                let Ok(meta) = fs::metadata(entry.path()).await else { continue };
                lines.push(format_entry(cmd, &name, &meta));
            }
        } else if cmd == "MLSD" {
            return self.reply(501, "Not a folder").await;
        } else {
            let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            lines.push(format_entry(cmd, &name, &meta));
        }
        let Some(mut stream) = self.open_data().await? else { return Ok(()) };
        let mut body = lines.join("\r\n");
        if !body.is_empty() {
            body.push_str("\r\n");
        }
        let ok = stream.write_all(body.as_bytes()).await.is_ok() && stream.shutdown().await.is_ok();
        if ok {
            self.reply(226, "Listing sent").await
        } else {
            self.reply(426, "Connection closed; listing aborted").await
        }
    }

    async fn retrieve(&mut self, arg: &str) -> std::io::Result<()> {
        let offset = std::mem::take(&mut self.rest);
        let (virt, real) = match self.resolve(arg) {
            Ok(r) => r,
            Err(e) => return self.reply(550, io_reason(&e)).await,
        };
        let mut file = match fs::File::open(&real).await {
            Ok(f) => f,
            Err(e) => return self.reply(550, io_reason(&e)).await,
        };
        if file.metadata().await.map(|m| m.is_dir()).unwrap_or(false) {
            return self.reply(550, "Is a folder").await;
        }
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset)).await?;
        }
        let Some(mut stream) = self.open_data().await? else { return Ok(()) };
        self.shared.log(Some(self.peer), format!("Download {virt}"));
        match copy_and_close(&mut file, &mut stream).await {
            Ok(bytes) => {
                self.shared.log(Some(self.peer), format!("Download of {virt} done: {bytes} bytes"));
                self.reply(226, "Transfer complete").await
            }
            Err(e) => {
                self.shared.log(Some(self.peer), format!("Download of {virt} failed: {e}"));
                self.reply(426, "Transfer aborted").await
            }
        }
    }

    async fn store(&mut self, arg: &str, append: bool) -> std::io::Result<()> {
        let offset = std::mem::take(&mut self.rest);
        if !self.writable() {
            return self.reply(550, "Permission denied: this server is read only").await;
        }
        let (virt, real) = match self.resolve(arg) {
            Ok(r) => r,
            Err(e) => return self.reply(550, io_reason(&e)).await,
        };
        if real.is_dir() {
            return self.reply(550, "Is a folder").await;
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true);
        if append {
            options.append(true);
        } else if offset == 0 {
            options.truncate(true);
        }
        let mut file = match options.open(&real).await {
            Ok(f) => f,
            Err(e) => return self.reply(550, io_reason(&e)).await,
        };
        if offset > 0 && !append {
            file.set_len(offset).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
        }
        let Some(mut stream) = self.open_data().await? else { return Ok(()) };
        self.shared.log(Some(self.peer), format!("Upload {virt}"));
        let result = async {
            let bytes = tokio::io::copy(&mut stream, &mut file).await?;
            file.flush().await?;
            Ok::<u64, std::io::Error>(bytes)
        }
        .await;
        match result {
            Ok(bytes) => {
                self.shared.log(Some(self.peer), format!("Upload of {virt} done: {bytes} bytes"));
                self.reply(226, "Transfer complete").await
            }
            Err(e) => {
                self.shared.log(Some(self.peer), format!("Upload of {virt} failed: {e}"));
                self.reply(426, "Transfer aborted").await
            }
        }
    }
}

async fn copy_and_close<R, W>(from: &mut R, to: &mut W) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let bytes = tokio::io::copy(from, to).await?;
    to.shutdown().await?;
    Ok(bytes)
}

/// `h1,h2,h3,h4,p1,p2`.
pub fn parse_port(arg: &str) -> Option<SocketAddr> {
    let nums: Vec<u8> = arg.split(',').map(|n| n.trim().parse().ok()).collect::<Option<_>>()?;
    if nums.len() != 6 {
        return None;
    }
    let ip = IpAddr::from([nums[0], nums[1], nums[2], nums[3]]);
    let port = u16::from(nums[4]) << 8 | u16::from(nums[5]);
    Some(SocketAddr::new(ip, port))
}

/// `|1|10.0.0.5|6000|` or `|2|::1|6000|`.
pub fn parse_eprt(arg: &str) -> Option<SocketAddr> {
    let delim = arg.chars().next()?;
    let parts: Vec<&str> = arg.split(delim).collect();
    if parts.len() < 5 {
        return None;
    }
    let ip: IpAddr = parts[2].parse().ok()?;
    let port: u16 = parts[3].parse().ok()?;
    Some(SocketAddr::new(ip, port))
}

fn mdtm(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).format("%Y%m%d%H%M%S").to_string()
}

fn mlsx_fact(meta: &std::fs::Metadata, name: &str) -> String {
    let kind = if meta.is_dir() { "dir" } else { "file" };
    let modify = meta.modified().map(mdtm).unwrap_or_default();
    if meta.is_dir() {
        format!("type={kind};modify={modify}; {name}")
    } else {
        format!("type={kind};size={};modify={modify}; {name}", meta.len())
    }
}

pub fn format_entry(cmd: &str, name: &str, meta: &std::fs::Metadata) -> String {
    match cmd {
        "NLST" => name.to_string(),
        "MLSD" => mlsx_fact(meta, name),
        _ => {
            let perms = if meta.is_dir() { "drwxr-xr-x" } else { "-rw-r--r--" };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let date = chrono::DateTime::<chrono::Local>::from(modified);
            let age = SystemTime::now().duration_since(modified).unwrap_or_default();
            // ls shows the time for the last six months, the year before that.
            let when = if age < Duration::from_secs(180 * 24 * 3600) {
                date.format("%b %d %H:%M").to_string()
            } else {
                date.format("%b %d  %Y").to_string()
            };
            format!("{perms} 1 owner group {:>12} {when} {name}", meta.len())
        }
    }
}
