//! The WireGuard tunnels that are on, by profile id. A tunnel is brought up
//! on purpose (the user confirmed it, or pressed Connect in the list) and
//! stays up until turned off or the app quits; connections only use it.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Mutex;

use super::config::WgConfig;
use super::stack::{Tunnel, WgStream};

/// How long the server may take to answer the first handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

struct Running {
    tunnel: Arc<Tunnel>,
    since: Instant,
}

fn running() -> &'static Mutex<HashMap<String, Running>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, Running>>> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// One tunnel's state, for the list.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TunnelState {
    pub running: bool,
    /// Seconds since the last handshake with the server.
    pub handshake_secs: Option<u64>,
    pub up_secs: Option<u64>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub connections: usize,
    pub error: Option<String>,
}

/// Brings the tunnel up, unless it already is with the same profile. Waits
/// for the server's answer, so an error here is a tunnel that cannot work.
pub async fn up(id: &str, config: &WgConfig) -> Result<(), String> {
    let mut all = running().lock().await;
    if let Some(r) = all.get(id) {
        if r.tunnel.is_running() && r.tunnel.config() == config {
            return Ok(());
        }
        all.remove(id);
    }
    let tunnel = Tunnel::start(config).await.map_err(|e| e.to_string())?;
    tunnel.wait_handshake(HANDSHAKE_TIMEOUT).await.map_err(|e| e.to_string())?;
    tracing::info!("WireGuard tunnel {id} is up");
    all.insert(id.to_string(), Running { tunnel: Arc::new(tunnel), since: Instant::now() });
    Ok(())
}

/// Turns the tunnel off; its connections end with it.
pub async fn down(id: &str) {
    if running().lock().await.remove(id).is_some() {
        tracing::info!("WireGuard tunnel {id} is off");
    }
}

pub async fn is_up(id: &str) -> bool {
    running().lock().await.get(id).is_some_and(|r| r.tunnel.is_running())
}

pub async fn state(id: &str) -> TunnelState {
    let all = running().lock().await;
    let Some(r) = all.get(id) else { return TunnelState::default() };
    let s = r.tunnel.stats();
    TunnelState {
        running: r.tunnel.is_running(),
        handshake_secs: s.last_handshake.map(|t| t.elapsed().as_secs()),
        up_secs: Some(r.since.elapsed().as_secs()),
        tx_bytes: s.tx_bytes,
        rx_bytes: s.rx_bytes,
        connections: s.connections,
        error: s.error,
    }
}

/// Ids of the tunnels that are on.
pub async fn running_ids() -> Vec<String> {
    running().lock().await.iter().filter(|(_, r)| r.tunnel.is_running()).map(|(id, _)| id.clone()).collect()
}

/// A TCP connection to `host:port` through the tunnel of profile `id`,
/// which must be up.
pub async fn dial(id: &str, host: &str, port: u16) -> io::Result<WgStream> {
    let tunnel = {
        let all = running().lock().await;
        all.get(id).filter(|r| r.tunnel.is_running()).map(|r| r.tunnel.clone())
    };
    let Some(tunnel) = tunnel else {
        return Err(io::Error::new(io::ErrorKind::NotConnected, "The WireGuard tunnel for this connection is off. Connect again to turn it on."));
    };
    tracing::info!("Connecting to {host}:{port} through WireGuard tunnel {id}");
    tunnel.connect(host, port).await
}
