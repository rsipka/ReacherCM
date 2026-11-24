//! Telnet sessions: a TCP connection driven by [`protocol::TelnetCodec`], with
//! output streamed to the terminal as `telnet-data-{id}` events and the end of
//! the connection as `telnet-exit-{id}`.
//!
//! The terminal connects only once it is listening, so nothing the device
//! sends first (a banner, a login prompt) is lost.

pub mod protocol;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A connection to the device: a TCP socket, or one through a WireGuard tunnel.
pub trait Link: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Link for T {}
pub type Stream = Box<dyn Link>;
use tokio::sync::mpsc;

use protocol::{AutoLogin, LoginStep, TelnetCodec, TelnetOptions};

/// How long to wait for the device to answer the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long after connecting a login prompt is answered from the saved login.
const LOGIN_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetConnectParams {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub options: TelnetOptions,
    /// Typed at the device's login prompt, if it shows one.
    #[serde(default)]
    pub username: Option<String>,
    /// Typed at the device's password prompt.
    #[serde(default)]
    pub password: Option<String>,
    /// A password manager entry used instead of `password`.
    #[serde(default)]
    pub password_ref: Option<crate::pwmanager::SecretRef>,
    /// A WireGuard profile whose tunnel (already up) carries the connection.
    #[serde(default)]
    pub wireguard: Option<String>,
}

enum Command {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct Connection {
    commands: mpsc::UnboundedSender<Command>,
    /// Set when a newer connection took this one's id: its events now
    /// belong to that one, so this one ends without reporting anything.
    replaced: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct TelnetManager {
    connections: HashMap<String, Connection>,
}

impl TelnetManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a connection made by [`open`] until either side closes it.
    pub fn start(&mut self, app: tauri::AppHandle, params: TelnetConnectParams, stream: Stream) {
        let (tx, rx) = mpsc::unbounded_channel();
        // A connection made again under the same id replaces the old one.
        let replaced = Arc::new(AtomicBool::new(false));
        let connection = Connection { commands: tx, replaced: replaced.clone() };
        if let Some(old) = self.connections.insert(params.id.clone(), connection) {
            old.replaced.store(true, Ordering::SeqCst);
            let _ = old.commands.send(Command::Close);
        }
        tauri::async_runtime::spawn(run(app, params, stream, rx, replaced));
    }

    pub fn send(&self, id: &str, data: Vec<u8>) -> Result<(), String> {
        self.command(id, Command::Input(data))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        self.command(id, Command::Resize(cols, rows))
    }

    pub fn close(&mut self, id: &str) {
        if let Some(connection) = self.connections.remove(id) {
            let _ = connection.commands.send(Command::Close);
        }
    }

    fn command(&self, id: &str, cmd: Command) -> Result<(), String> {
        self.connections
            .get(id)
            .ok_or_else(|| format!("Telnet connection not found: {id}"))?
            .commands
            .send(cmd)
            .map_err(|_| "The Telnet connection is closed".to_string())
    }
}

/// Connect to the device.
pub async fn open(host: &str, port: u16, wireguard: Option<&str>) -> Result<Stream, String> {
    if let Some(wg) = wireguard {
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, crate::wireguard::dial(wg, host, port))
            .await
            .map_err(|_| format!("{host}:{port} did not answer through WireGuard within {} seconds", CONNECT_TIMEOUT.as_secs()))?
            .map_err(|e| format!("Could not connect to {host}:{port} through WireGuard: {e}"))?;
        return Ok(Box::new(stream));
    }
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| {
            format!(
                "{host}:{port} did not answer within {} seconds",
                CONNECT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("Could not connect to {host}:{port}: {e}"))?;
    // Keystrokes go out as they are typed.
    let _ = stream.set_nodelay(true);
    Ok(Box::new(stream))
}

async fn run(
    app: tauri::AppHandle,
    params: TelnetConnectParams,
    stream: Stream,
    mut commands: mpsc::UnboundedReceiver<Command>,
    replaced: Arc<AtomicBool>,
) {
    let id = params.id.clone();
    let data_event = format!("telnet-data-{id}");
    let exit_event = format!("telnet-exit-{id}");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut codec = TelnetCodec::new(params.options, params.cols, params.rows);
    let mut login = AutoLogin::new(params.username.clone(), params.password.clone());
    let login_until = tokio::time::Instant::now() + LOGIN_WINDOW;
    let mut decoder = crate::text::Utf8Stream::new();
    let mut buf = vec![0u8; 8192];
    // rz/sz on the device: a ZMODEM transfer over this connection.
    let mut zmodem = crate::zmodem::Tap::for_app(&app, &id, crate::zmodem::WINDOW_NETWORK);
    let mut zmodem_tick = tokio::time::interval(Duration::from_millis(100));
    zmodem_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let start = codec.start();
    let mut error: Option<String> = None;
    if !start.is_empty() {
        if let Err(e) = writer.write_all(&start).await {
            error = Some(e.to_string());
        }
    }

    while error.is_none() {
        tokio::select! {
            read = reader.read(&mut buf) => {
                let n = match read {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        error = Some(e.to_string());
                        break;
                    }
                };
                let received = codec.receive(&buf[..n]);
                let mut reply = received.reply;
                if login.active() {
                    if tokio::time::Instant::now() > login_until {
                        login.stop();
                    } else if let Some(step) = login.feed(&received.data) {
                        let mut typed = match step {
                            LoginStep::Username(u) => u.into_bytes(),
                            LoginStep::Password(p) => p.into_bytes(),
                        };
                        typed.push(b'\r');
                        reply.extend_from_slice(&codec.encode(&typed));
                    }
                }
                let z = zmodem.input(&received.data);
                if !z.wire.is_empty() {
                    reply.extend_from_slice(&codec.encode_raw(&z.wire));
                }
                if !reply.is_empty() {
                    if let Err(e) = writer.write_all(&reply).await {
                        error = Some(e.to_string());
                        break;
                    }
                }
                let text = decoder.push(&z.display);
                if !text.is_empty() {
                    crate::history::output(&id, &text);
                }
                if !text.is_empty() && app.emit(&data_event, text).is_err() {
                    break;
                }
            }
            _ = zmodem_tick.tick(), if zmodem.busy() => {
                let z = zmodem.poll();
                if !z.wire.is_empty() {
                    if let Err(e) = writer.write_all(&codec.encode_raw(&z.wire)).await {
                        error = Some(e.to_string());
                    }
                }
            }
            cmd = commands.recv() => {
                let bytes = match cmd {
                    Some(Command::Input(data)) => codec.encode(&data),
                    Some(Command::Resize(cols, rows)) => codec.resize(cols, rows),
                    Some(Command::Close) | None => break,
                };
                if !bytes.is_empty() {
                    if let Err(e) = writer.write_all(&bytes).await {
                        error = Some(e.to_string());
                    }
                }
            }
        }
    }

    let replaced = replaced.load(Ordering::SeqCst);
    if !replaced {
        let held = zmodem.flush();
        if !held.is_empty() {
            let _ = app.emit(&data_event, decoder.push(&held));
        }
        if let Some(tail) = decoder.flush() {
            let _ = app.emit(&data_event, tail);
        }
    }
    if !replaced {
        crate::history::stop(&id);
    }
    if let Some(e) = &error {
        tracing::info!("Telnet {id}: connection ended: {e}");
    }
    let _ = writer.shutdown().await;
    // A connection made again under this id listens for the same event
    // name: the end of the old one is not its end.
    if !replaced {
        let _ = app.emit(&exit_event, serde_json::json!({ "error": error }));
    }
}
