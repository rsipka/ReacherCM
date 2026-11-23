//! FTP and FTPS sessions: their files, shown in the full-pane file manager
//! the way an SSH connection's are. The `sftp_*` commands send a connection
//! id that belongs to one of these here (see `ipc::sftp_commands`), so the
//! file manager needs no FTP of its own.
//!
//! An FTP connection does one thing at a time, so each open session keeps a
//! few logged-in connections and lends one to each operation: the folder
//! can be browsed while a file is copied. One that the server closed while
//! idle is replaced and the operation tried again.

pub mod client;
pub mod list;
pub mod ops;
pub mod tls;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};

pub use client::{Client, FtpError, Params, Security};

/// How many connections one session opens at most: many servers allow
/// only a few per address.
const MAX_CONNECTIONS: usize = 3;

/// An FTP session's own settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FtpOptions {
    /// `explicit` (unset: AUTH TLS, refused without), `implicit` (TLS from
    /// the start, usually port 990) or `none` (plain FTP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<String>,
    /// SHA-256 fingerprints (`AB:CD:..`) of certificates this session
    /// trusts although the system does not. Changed only by answering the
    /// prompt; an editor's save keeps the stored ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certs: Vec<String>,
}

impl FtpOptions {
    pub fn is_empty(&self) -> bool {
        self.tls.is_none() && self.certs.is_empty()
    }
}

/// One open FTP session.
pub struct FtpConn {
    params: Params,
    idle: Mutex<Vec<Client>>,
    slots: Semaphore,
    pub home: String,
}

/// A connection lent out; given back with [`FtpConn::give_back`].
pub struct Lent<'a> {
    pub client: Client,
    /// Just made, so a failure is not the idle timeout's doing.
    fresh: bool,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl FtpConn {
    /// Log in once, which also checks the login.
    pub async fn open(params: Params) -> Result<Self, FtpError> {
        let first = Client::connect(&params).await?;
        let home = first.home.clone();
        Ok(Self { params, idle: Mutex::new(vec![first]), slots: Semaphore::new(MAX_CONNECTIONS), home })
    }

    pub async fn lend(&self) -> Result<Lent<'_>, FtpError> {
        let permit = self.slots.acquire().await.map_err(|_| FtpError::Other("The FTP session was closed".into()))?;
        let idle = self.idle.lock().await.pop();
        let (client, fresh) = match idle {
            Some(c) => (c, false),
            None => (Client::connect(&self.params).await?, true),
        };
        Ok(Lent { client, fresh, _permit: permit })
    }

    async fn fresh<'a>(&'a self, old: Lent<'a>) -> Result<Lent<'a>, FtpError> {
        let Lent { _permit, .. } = old;
        Ok(Lent { client: Client::connect(&self.params).await?, fresh: true, _permit })
    }

    pub async fn give_back(&self, lent: Lent<'_>) {
        if !lent.client.broken {
            self.idle.lock().await.push(lent.client);
        }
    }

    pub async fn close(&self) {
        let all: Vec<Client> = std::mem::take(&mut *self.idle.lock().await);
        for c in all {
            c.quit().await;
        }
    }
}

/// Run `op` on a lent connection; once more on a new one when the
/// connection itself failed (an idle one the server had closed).
macro_rules! with_client {
    ($conn:expr, |$c:ident| $body:expr) => {{
        let conn: &crate::ftp::FtpConn = $conn;
        let mut lent = conn.lend().await?;
        let mut result = {
            let $c = &mut lent.client;
            $body.await
        };
        if matches!(&result, Err(e) if e.is_connection()) && !lent.fresh {
            lent = conn.fresh(lent).await?;
            result = {
                let $c = &mut lent.client;
                $body.await
            };
        }
        conn.give_back(lent).await;
        result
    }};
}
pub(crate) use with_client;

/// The open FTP sessions, by connection id.
#[derive(Default)]
pub struct FtpManager {
    conns: std::sync::Mutex<HashMap<String, Arc<FtpConn>>>,
}

impl FtpManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<FtpConn>> {
        self.conns.lock().ok()?.get(id).cloned()
    }

    pub fn insert(&self, id: String, conn: FtpConn) {
        if let Ok(mut m) = self.conns.lock() {
            m.insert(id, Arc::new(conn));
        }
    }

    pub fn remove(&self, id: &str) -> Option<Arc<FtpConn>> {
        self.conns.lock().ok()?.remove(id)
    }
}
