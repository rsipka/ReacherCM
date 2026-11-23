//! Local file servers (TFTP, FTP, SFTP, HTTP), as MobaXterm offers them: a
//! folder of this computer served to devices on the network, for a switch
//! that pulls its firmware over TFTP, a camera that uploads over FTP, or a
//! quick download from a phone over HTTP. Desktop only.
//!
//! Each server is started with a whole [`ServerConfig`] from the page, which
//! keeps the settings (in the vault, with the other app settings); nothing is
//! stored here but the SFTP host key. Every path a client names is resolved
//! inside the served folder by [`fsroot::Root`], which refuses `..` and
//! symbolic links that lead out of it.

pub mod fsroot;
pub mod ftp;
pub mod http;
pub mod net;
pub mod sftp;
pub mod tftp;

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The kinds of server, one of each at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    Tftp,
    Ftp,
    Sftp,
    Http,
}

impl ServerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tftp => "TFTP",
            Self::Ftp => "FTP",
            Self::Sftp => "SFTP",
            Self::Http => "HTTP",
        }
    }

    /// TFTP is the only one on UDP.
    pub fn is_udp(self) -> bool {
        matches!(self, Self::Tftp)
    }

    /// The standard port, then the usual unprivileged stand-ins for it, in
    /// the order a free port is looked for when the one asked for is taken.
    pub fn port_candidates(self) -> &'static [u16] {
        match self {
            Self::Tftp => &[69, 6969, 1069],
            Self::Ftp => &[21, 2121, 2021],
            Self::Sftp => &[22, 2222, 2022],
            Self::Http => &[80, 8080, 8000, 8888],
        }
    }
}

/// One login a server accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerUser {
    pub username: String,
    pub password: String,
}

/// What a server is started with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub kind: ServerKind,
    /// The served folder.
    pub root: String,
    /// The address to listen on: one interface's, 127.0.0.1, or 0.0.0.0 for all.
    pub bind: String,
    pub port: u16,
    #[serde(default)]
    pub users: Vec<ServerUser>,
    /// FTP and HTTP: no login needed. TFTP never has one; SFTP always does.
    #[serde(default)]
    pub anonymous: bool,
    /// Uploads, deletes, renames and new folders.
    #[serde(default)]
    pub allow_write: bool,
}

/// One line of a server's connection log.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub kind: ServerKind,
    /// Local time, HH:MM:SS.
    pub time: String,
    /// The client's address, or empty for the server's own lines.
    pub peer: String,
    pub message: String,
}

const LOG_LINES: usize = 500;

/// Where log lines go: kept for the page to read back, and sent to it as
/// they happen.
pub type LogEmitter = Arc<dyn Fn(&LogEntry) + Send + Sync>;

/// What a running server's tasks share.
pub struct Shared {
    pub kind: ServerKind,
    pub root: fsroot::Root,
    pub users: Vec<ServerUser>,
    pub anonymous: bool,
    pub allow_write: bool,
    log: Mutex<VecDeque<LogEntry>>,
    emit: LogEmitter,
}

impl Shared {
    pub fn new(config: &ServerConfig, root: fsroot::Root, emit: LogEmitter) -> Self {
        Self {
            kind: config.kind,
            root,
            users: config
                .users
                .iter()
                .filter(|u| !u.username.is_empty())
                .cloned()
                .collect(),
            anonymous: config.anonymous,
            allow_write: config.allow_write,
            log: Mutex::new(VecDeque::new()),
            emit,
        }
    }

    pub fn log(&self, peer: Option<SocketAddr>, message: impl Into<String>) {
        let entry = LogEntry {
            kind: self.kind,
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            peer: peer.map(|p| p.to_string()).unwrap_or_default(),
            message: message.into(),
        };
        (self.emit)(&entry);
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        if log.len() == LOG_LINES {
            log.pop_front();
        }
        log.push_back(entry);
    }

    pub fn log_lines(&self) -> Vec<LogEntry> {
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Whether a username and password log in. Compared in constant time.
    pub fn check_login(&self, username: &str, password: &str) -> bool {
        use subtle::ConstantTimeEq;
        let mut ok = false;
        for user in &self.users {
            let name = user.username.as_bytes().ct_eq(username.as_bytes());
            let pass = user.password.as_bytes().ct_eq(password.as_bytes());
            ok |= bool::from(name & pass);
        }
        ok
    }

    /// The username an anonymous FTP or HTTP client is allowed in as.
    pub fn is_anonymous_name(&self, username: &str) -> bool {
        self.anonymous && matches!(username.to_ascii_lowercase().as_str(), "anonymous" | "ftp")
    }
}

/// A running server, as the page sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub kind: ServerKind,
    pub running: bool,
    pub bind: String,
    pub port: u16,
    pub root: String,
    /// The addresses clients can reach it on: the bound one, or every
    /// interface's when it listens on all of them.
    pub addresses: Vec<String>,
    /// SFTP: the host key's SHA-256 fingerprint, for clients to check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub log: Vec<LogEntry>,
}

struct Running {
    cancel: CancellationToken,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
    bind: IpAddr,
    port: u16,
    fingerprint: Option<String>,
}

/// The servers that are running, at most one of each kind.
#[derive(Default)]
pub struct LocalServerManager {
    running: HashMap<ServerKind, Running>,
    /// Logs of servers since stopped, so the page still shows why one ended.
    stopped_logs: HashMap<ServerKind, Arc<Shared>>,
}

impl LocalServerManager {
    /// Starts a server, stopping the one of the same kind first.
    pub async fn start(&mut self, config: ServerConfig, emit: LogEmitter) -> Result<ServerStatus, String> {
        self.stop(config.kind);
        let bind: IpAddr = config
            .bind
            .trim()
            .parse()
            .map_err(|_| format!("Not an IP address: {}", config.bind))?;
        if config.port == 0 {
            return Err("Choose a port between 1 and 65535".into());
        }
        let root = fsroot::Root::new(&config.root)?;
        if config.kind != ServerKind::Tftp
            && !(config.anonymous && config.kind != ServerKind::Sftp)
            && !config.users.iter().any(|u| !u.username.is_empty())
        {
            return Err("Add a user, or allow anonymous access".into());
        }
        let shared = Arc::new(Shared::new(&config, root, emit));
        let addr = SocketAddr::new(bind, config.port);
        let cancel = CancellationToken::new();
        let mut fingerprint = None;

        let task = match config.kind {
            ServerKind::Tftp => {
                let socket = net::bind_udp(addr).map_err(|e| net::bind_error(config.kind, addr, &e))?;
                tokio::spawn(tftp::run(socket, shared.clone(), cancel.clone()))
            }
            ServerKind::Ftp => {
                let listener = net::bind_tcp(addr).map_err(|e| net::bind_error(config.kind, addr, &e))?;
                tokio::spawn(ftp::run(listener, shared.clone(), cancel.clone()))
            }
            ServerKind::Sftp => {
                let key = sftp::host_key()?;
                fingerprint = Some(sftp::fingerprint(&key));
                let listener = net::bind_tcp(addr).map_err(|e| net::bind_error(config.kind, addr, &e))?;
                tokio::spawn(sftp::run(listener, key, shared.clone(), cancel.clone()))
            }
            ServerKind::Http => {
                let listener = net::bind_tcp(addr).map_err(|e| net::bind_error(config.kind, addr, &e))?;
                tokio::spawn(http::run(listener, shared.clone(), cancel.clone()))
            }
        };

        let access = if config.kind == ServerKind::Tftp {
            "no login (TFTP has none)".to_string()
        } else {
            let mut parts = vec![format!("{} user(s)", shared.users.len())];
            if shared.anonymous && config.kind != ServerKind::Sftp {
                parts.push("anonymous allowed".into());
            }
            parts.join(", ")
        };
        shared.log(
            None,
            format!(
                "Started on {addr}, serving {}, {}, {access}",
                shared.root.path().display(),
                if shared.allow_write { "read and write" } else { "read only" },
            ),
        );
        self.stopped_logs.remove(&config.kind);
        self.running.insert(
            config.kind,
            Running { cancel, task, shared, bind, port: config.port, fingerprint },
        );
        Ok(self.status(config.kind))
    }

    pub fn stop(&mut self, kind: ServerKind) {
        if let Some(running) = self.running.remove(&kind) {
            running.cancel.cancel();
            running.task.abort();
            running.shared.log(None, "Stopped");
            self.stopped_logs.insert(kind, running.shared);
        }
    }

    pub fn stop_all(&mut self) {
        let kinds: Vec<_> = self.running.keys().copied().collect();
        for kind in kinds {
            self.stop(kind);
        }
    }

    pub fn status(&mut self, kind: ServerKind) -> ServerStatus {
        // A server whose task ended on its own (its socket failed) is stopped.
        if self.running.get(&kind).is_some_and(|r| r.task.is_finished()) {
            self.stop(kind);
        }
        match self.running.get(&kind) {
            Some(r) => ServerStatus {
                kind,
                running: true,
                bind: r.bind.to_string(),
                port: r.port,
                root: r.shared.root.path().display().to_string(),
                addresses: net::reachable_addresses(r.bind),
                fingerprint: r.fingerprint.clone(),
                log: r.shared.log_lines(),
            },
            None => ServerStatus {
                kind,
                running: false,
                bind: String::new(),
                port: 0,
                root: String::new(),
                addresses: Vec::new(),
                fingerprint: None,
                log: self.stopped_logs.get(&kind).map(|s| s.log_lines()).unwrap_or_default(),
            },
        }
    }

    pub fn statuses(&mut self) -> Vec<ServerStatus> {
        [ServerKind::Tftp, ServerKind::Ftp, ServerKind::Sftp, ServerKind::Http]
            .into_iter()
            .map(|k| self.status(k))
            .collect()
    }

    pub fn clear_log(&mut self, kind: ServerKind) {
        if let Some(r) = self.running.get(&kind) {
            r.shared.log.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
        self.stopped_logs.remove(&kind);
    }

    /// Whether this app's own server holds the address already, so a port
    /// check does not call it taken by someone else.
    pub fn holds(&self, kind: ServerKind, port: u16) -> bool {
        self.running.get(&kind).is_some_and(|r| r.port == port)
    }
}

#[cfg(test)]
mod tests;
