//! A WireGuard tunnel that lives inside the app: boringtun speaks the
//! WireGuard protocol over one UDP socket, and smoltcp is the TCP/IP stack
//! on the tunnel's side. A connection through it is a plain byte stream to
//! the rest of the app, so SSH, Telnet and the rest use it as they would a
//! TCP socket. Nothing on the system changes: no network interface, no
//! routes, no root or VPN permission, and only the app's own connections
//! go through the tunnel.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use boringtun::noise::{Tunn, TunnResult};
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::socket::{dns, tcp};
use smoltcp::wire::{DnsQueryType, HardwareAddress, IpAddress, IpCidr};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Notify};

use super::config::WgConfig;

/// A connection through the tunnel.
pub type WgStream = DuplexStream;

/// How long a TCP connect through the tunnel may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// boringtun's timers want a tick about this often.
const TIMER_TICK: Duration = Duration::from_millis(250);
const TCP_BUFFER: usize = 256 * 1024;
const CHUNK: usize = 16 * 1024;
/// After both sides of a connection are done, how long its socket may
/// wait for the server to finish closing before it is dropped.
const LINGER: Duration = Duration::from_secs(10);

/// What the tunnel has seen, for the status shown in the app.
#[derive(Debug, Clone, Default)]
pub struct TunnelStats {
    pub last_handshake: Option<Instant>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub connections: usize,
    /// The last thing that went wrong, until a handshake works again.
    pub error: Option<String>,
}

enum Cmd {
    Connect { addr: SocketAddr, reply: oneshot::Sender<io::Result<WgStream>> },
    Resolve { name: String, v6: bool, reply: oneshot::Sender<io::Result<Vec<IpAddr>>> },
    #[cfg(test)]
    Listen { port: u16, reply: oneshot::Sender<WgStream> },
}

/// A running tunnel. Dropping it stops the tunnel and every connection in it.
pub struct Tunnel {
    cmds: mpsc::UnboundedSender<Cmd>,
    stats: Arc<Mutex<TunnelStats>>,
    has_dns: bool,
    has_v6: bool,
    has_v4: bool,
    config: WgConfig,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn smol_now(start: Instant) -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(start.elapsed().as_micros() as i64)
}

fn to_smol(ip: IpAddr) -> IpAddress {
    match ip {
        IpAddr::V4(v) => IpAddress::Ipv4(v),
        IpAddr::V6(v) => IpAddress::Ipv6(v),
    }
}

fn from_smol(ip: IpAddress) -> IpAddr {
    match ip {
        IpAddress::Ipv4(v) => IpAddr::V4(v),
        IpAddress::Ipv6(v) => IpAddr::V6(v),
    }
}

impl Tunnel {
    /// Starts the tunnel: resolves the server, opens the UDP socket and
    /// sends the first handshake. Returns at once; [`Tunnel::wait_handshake`]
    /// says whether the server answered.
    pub async fn start(config: &WgConfig) -> io::Result<Tunnel> {
        let peer_addr = match &config.peer.endpoint {
            Some(e) => {
                let (host, port) = super::config::split_endpoint(e).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                let addr = tokio::net::lookup_host((host.as_str(), port))
                    .await
                    .map_err(|e| io::Error::new(e.kind(), format!("Could not find the WireGuard server {host}: {e}")))?
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("Could not find the WireGuard server {host}")))?;
                Some(addr)
            }
            None => None,
        };
        if peer_addr.is_none() && config.listen_port.is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "The [Peer] has no Endpoint to connect to"));
        }
        let bind: SocketAddr = match peer_addr {
            Some(SocketAddr::V6(_)) => (std::net::Ipv6Addr::UNSPECIFIED, config.listen_port.unwrap_or(0)).into(),
            _ => (std::net::Ipv4Addr::UNSPECIFIED, config.listen_port.unwrap_or(0)).into(),
        };
        let udp = {
            let raw = socket2::Socket::new(socket2::Domain::for_address(bind), socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
            // Room for a whole TCP window arriving at once.
            let _ = raw.set_recv_buffer_size(4 << 20);
            let _ = raw.set_send_buffer_size(4 << 20);
            raw.set_nonblocking(true)?;
            raw.bind(&bind.into())?;
            UdpSocket::from_std(raw.into())?
        };

        let tunn = Tunn::new(
            boringtun::x25519::StaticSecret::from(config.private_key),
            boringtun::x25519::PublicKey::from(config.peer.public_key),
            config.peer.preshared_key,
            config.peer.persistent_keepalive,
            rand::random::<u32>() >> 8,
            None,
        );

        let stats = Arc::new(Mutex::new(TunnelStats::default()));
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker::new(config, tunn, udp, peer_addr, stats.clone());
        let task = tokio::spawn(worker.run(rx));
        Ok(Tunnel {
            cmds: tx,
            stats,
            has_dns: !config.dns.is_empty(),
            has_v4: config.addresses.iter().any(|a| a.addr.is_ipv4()),
            has_v6: config.addresses.iter().any(|a| a.addr.is_ipv6()),
            config: config.clone(),
            task,
        })
    }

    pub fn config(&self) -> &WgConfig {
        &self.config
    }

    pub fn stats(&self) -> TunnelStats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    /// Waits until the server has answered a handshake.
    pub async fn wait_handshake(&self, timeout: Duration) -> io::Result<()> {
        let until = Instant::now() + timeout;
        loop {
            if self.stats().last_handshake.is_some() {
                return Ok(());
            }
            if !self.is_running() {
                return Err(io::Error::other(self.stats().error.unwrap_or_else(|| "The tunnel stopped".into())));
            }
            if Instant::now() >= until {
                let why = self.stats().error.map(|e| format!(" ({e})")).unwrap_or_default();
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("The WireGuard server did not answer{why}")));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<io::Result<T>>) -> Cmd) -> io::Result<T> {
        let (reply, answer) = oneshot::channel();
        self.cmds.send(make(reply)).map_err(|_| io::Error::other("The WireGuard tunnel is off"))?;
        answer.await.map_err(|_| io::Error::other("The WireGuard tunnel stopped"))?
    }

    /// The addresses of `name`, from the tunnel's DNS servers when the
    /// profile has some, else from the system's.
    pub async fn resolve(&self, name: &str) -> io::Result<Vec<IpAddr>> {
        if let Ok(ip) = name.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        if self.has_dns {
            let mut out = Vec::new();
            let mut last = None;
            for v6 in [false, true] {
                if (v6 && !self.has_v6) || (!v6 && !self.has_v4) {
                    continue;
                }
                match self.ask(|reply| Cmd::Resolve { name: name.to_string(), v6, reply }).await {
                    Ok(list) => out.extend(list),
                    Err(e) => last = Some(e),
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
            if let Some(e) = last {
                tracing::debug!("WireGuard DNS could not find {name}: {e}; asking the system");
            }
        }
        let found: Vec<IpAddr> = tokio::net::lookup_host((name, 0)).await?.map(|a| a.ip()).collect();
        if found.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("Could not find {name}")));
        }
        Ok(found)
    }

    /// Opens a TCP connection to `host:port` through the tunnel.
    pub async fn connect(&self, host: &str, port: u16) -> io::Result<WgStream> {
        let ips = self.resolve(host).await?;
        // Addresses of a family the tunnel has no address in cannot work.
        let usable: Vec<IpAddr> = ips.iter().copied().filter(|ip| if ip.is_ipv4() { self.has_v4 } else { self.has_v6 }).collect();
        if usable.is_empty() {
            return Err(io::Error::new(io::ErrorKind::AddrNotAvailable, format!("The tunnel has no {} address for {host}", if ips[0].is_ipv4() { "IPv4" } else { "IPv6" })));
        }
        let mut last = None;
        for ip in usable {
            match self.ask(|reply| Cmd::Connect { addr: SocketAddr::new(ip, port), reply }).await {
                Ok(s) => return Ok(s),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("at least one address was tried"))
    }

    /// Test servers: the next connection made to `port` on this end.
    #[cfg(test)]
    pub async fn accept(&self, port: u16) -> io::Result<WgStream> {
        let (reply, answer) = oneshot::channel();
        self.cmds.send(Cmd::Listen { port, reply }).map_err(|_| io::Error::other("off"))?;
        answer.await.map_err(|_| io::Error::other("stopped"))
    }
}

/// smoltcp's side of the tunnel: IP packets in from WireGuard, out to it.
struct Pipe {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
    mtu: usize,
}

struct RxToken(Vec<u8>);
struct TxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0; len];
        let r = f(&mut buf);
        self.0.push_back(buf);
        r
    }
}

impl Device for Pipe {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _: smoltcp::time::Instant) -> Option<(RxToken, TxToken<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((RxToken(packet), TxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<TxToken<'_>> {
        Some(TxToken(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

/// One TCP connection, between its smoltcp socket and the stream the app holds.
struct Conn {
    handle: SocketHandle,
    /// Until the connection is made: where the app's stream goes.
    reply: Option<oneshot::Sender<io::Result<WgStream>>>,
    app: Option<WgStream>,
    deadline: Instant,
    /// What the app wrote, in chunks.
    to_net: mpsc::Receiver<Vec<u8>>,
    pending: Option<(Vec<u8>, usize)>,
    app_done: bool,
    /// What the server sent, for the app; dropped at the server's end of stream.
    from_net: Option<mpsc::Sender<Vec<u8>>>,
    linger: Option<Instant>,
    #[cfg(test)]
    accept: Option<oneshot::Sender<WgStream>>,
}

struct Query {
    handle: dns::QueryHandle,
    reply: oneshot::Sender<io::Result<Vec<IpAddr>>>,
    name: String,
}

struct Worker {
    tunn: Tunn,
    udp: UdpSocket,
    peer: Option<SocketAddr>,
    stats: Arc<Mutex<TunnelStats>>,
    iface: Interface,
    pipe: Pipe,
    sockets: SocketSet<'static>,
    dns: Option<SocketHandle>,
    conns: Vec<Conn>,
    queries: Vec<Query>,
    wake: Arc<Notify>,
    start: Instant,
    next_port: u16,
    buf: Vec<u8>,
}

impl Worker {
    fn new(config: &WgConfig, tunn: Tunn, udp: UdpSocket, peer: Option<SocketAddr>, stats: Arc<Mutex<TunnelStats>>) -> Self {
        let start = Instant::now();
        let mtu = usize::from(config.mtu.unwrap_or(1420));
        let mut pipe = Pipe { rx: VecDeque::new(), tx: VecDeque::new(), mtu };
        let mut cfg = IfaceConfig::new(HardwareAddress::Ip);
        cfg.random_seed = rand::random();
        let mut iface = Interface::new(cfg, &mut pipe, smol_now(start));
        iface.update_ip_addrs(|addrs| {
            for a in &config.addresses {
                let _ = addrs.push(IpCidr::new(to_smol(a.addr), a.prefix));
            }
        });
        // Everything not on the tunnel's own subnet goes to the peer: on a
        // point-to-point link the gateway is only a placeholder.
        for a in &config.addresses {
            match a.addr {
                IpAddr::V4(v) => {
                    let _ = iface.routes_mut().add_default_ipv4_route(v);
                }
                IpAddr::V6(v) => {
                    let _ = iface.routes_mut().add_default_ipv6_route(v);
                }
            }
        }
        let mut sockets = SocketSet::new(Vec::new());
        let dns = (!config.dns.is_empty()).then(|| {
            let servers: Vec<IpAddress> = config.dns.iter().map(|ip| to_smol(*ip)).collect();
            sockets.add(dns::Socket::new(&servers, Vec::new()))
        });
        Worker {
            tunn,
            udp,
            peer,
            stats,
            iface,
            pipe,
            sockets,
            dns,
            conns: Vec::new(),
            queries: Vec::new(),
            wake: Arc::new(Notify::new()),
            start,
            next_port: 49152 + rand::random::<u16>() % 16000,
            buf: vec![0; 65536 + 256],
        }
    }

    fn set_error(&self, e: impl Into<String>) {
        if let Ok(mut s) = self.stats.lock() {
            s.error = Some(e.into());
        }
    }

    async fn send_udp(&self, packet: &[u8]) {
        let Some(peer) = self.peer else { return };
        if let Err(e) = self.udp.send_to(packet, peer).await {
            tracing::debug!("WireGuard: sending to {peer} failed: {e}");
            self.set_error(format!("Could not reach the WireGuard server: {e}"));
        }
    }

    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        // Start the handshake now rather than with the first connection.
        if let TunnResult::WriteToNetwork(p) = self.tunn.format_handshake_initiation(&mut self.buf, false) {
            let p = p.to_vec();
            self.send_udp(&p).await;
        }
        let mut last_tick = Instant::now();
        let mut datagram = vec![0u8; 65536];
        loop {
            // Let smoltcp and the connections move until nothing changes.
            for _ in 0..64 {
                self.iface.poll(smol_now(self.start), &mut self.pipe, &mut self.sockets);
                let sent = self.flush_to_peer().await;
                let moved = self.service();
                if !sent && !moved {
                    break;
                }
            }
            self.publish();

            let delay = self
                .iface
                .poll_delay(smol_now(self.start), &self.sockets)
                .map(|d| Duration::from_micros(d.total_micros()))
                .unwrap_or(TIMER_TICK)
                .min(TIMER_TICK.saturating_sub(last_tick.elapsed()));
            let wake = self.wake.clone();
            tokio::select! {
                got = self.udp.recv_from(&mut datagram) => match got {
                    Ok((n, from)) => {
                        self.handle_datagram(&datagram[..n], from).await;
                        // Take everything that is waiting before smoltcp
                        // runs again: a burst handled one by one overflows
                        // the socket's buffer and TCP slows to its timeouts.
                        for _ in 0..512 {
                            match self.udp.try_recv_from(&mut datagram) {
                                Ok((n, from)) => self.handle_datagram(&datagram[..n], from).await,
                                Err(_) => break,
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("WireGuard: UDP receive failed: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                },
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => self.command(cmd),
                    None => break,
                },
                _ = wake.notified() => {}
                _ = tokio::time::sleep(delay) => {}
            }
            if last_tick.elapsed() >= TIMER_TICK {
                last_tick = Instant::now();
                self.timers().await;
            }
        }
        tracing::info!("WireGuard tunnel stopped");
    }

    async fn handle_datagram(&mut self, data: &[u8], from: SocketAddr) {
        match self.tunn.decapsulate(Some(from.ip()), data, &mut self.buf) {
            TunnResult::WriteToNetwork(p) => {
                // The peer moves: answer where it wrote from (roaming).
                self.peer = Some(from);
                let p = p.to_vec();
                self.send_udp(&p).await;
                // Packets queued while the handshake was going go now.
                while let TunnResult::WriteToNetwork(p) = self.tunn.decapsulate(None, &[], &mut self.buf) {
                    let p = p.to_vec();
                    self.send_udp(&p).await;
                }
            }
            TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
                self.peer = Some(from);
                self.pipe.rx.push_back(p.to_vec());
            }
            TunnResult::Done => {}
            TunnResult::Err(e) => tracing::debug!("WireGuard: packet from {from} dropped: {e:?}"),
        }
        self.publish();
    }

    async fn timers(&mut self) {
        match self.tunn.update_timers(&mut self.buf) {
            TunnResult::WriteToNetwork(p) => {
                let p = p.to_vec();
                self.send_udp(&p).await;
            }
            TunnResult::Err(boringtun::noise::errors::WireGuardError::ConnectionExpired) => {
                self.set_error("The WireGuard server stopped answering");
                if let Ok(mut s) = self.stats.lock() {
                    s.last_handshake = None;
                }
            }
            _ => {}
        }
        // Connections that never got through.
        let now = Instant::now();
        for c in &mut self.conns {
            if c.reply.is_some() && now >= c.deadline {
                let _ = c.reply.take().expect("checked").send(Err(io::Error::new(io::ErrorKind::TimedOut, "Connection through the WireGuard tunnel timed out")));
                self.sockets.get_mut::<tcp::Socket>(c.handle).abort();
            }
        }
    }

    /// Encrypts what smoltcp sent and hands it to the peer.
    async fn flush_to_peer(&mut self) -> bool {
        let mut sent = false;
        while let Some(packet) = self.pipe.tx.pop_front() {
            sent = true;
            if let TunnResult::WriteToNetwork(p) = self.tunn.encapsulate(&packet, &mut self.buf) {
                let p = p.to_vec();
                self.send_udp(&p).await;
            }
        }
        sent
    }

    fn publish(&self) {
        let (since, tx, rx, _, _) = self.tunn.stats();
        if let Ok(mut s) = self.stats.lock() {
            if let Some(since) = since {
                s.last_handshake = Instant::now().checked_sub(since);
                s.error = None;
            }
            s.tx_bytes = tx as u64;
            s.rx_bytes = rx as u64;
            s.connections = self.conns.len();
        }
    }

    fn local_port(&mut self) -> u16 {
        let port = self.next_port;
        self.next_port = if self.next_port >= 65000 { 49152 } else { self.next_port + 1 };
        port
    }

    fn new_conn(&mut self, socket: tcp::Socket<'static>) -> (Conn, WgStream) {
        let handle = self.sockets.add(socket);
        let (ours, theirs) = tokio::io::duplex(CHUNK * 4);
        let (mut read, mut write) = tokio::io::split(ours);
        let (to_net_tx, to_net) = mpsc::channel::<Vec<u8>>(8);
        let (from_net, mut from_net_rx) = mpsc::channel::<Vec<u8>>(8);
        let wake = self.wake.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; CHUNK];
            loop {
                match read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if to_net_tx.send(buf[..n].to_vec()).await.is_err() {
                            break;
                        }
                        wake.notify_one();
                    }
                }
            }
            drop(to_net_tx);
            wake.notify_one();
        });
        let wake = self.wake.clone();
        tokio::spawn(async move {
            while let Some(chunk) = from_net_rx.recv().await {
                wake.notify_one();
                if write.write_all(&chunk).await.is_err() {
                    break;
                }
            }
            let _ = write.shutdown().await;
            wake.notify_one();
        });
        let conn = Conn {
            handle,
            reply: None,
            app: None,
            deadline: Instant::now() + CONNECT_TIMEOUT,
            to_net,
            pending: None,
            app_done: false,
            from_net: Some(from_net),
            linger: None,
            #[cfg(test)]
            accept: None,
        };
        (conn, theirs)
    }

    fn tcp_socket() -> tcp::Socket<'static> {
        let mut s = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; TCP_BUFFER]), tcp::SocketBuffer::new(vec![0; TCP_BUFFER]));
        s.set_nagle_enabled(false);
        // A WireGuard path drops packets like any internet path; without
        // congestion control a full window keeps getting lost.
        s.set_congestion_control(tcp::CongestionControl::Cubic);
        s.set_keep_alive(Some(smoltcp::time::Duration::from_secs(30)));
        s
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Connect { addr, reply } => {
                let mut socket = Self::tcp_socket();
                let port = self.local_port();
                if let Err(e) = socket.connect(self.iface.context(), (to_smol(addr.ip()), addr.port()), port) {
                    let _ = reply.send(Err(io::Error::other(format!("Could not connect to {addr} through the tunnel: {e}"))));
                    return;
                }
                let (mut conn, theirs) = self.new_conn(socket);
                conn.reply = Some(reply);
                conn.app = Some(theirs);
                self.conns.push(conn);
            }
            Cmd::Resolve { name, v6, reply } => {
                let Some(handle) = self.dns else {
                    let _ = reply.send(Err(io::Error::other("The profile has no DNS")));
                    return;
                };
                let kind = if v6 { DnsQueryType::Aaaa } else { DnsQueryType::A };
                let socket = self.sockets.get_mut::<dns::Socket>(handle);
                match socket.start_query(self.iface.context(), &name, kind) {
                    Ok(q) => self.queries.push(Query { handle: q, reply, name }),
                    Err(e) => {
                        let _ = reply.send(Err(io::Error::new(io::ErrorKind::InvalidInput, format!("Could not look up {name}: {e:?}"))));
                    }
                }
            }
            #[cfg(test)]
            Cmd::Listen { port, reply } => {
                let mut socket = Self::tcp_socket();
                socket.listen(port).expect("listen");
                let (mut conn, theirs) = self.new_conn(socket);
                conn.app = Some(theirs);
                conn.accept = Some(reply);
                conn.deadline = Instant::now() + Duration::from_secs(3600);
                self.conns.push(conn);
            }
        }
    }

    /// Moves data between the sockets and the app's streams. True when
    /// something moved.
    fn service(&mut self) -> bool {
        let mut moved = false;
        let now = Instant::now();

        if let Some(dns) = self.dns {
            let socket = self.sockets.get_mut::<dns::Socket>(dns);
            self.queries.retain_mut(|q| match socket.get_query_result(q.handle) {
                Err(dns::GetQueryResultError::Pending) => true,
                Err(dns::GetQueryResultError::Failed) => {
                    // The reply is taken out by replacing it: retain_mut owns the slot.
                    let (dummy, _) = oneshot::channel();
                    let reply = std::mem::replace(&mut q.reply, dummy);
                    let _ = reply.send(Err(io::Error::new(io::ErrorKind::NotFound, format!("The tunnel's DNS does not know {}", q.name))));
                    false
                }
                Ok(list) => {
                    let (dummy, _) = oneshot::channel();
                    let reply = std::mem::replace(&mut q.reply, dummy);
                    let _ = reply.send(Ok(list.iter().map(|ip| from_smol(*ip)).collect()));
                    false
                }
            });
        }

        for c in &mut self.conns {
            let socket = self.sockets.get_mut::<tcp::Socket>(c.handle);
            let state = socket.state();

            #[cfg(test)]
            if c.accept.is_some() && socket.is_active() && state != tcp::State::Listen && state != tcp::State::SynReceived {
                if let (Some(accept), Some(app)) = (c.accept.take(), c.app.take()) {
                    let _ = accept.send(app);
                }
            }

            if c.reply.is_some() {
                match state {
                    tcp::State::Established => {
                        let reply = c.reply.take().expect("checked");
                        let app = c.app.take().expect("set with the reply");
                        let _ = reply.send(Ok(app));
                        moved = true;
                    }
                    tcp::State::Closed | tcp::State::TimeWait => {
                        let _ = c.reply.take().expect("checked").send(Err(io::Error::new(io::ErrorKind::ConnectionRefused, "The server refused the connection")));
                        c.app = None;
                        moved = true;
                    }
                    _ => {}
                }
            }

            // The app's data out.
            while socket.can_send() {
                if c.pending.is_none() {
                    match c.to_net.try_recv() {
                        Ok(v) => c.pending = Some((v, 0)),
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            c.app_done = true;
                            break;
                        }
                    }
                }
                let (data, at) = c.pending.as_mut().expect("filled above");
                match socket.send_slice(&data[*at..]) {
                    Ok(0) => break,
                    Ok(n) => {
                        moved = true;
                        *at += n;
                        if *at >= data.len() {
                            c.pending = None;
                        }
                    }
                    Err(_) => break,
                }
            }
            if !c.app_done && c.pending.is_none() && !matches!(state, tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived) {
                if let Err(mpsc::error::TryRecvError::Disconnected) = c.to_net.try_recv().map(|v| c.pending = Some((v, 0))) {
                    c.app_done = true;
                }
            }
            if c.app_done && c.pending.is_none() && socket.may_send() {
                socket.close();
                moved = true;
            }

            // The server's data in, as fast as the app takes it.
            if let Some(tx) = c.from_net.clone() {
                while socket.can_recv() {
                    match tx.try_reserve() {
                        Ok(permit) => {
                            if let Ok(chunk) = socket.recv(|b| {
                                let n = b.len().min(CHUNK);
                                (n, b[..n].to_vec())
                            }) {
                                permit.send(chunk);
                                moved = true;
                            }
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => break,
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            c.from_net = None;
                            break;
                        }
                    }
                }
                let server_done = !socket.may_recv() && !socket.can_recv() && !matches!(state, tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived);
                if server_done && c.reply.is_none() {
                    c.from_net = None;
                    moved = true;
                }
            }

            if c.app_done && c.from_net.is_none() && c.linger.is_none() {
                c.linger = Some(now + LINGER);
            }
            if c.linger.is_some_and(|l| now >= l) && socket.is_open() {
                socket.abort();
            }
        }

        // Done connections go.
        let sockets = &mut self.sockets;
        self.conns.retain(|c| {
            let state = sockets.get::<tcp::Socket>(c.handle).state();
            let finished = matches!(state, tcp::State::Closed | tcp::State::TimeWait) && c.reply.is_none() && c.from_net.is_none();
            #[cfg(test)]
            let finished = finished && c.accept.is_none();
            if finished {
                sockets.remove(c.handle);
            }
            !finished
        });
        moved
    }
}
