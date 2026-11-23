//! How the file explorer reaches a connection's files: the SFTP protocol
//! where the server offers it, shell commands where it does not.
//!
//! SFTP works on accounts that have no shell (`ForceCommand internal-sftp`,
//! chroot jails) and on Windows OpenSSH, where `ls`, `base64` and friends do
//! not exist. Servers without the subsystem (Dropbear and many embedded
//! devices) keep the shell path the explorer has always used.
//!
//! Ported from ReachQ (github.com/CPTProgrammer/ReachQ, MIT), commit f3d9424,
//! and adapted: the session is kept on the connection instead of a separate
//! manager, a server without SFTP is remembered for a minute instead of being
//! probed before every operation, and a session that died is dropped so the
//! next operation opens a new one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::ChannelMsg;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::{Limits, RawSftpSession};
use russh_sftp::extensions;

use crate::ssh::client::SharedHandle;

/// How long to wait for the server to accept or refuse the subsystem.
const SUBSYSTEM_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the protocol handshake may take once the subsystem is accepted.
const INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one request may wait for its answer. Pipelined requests can sit
/// behind a full channel window on a slow link, so this is generous.
const REQUEST_TIMEOUT_SECS: u64 = 60;
/// A server that refused SFTP is asked again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// The request size every SFTP server must take (draft-ietf-secsh-filexfer
/// section 3), used when the server does not say what it takes.
const SAFE_CHUNK: u32 = 32 * 1024;
/// The most asked for in one request even when the server allows more; the
/// incoming packet limit of russh-sftp is 256 KiB including headers.
const MAX_CHUNK: u32 = 255 * 1024;

/// A live SFTP session on one SSH connection.
///
/// The raw session, not russh-sftp's `File` API: requests are matched by id,
/// so a transfer keeps many in flight with explicit offsets and fills the
/// channel's window instead of waiting a round trip per chunk.
pub struct SftpProtocol {
    pub session: Arc<RawSftpSession>,
    /// Bytes per read request.
    pub read_len: u32,
    /// Bytes per write request.
    pub write_len: u32,
}

impl SftpProtocol {
    /// How many requests a transfer keeps in flight: enough to cover about
    /// 2 MiB, the default channel window, and never fewer than 8.
    pub fn depth(chunk: u32) -> usize {
        ((2 * 1024 * 1024) / chunk.max(1) as usize).clamp(8, 64)
    }
}

impl Drop for SftpProtocol {
    fn drop(&mut self) {
        let _ = self.session.close_session();
    }
}

#[derive(Default)]
struct SlotState {
    session: Option<Arc<SftpProtocol>>,
    /// Set when the server refused SFTP: until then, use the shell.
    refused_until: Option<Instant>,
    /// An SCP session: never SFTP; copies go through `scp` (see `sftp::scp`).
    scp: bool,
}

/// Where a connection keeps its SFTP session. Lives on the connection, so it
/// goes when the connection does.
#[derive(Clone, Default)]
pub struct SftpSlot(Arc<tokio::sync::Mutex<SlotState>>);

impl SftpSlot {
    /// Make the connection an SCP one: shell commands to browse, `scp` to
    /// copy, never the SFTP subsystem.
    pub async fn set_scp(&self) {
        let mut state = self.0.lock().await;
        state.scp = true;
        state.session = None;
    }
}

/// One connection's files: its SSH handle and its SFTP session, if any.
#[derive(Clone)]
pub struct RemoteFs {
    pub handle: SharedHandle,
    slot: SftpSlot,
}

impl RemoteFs {
    pub fn new(handle: SharedHandle, slot: SftpSlot) -> Self {
        Self { handle, slot }
    }

    /// One that never tries SFTP, to test the shell path on a server that
    /// has both.
    #[cfg(test)]
    pub fn shell_only(handle: SharedHandle) -> Self {
        let state = SlotState {
            session: None,
            refused_until: Some(Instant::now() + Duration::from_secs(3600)),
            scp: false,
        };
        Self { handle, slot: SftpSlot(Arc::new(tokio::sync::Mutex::new(state))) }
    }

    /// The SFTP session, opened on first use. `None` means use the shell.
    ///
    /// The lock is held while opening, so operations started together open
    /// one session between them.
    pub async fn sftp(&self) -> Option<Arc<SftpProtocol>> {
        let mut state = self.slot.0.lock().await;
        if state.scp {
            return None;
        }
        if let Some(session) = &state.session {
            return Some(session.clone());
        }
        if state.refused_until.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        match open_sftp(&self.handle).await {
            Ok(protocol) => {
                tracing::info!(
                    "SFTP session open (read {} B, write {} B per request)",
                    protocol.read_len,
                    protocol.write_len
                );
                let protocol = Arc::new(protocol);
                state.session = Some(protocol.clone());
                state.refused_until = None;
                Some(protocol)
            }
            Err(reason) => {
                tracing::info!("No SFTP on this connection ({reason}); using shell commands");
                state.refused_until = Some(Instant::now() + RETRY_AFTER);
                None
            }
        }
    }

    /// Whether copies go through `scp` (an SCP session).
    pub async fn is_scp(&self) -> bool {
        self.slot.0.lock().await.scp
    }

    /// Note an SFTP error. One that means the session itself is gone (the
    /// channel closed, a request timed out) drops it, so the next operation
    /// opens a new one instead of failing the same way forever.
    pub async fn note_error(&self, error: &SftpError) {
        if !matches!(error, SftpError::Status(_)) {
            let mut state = self.slot.0.lock().await;
            state.session = None;
        }
    }
}

/// Open a channel, ask for the `sftp` subsystem and do the handshake.
async fn open_sftp(handle: &SharedHandle) -> Result<SftpProtocol, String> {
    let mut channel = {
        let guard = handle.lock().await;
        guard
            .channel_open_session()
            .await
            .map_err(|e| format!("channel open failed: {e}"))?
    };

    // request_subsystem() does not wait for the answer even with want_reply,
    // and OpenSSH keeps the channel open when it refuses a subsystem, so the
    // answer is read here; otherwise a refusal looks like a slow server.
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("subsystem request failed: {e}"))?;
    let accepted = tokio::time::timeout(SUBSYSTEM_REPLY_TIMEOUT, async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => break true,
                Some(ChannelMsg::Failure) | None => break false,
                Some(_) => {}
            }
        }
    })
    .await
    .map_err(|_| "no answer to the subsystem request".to_string())?;
    if !accepted {
        let _ = channel.close().await;
        return Err("the server refused the sftp subsystem".to_string());
    }

    let mut session = RawSftpSession::new(channel.into_stream());
    session.set_timeout(REQUEST_TIMEOUT_SECS);

    let version = match tokio::time::timeout(INIT_TIMEOUT, session.init()).await {
        Ok(Ok(version)) => version,
        Ok(Err(e)) => {
            let _ = session.close_session();
            return Err(format!("handshake failed: {e}"));
        }
        Err(_) => {
            let _ = session.close_session();
            return Err("handshake timed out".to_string());
        }
    };

    let (mut read_len, mut write_len) = (SAFE_CHUNK, SAFE_CHUNK);
    if version.extensions.get(extensions::LIMITS).is_some_and(|v| v == "1") {
        if let Ok(ext) = session.limits().await {
            let limits = Limits::from(ext);
            read_len = chunk_within(limits.read_len);
            write_len = chunk_within(limits.write_len);
            session.set_limits(limits);
        }
    }

    Ok(SftpProtocol { session: Arc::new(session), read_len, write_len })
}

/// The request size to use given what the server says it takes.
fn chunk_within(server_limit: Option<u64>) -> u32 {
    match server_limit {
        Some(limit) => limit.min(u64::from(MAX_CHUNK)).max(1) as u32,
        None => SAFE_CHUNK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_says_nothing_gets_the_size_every_server_takes() {
        assert_eq!(chunk_within(None), 32 * 1024);
    }

    #[test]
    fn a_server_limit_is_kept_but_never_above_what_the_client_can_receive() {
        assert_eq!(chunk_within(Some(64 * 1024)), 64 * 1024);
        assert_eq!(chunk_within(Some(4 * 1024 * 1024)), 255 * 1024);
    }

    #[test]
    fn enough_requests_fly_to_fill_the_window() {
        assert_eq!(SftpProtocol::depth(32 * 1024), 64);
        assert_eq!(SftpProtocol::depth(255 * 1024), 8);
        assert_eq!(SftpProtocol::depth(64 * 1024), 32);
    }
}
