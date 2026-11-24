//! The WireGuard tunnels that are on, by profile id. A tunnel is brought up
//! on purpose (the user confirmed it, or pressed Connect in the list) and
//! stays up until turned off or the app quits; connections only use it.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::config::WgConfig;
use super::stack::{Tunnel, WgStream};

/// How long the server may take to answer the first handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

struct Running {
    tunnel: Arc<Tunnel>,
    since: Instant,
    /// Loopback listeners relaying to one address through the tunnel, for
    /// clients that open their own TCP connection (RDP, VNC, WinBox).
    forwards: HashMap<(String, u16), Listener>,
    /// A loopback SOCKS5 proxy through the tunnel, for web sessions.
    socks: Option<Listener>,
}

impl Running {
    fn new(tunnel: Tunnel) -> Self {
        Self { tunnel: Arc::new(tunnel), since: Instant::now(), forwards: HashMap::new(), socks: None }
    }
}

/// A loopback listener's port and accept loop, stopped when dropped (the
/// tunnel went down).
struct Listener {
    port: u16,
    task: JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
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
    all.insert(id.to_string(), Running::new(tunnel));
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
    let Some(tunnel) = tunnel else { return Err(off(id)) };
    tracing::info!("Connecting to {host}:{port} through WireGuard tunnel {id}");
    tunnel.connect(host, port).await
}

fn off(_id: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "The WireGuard tunnel for this connection is off. Connect again to turn it on.")
}

/// A port on 127.0.0.1 that relays every connection to `host:port` through
/// the tunnel of profile `id`, which must be up. The same address gets the
/// same port while the tunnel stays up; turning the tunnel off closes it.
pub async fn forward(id: &str, host: &str, port: u16) -> io::Result<u16> {
    let mut all = running().lock().await;
    let r = all.get_mut(id).filter(|r| r.tunnel.is_running()).ok_or_else(|| off(id))?;
    let key = (host.to_ascii_lowercase(), port);
    if let Some(l) = r.forwards.get(&key) {
        if !l.task.is_finished() {
            return Ok(l.port);
        }
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let local = listener.local_addr()?.port();
    let tunnel = Arc::downgrade(&r.tunnel);
    let target = host.to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { break };
            let Some(tunnel) = tunnel.upgrade() else { break };
            let target = target.clone();
            tokio::spawn(async move {
                // Only the tunnel's own entry keeps it alive, so turning it
                // off ends this relay too.
                let dialed = tunnel.connect(&target, port).await;
                drop(tunnel);
                match dialed {
                    Ok(mut remote) => {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut remote).await;
                    }
                    Err(e) => tracing::warn!("WireGuard forward to {target}:{port} failed: {e}"),
                }
            });
        }
    });
    tracing::info!("Forwarding 127.0.0.1:{local} to {host}:{port} through WireGuard tunnel {id}");
    r.forwards.insert(key, Listener { port: local, task });
    Ok(local)
}

/// A SOCKS5 proxy on 127.0.0.1 (no login, CONNECT only) whose connections go
/// through the tunnel of profile `id`, which must be up. Names are looked up
/// through the tunnel too.
pub async fn socks(id: &str) -> io::Result<u16> {
    let mut all = running().lock().await;
    let r = all.get_mut(id).filter(|r| r.tunnel.is_running()).ok_or_else(|| off(id))?;
    if let Some(l) = &r.socks {
        if !l.task.is_finished() {
            return Ok(l.port);
        }
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let local = listener.local_addr()?.port();
    let tunnel = Arc::downgrade(&r.tunnel);
    let task = tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else { break };
            let Some(tunnel) = tunnel.upgrade() else { break };
            tokio::spawn(async move {
                if let Err(e) = socks_client(client, tunnel).await {
                    tracing::debug!("WireGuard SOCKS connection ended: {e}");
                }
            });
        }
    });
    tracing::info!("SOCKS proxy on 127.0.0.1:{local} through WireGuard tunnel {id}");
    r.socks = Some(Listener { port: local, task });
    Ok(local)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// One SOCKS5 client (RFC 1928): no-auth greeting, then a CONNECT request.
async fn socks_client(mut c: TcpStream, tunnel: Arc<Tunnel>) -> io::Result<()> {
    let mut head = [0u8; 2];
    c.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    c.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        c.write_all(&[5, 0xff]).await?;
        return Err(bad("no acceptable auth method"));
    }
    c.write_all(&[5, 0]).await?;

    let mut req = [0u8; 4];
    c.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            c.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut n = [0u8; 1];
            c.read_exact(&mut n).await?;
            let mut name = vec![0u8; n[0] as usize];
            c.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| bad("bad host name"))?
        }
        4 => {
            let mut a = [0u8; 16];
            c.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => {
            c.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(bad("address type not supported"));
        }
    };
    let mut p = [0u8; 2];
    c.read_exact(&mut p).await?;
    let port = u16::from_be_bytes(p);
    if req[1] != 1 {
        c.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(bad("only CONNECT is supported"));
    }
    let dialed = tunnel.connect(&host, port).await;
    drop(tunnel);
    match dialed {
        Ok(mut remote) => {
            c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            tokio::io::copy_bidirectional(&mut c, &mut remote).await?;
            Ok(())
        }
        Err(e) => {
            let code = match e.kind() {
                io::ErrorKind::ConnectionRefused => 5,
                io::ErrorKind::TimedOut => 4,
                _ => 1,
            };
            c.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            Err(e)
        }
    }
}
