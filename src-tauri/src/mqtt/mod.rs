//! MQTT sessions: a broker explorer. A session connects to a broker over
//! TCP, TLS, WebSocket or secure WebSocket (with a client certificate when
//! the broker asks for one), subscribes to its topic filters (`#` unless
//! told otherwise) and hands what arrives to the window in batches, which
//! builds the topic tree. The window can publish, subscribe and
//! unsubscribe while it is open.
//!
//! The protocol is MQTT 3.1.1, which every broker speaks, in a small client
//! of its own (`codec`), so the connection runs over our own transports: a
//! WireGuard tunnel, our certificate pinning, our WebSocket client.

pub mod codec;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::ftp::client::Stream;
use codec::{Connect, Incoming, Publish};

/// How long reaching the broker, TLS and its CONNACK may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a publish, subscribe or unsubscribe waits for its answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// How often what arrived is handed to the window.
const BATCH_EVERY: Duration = Duration::from_millis(150);
/// A batch is handed over early at this many messages.
const BATCH_MAX: usize = 500;
/// The most of one payload the window gets; the size says how much more
/// there was.
pub const PAYLOAD_SHOWN: usize = 256 * 1024;

/// An MQTT session's own settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MqttOptions {
    /// `tcp` (unset), `tls`, `ws` or `wss`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// The WebSocket path; `/mqtt` unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws_path: Option<String>,
    /// The client ID; a new random one each connect unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Seconds; 60 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u16>,
    /// The topic filters subscribed to on connect; `#` when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<String>,
    /// A client certificate (PEM, its chain after it) and its key (PEM), for
    /// brokers that ask for one over TLS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// SHA-256 fingerprints of broker certificates this session trusts
    /// although the system does not. Changed only by answering the prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certs: Vec<String>,
}

impl MqttOptions {
    pub fn is_empty(&self) -> bool {
        *self == MqttOptions::default()
    }

    /// The filters to subscribe to on connect.
    pub fn filters(&self) -> Vec<String> {
        let list: Vec<String> = self.subscriptions.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if list.is_empty() {
            vec!["#".into()]
        } else {
            list
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Tls,
    Ws,
    Wss,
}

impl Transport {
    pub fn parse(s: Option<&str>) -> Transport {
        match s.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("tls") | Some("mqtts") | Some("ssl") => Transport::Tls,
            Some("ws") => Transport::Ws,
            Some("wss") => Transport::Wss,
            _ => Transport::Tcp,
        }
    }

    fn secure(self) -> bool {
        matches!(self, Transport::Tls | Transport::Wss)
    }
}

/// What a connect needs.
pub struct Params {
    pub host: String,
    pub port: u16,
    pub transport: Transport,
    pub ws_path: String,
    pub client_id: String,
    pub keep_alive: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Client certificate and key, PEM.
    pub client_auth: Option<(String, String)>,
    pub trusted: Vec<String>,
    /// A WireGuard profile whose tunnel carries the connection.
    pub wireguard: Option<String>,
    pub filters: Vec<String>,
}

#[derive(Debug)]
pub enum MqttError {
    /// The broker's certificate is trusted by neither the system nor the
    /// session: its fingerprint.
    UntrustedCert(String),
    /// The CONNACK refused the login (bad user name or password, or not
    /// authorised).
    LoginRefused(String),
    Other(String),
}

impl std::fmt::Display for MqttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MqttError::UntrustedCert(fp) => write!(f, "The broker's certificate is not trusted ({fp})"),
            MqttError::LoginRefused(m) | MqttError::Other(m) => f.write_str(m),
        }
    }
}

/// One message, as the window gets it.
#[derive(Debug, Clone, Serialize)]
pub struct Received {
    pub topic: String,
    /// Base64, at most [`PAYLOAD_SHOWN`] bytes of it.
    pub payload: String,
    /// The whole payload's size.
    pub size: usize,
    pub qos: u8,
    pub retain: bool,
    /// Milliseconds since 1970.
    pub time: i64,
}

/// What the connection tells the window.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Messages { messages: Vec<Received> },
    /// The connection ended; `error` unless the window closed it.
    Closed { error: Option<String> },
}

type Emit = Arc<dyn Fn(Event) + Send + Sync>;

enum Command {
    Publish { publish: Publish, reply: oneshot::Sender<Result<(), String>> },
    Subscribe { filters: Vec<(String, u8)>, reply: oneshot::Sender<Result<Vec<u8>, String>> },
    Unsubscribe { filters: Vec<String>, reply: oneshot::Sender<Result<(), String>> },
    Close,
}

/// An open MQTT session.
pub struct MqttConn {
    commands: mpsc::Sender<Command>,
}

impl MqttConn {
    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Command) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.commands.send(make(tx)).await.map_err(|_| "The MQTT session is closed; reconnect it".to_string())?;
        match tokio::time::timeout(ANSWER_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("The MQTT session is closed; reconnect it".into()),
            Err(_) => Err("The broker did not answer in time".into()),
        }
    }

    pub async fn publish(&self, topic: String, payload: Vec<u8>, qos: u8, retain: bool) -> Result<(), String> {
        if !codec::valid_topic(&topic) {
            return Err("A topic to publish to has no + or #".into());
        }
        if qos > 2 {
            return Err("QoS is 0, 1 or 2".into());
        }
        let publish = Publish { topic, payload, qos, retain, dup: false, pkid: 0 };
        self.ask(|reply| Command::Publish { publish, reply }).await
    }

    /// Subscribe; the broker's granted QoS per filter (0x80: refused).
    pub async fn subscribe(&self, filters: Vec<(String, u8)>) -> Result<Vec<u8>, String> {
        if filters.is_empty() {
            return Ok(Vec::new());
        }
        if let Some((bad, _)) = filters.iter().find(|(f, _)| !codec::valid_filter(f)) {
            return Err(format!("Not a valid topic filter: {bad}"));
        }
        self.ask(|reply| Command::Subscribe { filters, reply }).await
    }

    pub async fn unsubscribe(&self, filters: Vec<String>) -> Result<(), String> {
        if filters.is_empty() {
            return Ok(());
        }
        self.ask(|reply| Command::Unsubscribe { filters, reply }).await
    }

    pub async fn close(&self) {
        let _ = self.commands.send(Command::Close).await;
    }
}

/// The open MQTT sessions, by connection id.
#[derive(Default)]
pub struct MqttManager {
    conns: std::sync::Mutex<HashMap<String, Arc<MqttConn>>>,
}

impl MqttManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<MqttConn>> {
        self.conns.lock().ok()?.get(id).cloned()
    }

    pub fn insert(&self, id: String, conn: MqttConn) {
        if let Ok(mut m) = self.conns.lock() {
            m.insert(id, Arc::new(conn));
        }
    }

    pub fn remove(&self, id: &str) -> Option<Arc<MqttConn>> {
        self.conns.lock().ok()?.remove(id)
    }
}

enum Rx {
    Raw(ReadHalf<Box<dyn Stream>>),
    Ws(SplitStream<WebSocketStream<Box<dyn Stream>>>),
}

enum Tx {
    Raw(WriteHalf<Box<dyn Stream>>),
    Ws(SplitSink<WebSocketStream<Box<dyn Stream>>, Message>),
}

impl Rx {
    /// More bytes onto `buf`; false once the connection closed. Safe to
    /// cancel: nothing is taken until it returns.
    async fn read(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
        match self {
            Rx::Raw(r) => {
                let mut chunk = [0u8; 16 * 1024];
                let n = r.read(&mut chunk).await.map_err(|e| format!("The connection failed: {e}"))?;
                buf.extend_from_slice(&chunk[..n]);
                Ok(n > 0)
            }
            Rx::Ws(r) => loop {
                match r.next().await {
                    Some(Ok(Message::Binary(b))) => {
                        buf.extend_from_slice(&b);
                        return Ok(true);
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(false),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(format!("The connection failed: {e}")),
                }
            },
        }
    }
}

impl Tx {
    async fn send(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let r = match self {
            Tx::Raw(w) => async {
                w.write_all(&bytes).await?;
                w.flush().await
            }
            .await
            .map_err(|e| e.to_string()),
            Tx::Ws(w) => w.send(Message::binary(bytes)).await.map_err(|e| e.to_string()),
        };
        r.map_err(|e| format!("The connection failed: {e}"))
    }

    async fn close(&mut self) {
        let _ = match self {
            Tx::Raw(w) => w.shutdown().await.map_err(|e| e.to_string()),
            Tx::Ws(w) => w.close().await.map_err(|e| e.to_string()),
        };
    }
}

/// A key and certificate chain from PEM text.
fn client_auth(cert: &str, key: &str) -> Result<crate::ftp::tls::ClientAuth, MqttError> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|e| MqttError::Other(format!("The client certificate is not PEM: {e}")))?;
    if chain.is_empty() {
        return Err(MqttError::Other("The client certificate field holds no certificate".into()));
    }
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|e| MqttError::Other(format!("The client key is not a PEM private key: {e}")))?;
    Ok((chain, key))
}

/// Reach the broker: TCP or a WireGuard tunnel, TLS, then a WebSocket.
async fn open_link(p: &Params) -> Result<(Rx, Tx), MqttError> {
    let host = p.host.trim_matches(['[', ']']).to_string();
    let tcp: Box<dyn Stream> = match p.wireguard.as_deref().filter(|w| !w.is_empty()) {
        Some(wg) => Box::new(crate::wireguard::dial(wg, &host, p.port).await.map_err(|e| MqttError::Other(format!("{}:{}: {e}", p.host, p.port)))?),
        None => {
            let tcp = tokio::net::TcpStream::connect((host.as_str(), p.port)).await.map_err(|e| MqttError::Other(format!("{}:{}: {e}", p.host, p.port)))?;
            let _ = tcp.set_nodelay(true);
            Box::new(tcp)
        }
    };
    let stream: Box<dyn Stream> = if p.transport.secure() {
        let auth = match &p.client_auth {
            Some((cert, key)) => Some(client_auth(cert, key)?),
            None => None,
        };
        let (config, refused) = crate::ftp::tls::pinning_config_with(&p.trusted, auth).map_err(MqttError::Other)?;
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| MqttError::Other(format!("{host}: not a host name TLS can check ({e})")))?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| {
            match refused.lock().ok().and_then(|mut r| r.take()) {
                Some(fp) => MqttError::UntrustedCert(fp),
                None => MqttError::Other(format!("TLS failed: {e}")),
            }
        })?;
        Box::new(tls)
    } else {
        tcp
    };
    match p.transport {
        Transport::Tcp | Transport::Tls => {
            let (r, w) = tokio::io::split(stream);
            Ok((Rx::Raw(r), Tx::Raw(w)))
        }
        Transport::Ws | Transport::Wss => {
            let scheme = if p.transport == Transport::Wss { "wss" } else { "ws" };
            let authority = if host.contains(':') { format!("[{host}]") } else { host.clone() };
            let path = if p.ws_path.starts_with('/') { p.ws_path.clone() } else { format!("/{}", p.ws_path) };
            let url = format!("{scheme}://{authority}:{}{path}", p.port);
            let mut req = url.as_str().into_client_request().map_err(|e| MqttError::Other(format!("{url}: {e}")))?;
            req.headers_mut().insert("Sec-WebSocket-Protocol", "mqtt".parse().expect("static header"));
            let (ws, _) = tokio_tungstenite::client_async(req, stream).await.map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(r) => MqttError::Other(format!("The WebSocket was refused ({})", r.status())),
                other => MqttError::Other(format!("WebSocket: {other}")),
            })?;
            let (w, r) = ws.split();
            Ok((Rx::Ws(r), Tx::Ws(w)))
        }
    }
}

/// Connect and log in. On success the connection runs on its own, handing
/// what arrives to `emit`, until closed.
pub async fn connect(p: Params, emit: Emit) -> Result<MqttConn, MqttError> {
    let (mut rx, mut tx, mut buf) = tokio::time::timeout(CONNECT_TIMEOUT, async {
        let (mut rx, mut tx) = open_link(&p).await?;
        let hello = codec::connect(&Connect {
            client_id: p.client_id.clone(),
            keep_alive: p.keep_alive,
            clean_session: true,
            username: p.username.clone().filter(|u| !u.is_empty()),
            password: p.password.clone().filter(|_| p.username.as_deref().is_some_and(|u| !u.is_empty())),
        })
        .map_err(MqttError::Other)?;
        tx.send(hello).await.map_err(MqttError::Other)?;
        let mut buf = Vec::new();
        loop {
            match codec::decode(&mut buf).map_err(|e| MqttError::Other(e.to_string()))? {
                Some(Incoming::ConnAck { code: 0, .. }) => return Ok((rx, tx, buf)),
                Some(Incoming::ConnAck { code, .. }) => {
                    let why = format!("The broker refused the connection: {}", codec::connack_reason(code));
                    return Err(if code == 4 || code == 5 { MqttError::LoginRefused(why) } else { MqttError::Other(why) });
                }
                Some(_) => return Err(MqttError::Other("The broker did not answer the login".into())),
                None => {
                    if !rx.read(&mut buf).await.map_err(MqttError::Other)? {
                        return Err(MqttError::Other("The broker closed the connection (is this an MQTT port, and the right transport?)".into()));
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| MqttError::Other(format!("{}:{} did not answer in time", p.host, p.port)))??;

    let (commands, inbox) = mpsc::channel(64);
    let keep_alive = p.keep_alive;
    let filters: Vec<(String, u8)> = p.filters.iter().map(|f| (f.clone(), 1)).collect();
    tokio::spawn(async move {
        let mut s = Run::new(keep_alive);
        let error = match s.subscribe_first(&mut tx, filters).await {
            Ok(()) => s.run(&mut rx, &mut tx, &mut buf, inbox, &emit).await.err(),
            Err(e) => Some(e),
        };
        s.flush(&emit);
        tx.close().await;
        emit(Event::Closed { error });
    });
    Ok(MqttConn { commands })
}

/// A connection's state while it runs.
struct Run {
    keep_alive: Duration,
    next_pkid: u16,
    last_sent: Instant,
    ping_sent: Option<Instant>,
    batch: Vec<Received>,
    publishes: HashMap<u16, oneshot::Sender<Result<(), String>>>,
    subscribes: HashMap<u16, oneshot::Sender<Result<Vec<u8>, String>>>,
    unsubscribes: HashMap<u16, oneshot::Sender<Result<(), String>>>,
}

impl Run {
    fn new(keep_alive: u16) -> Self {
        Run {
            keep_alive: Duration::from_secs(keep_alive.max(5) as u64),
            next_pkid: 0,
            last_sent: Instant::now(),
            ping_sent: None,
            batch: Vec::new(),
            publishes: HashMap::new(),
            subscribes: HashMap::new(),
            unsubscribes: HashMap::new(),
        }
    }

    fn pkid(&mut self) -> u16 {
        self.next_pkid = self.next_pkid.wrapping_add(1).max(1);
        self.next_pkid
    }

    async fn send(&mut self, tx: &mut Tx, bytes: Vec<u8>) -> Result<(), String> {
        self.last_sent = Instant::now();
        tx.send(bytes).await
    }

    async fn subscribe_first(&mut self, tx: &mut Tx, filters: Vec<(String, u8)>) -> Result<(), String> {
        let filters: Vec<(String, u8)> = filters.into_iter().filter(|(f, _)| codec::valid_filter(f)).collect();
        if filters.is_empty() {
            return Ok(());
        }
        let id = self.pkid();
        let bytes = codec::subscribe(id, &filters)?;
        self.send(tx, bytes).await
    }

    fn flush(&mut self, emit: &Emit) {
        if !self.batch.is_empty() {
            emit(Event::Messages { messages: std::mem::take(&mut self.batch) });
        }
    }

    async fn run(&mut self, rx: &mut Rx, tx: &mut Tx, buf: &mut Vec<u8>, mut inbox: mpsc::Receiver<Command>, emit: &Emit) -> Result<(), String> {
        let mut batch_tick = tokio::time::interval(BATCH_EVERY);
        let mut ping_tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            // What is already read first.
            while let Some(packet) = codec::decode(buf).map_err(|e| e.to_string())? {
                self.incoming(tx, packet).await?;
                if self.batch.len() >= BATCH_MAX {
                    self.flush(emit);
                }
            }
            tokio::select! {
                more = rx.read(buf) => {
                    if !more? {
                        return Err("The broker closed the connection".into());
                    }
                }
                cmd = inbox.recv() => match cmd {
                    None | Some(Command::Close) => {
                        let _ = self.send(tx, codec::disconnect()).await;
                        return Ok(());
                    }
                    Some(cmd) => self.command(tx, cmd).await?,
                },
                _ = batch_tick.tick() => self.flush(emit),
                _ = ping_tick.tick() => {
                    if let Some(at) = self.ping_sent {
                        if at.elapsed() > self.keep_alive {
                            return Err("The broker stopped answering".into());
                        }
                    } else if self.last_sent.elapsed() >= self.keep_alive / 2 {
                        self.ping_sent = Some(Instant::now());
                        self.send(tx, codec::pingreq()).await?;
                    }
                }
            }
        }
    }

    async fn command(&mut self, tx: &mut Tx, cmd: Command) -> Result<(), String> {
        match cmd {
            Command::Publish { mut publish, reply } => {
                if publish.qos > 0 {
                    publish.pkid = self.pkid();
                }
                let bytes = match codec::publish(&publish) {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        return Ok(());
                    }
                };
                self.send(tx, bytes).await?;
                if publish.qos == 0 {
                    let _ = reply.send(Ok(()));
                } else {
                    self.publishes.insert(publish.pkid, reply);
                }
            }
            Command::Subscribe { filters, reply } => {
                let id = self.pkid();
                match codec::subscribe(id, &filters) {
                    Ok(bytes) => {
                        self.send(tx, bytes).await?;
                        self.subscribes.insert(id, reply);
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Command::Unsubscribe { filters, reply } => {
                let id = self.pkid();
                match codec::unsubscribe(id, &filters) {
                    Ok(bytes) => {
                        self.send(tx, bytes).await?;
                        self.unsubscribes.insert(id, reply);
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Command::Close => {}
        }
        Ok(())
    }

    async fn incoming(&mut self, tx: &mut Tx, packet: Incoming) -> Result<(), String> {
        match packet {
            Incoming::Publish(p) => {
                match p.qos {
                    1 => self.send(tx, codec::ack(codec::PUBACK, p.pkid)).await?,
                    2 => self.send(tx, codec::ack(codec::PUBREC, p.pkid)).await?,
                    _ => {}
                }
                let shown = &p.payload[..p.payload.len().min(PAYLOAD_SHOWN)];
                self.batch.push(Received {
                    payload: base64::engine::general_purpose::STANDARD.encode(shown),
                    size: p.payload.len(),
                    topic: p.topic,
                    qos: p.qos,
                    retain: p.retain,
                    time: chrono::Utc::now().timestamp_millis(),
                });
            }
            Incoming::PubRel(id) => self.send(tx, codec::ack(codec::PUBCOMP, id)).await?,
            Incoming::PubAck(id) | Incoming::PubComp(id) => {
                if let Some(r) = self.publishes.remove(&id) {
                    let _ = r.send(Ok(()));
                }
            }
            Incoming::PubRec(id) => self.send(tx, codec::ack(codec::PUBREL, id)).await?,
            Incoming::SubAck { pkid, codes } => {
                if let Some(r) = self.subscribes.remove(&pkid) {
                    let _ = r.send(Ok(codes));
                }
            }
            Incoming::UnsubAck(id) => {
                if let Some(r) = self.unsubscribes.remove(&id) {
                    let _ = r.send(Ok(()));
                }
            }
            Incoming::PingResp => self.ping_sent = None,
            Incoming::ConnAck { .. } => return Err("The broker sent a second CONNACK".into()),
        }
        Ok(())
    }
}

/// A client ID for a connect: `reachercm-` and eight random hex digits.
pub fn random_client_id() -> String {
    format!("reachercm-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

#[cfg(test)]
#[path = "mock_tests.rs"]
mod mock_tests;
