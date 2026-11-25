//! SPICE desktops: KVM/QEMU virtual machines, as Proxmox VE, libvirt
//! (virt-manager) and oVirt serve them.
//!
//! The protocol, the image codecs and the channel handlers are the ryll SPICE
//! client's (shakenfist-spice-renderer, Apache-2.0); what reaches the webview
//! is the RDP backend's, as for VNC. Every draw the guest sends lands on the
//! renderer's own surfaces ([`SurfaceMirror`]), and what changed on the
//! primary one is folded into the same [`Screen`] the RDP pump keeps, under
//! the same pacing ([`Flow`]). The desktop panel that draws RDP and VNC
//! draws SPICE unchanged.
//!
//! Keys go as PC/AT scancodes, as for RDP: SPICE types the key at that
//! position, so the guest's keyboard layout is the one that counts.
//!
//! The guest's clipboard and its display size go through the SPICE agent
//! (spice-vdagent in the guest); without one, neither does.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironrdp_graphics::pointer::DecodedPointer;
use serde::{Deserialize, Serialize};
use shakenfist_spice_protocol::{mouse_buttons, ConnectionConfig};
use shakenfist_spice_renderer::channels::volume::VolumeControl;
use shakenfist_spice_renderer::{
    make_scancode, ByteCounter, ChannelEvent, ChannelSnapshots, ClipboardBackend, DisplaySize, InputEvent, LogConfig,
    PlaybackOutput, PointerTranslator, SessionState, SurfaceMirror, TrafficSink,
};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::db::conn::Forward;
use crate::rdp::{Cursor, Flow, RdpStatus, Screen};

pub mod vv;

/// A saved SPICE session's own settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceOptions {
    /// The TLS port: every channel goes over TLS to it, and `port` is not
    /// used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_port: Option<u16>,
    /// The CA that signed the server's certificate, PEM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    /// The certificate subject the server must have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_subject: Option<String>,
}

impl SpiceOptions {
    /// Nothing set: stored as no options at all.
    pub fn is_empty(&self) -> bool {
        let blank = |v: &Option<String>| v.as_deref().is_none_or(|v| v.trim().is_empty());
        self.tls_port.is_none() && blank(&self.ca) && blank(&self.host_subject)
    }
}

/// What the webview sends to open a desktop.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpiceConnectParams {
    pub id: String,
    pub host: String,
    /// The plain port; with `tls_port` set, unused (every channel goes
    /// over TLS then).
    pub port: u16,
    /// The SPICE password (the "ticket"); empty for a server that asks for
    /// none.
    #[serde(default)]
    pub password: String,
    /// Use the saved session's password manager entry, fetched by
    /// `spice_connect` from the stored session (not from this value); wins
    /// over `password`.
    #[serde(default, alias = "password_ref")]
    pub password_ref: Option<crate::pwmanager::SecretRef>,
    /// A saved SSH session to reach the server through.
    #[serde(default)]
    pub via_session_id: Option<String>,
    /// The TLS port: every channel goes over TLS to it.
    #[serde(default)]
    pub tls_port: Option<u16>,
    /// The CA that signed the server's certificate, PEM. It is then the
    /// only one trusted, and the certificate's name need not match the host.
    #[serde(default)]
    pub ca: Option<String>,
    /// The certificate subject the server must have, as `C=..,O=..,CN=..`.
    #[serde(default)]
    pub host_subject: Option<String>,
    /// An HTTP proxy to go through with CONNECT, as Proxmox VE's
    /// spiceproxy. Only from a .vv file; needs `tls_port` and
    /// `host_subject`.
    #[serde(default)]
    pub proxy: Option<String>,
    /// A Proxmox VM's console: the rest is asked of the Proxmox session's
    /// open connection at each connect (its ticket lasts 30 seconds).
    #[serde(default)]
    pub proxmox: Option<crate::proxmox::ConsoleTarget>,
}

impl SpiceConnectParams {
    /// The port the server is reached on: the TLS one when there is one.
    pub fn target_port(&self) -> u16 {
        self.tls_port.unwrap_or(self.port)
    }
}

/// How long the first channel may take to connect and log in. The renderer
/// bounds each dial and handshake itself; this is the whole of it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Mouse and keyboard events that may wait for the session.
const INPUT_QUEUE: usize = 256;

/// What the channels may have waiting for the pump. A full queue holds the
/// channel back, which is how a slow canvas slows the guest's stream.
const EVENT_QUEUE: usize = 1024;

/// The decoded-image cache and the GLZ dictionary, each. ryll's default is
/// 256 MiB; a desktop in a tab, on a phone too, needs less.
const CACHE_BYTES: usize = 128 * 1024 * 1024;

/// SPICE_IMAGE_COMPRESSION_AUTO_GLZ: what spice-server does by default.
const COMPRESSION_AUTO_GLZ: u8 = 3;

pub enum Input {
    /// A PC/AT set-1 scancode; `extended` for the 0xE0-prefixed ones.
    Key { scancode: u8, extended: bool, down: bool },
    Mouse { x: u16, y: u16, action: crate::vnc::MouseAction, button: u8, delta: i16 },
    /// Text typed with no key behind it (a phone keyboard, an IME), sent as
    /// US-layout keystrokes.
    Type(String),
    Resize { width: u16, height: u16 },
    Close,
}

/// One open desktop.
struct Open {
    input: mpsc::Sender<Input>,
    task: tokio::task::JoinHandle<()>,
    flow: Arc<Flow>,
    frames: Arc<Mutex<Channel<InvokeResponseBody>>>,
    clipboard: Arc<OfferedClipboard>,
}

#[derive(Default)]
pub struct SpiceManager {
    open: HashMap<String, Open>,
}

impl SpiceManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `id` is a session still running, to which a re-created panel
    /// can re-attach rather than connect again.
    pub fn running(&self, id: &str) -> bool {
        self.open.get(id).is_some_and(|o| !o.task.is_finished())
    }

    /// Open a desktop and start streaming it to `frames`. Returns at once;
    /// the outcome arrives as `spice-status-{id}` events. `forward` is the
    /// SSH leg when there is one, kept alive for as long as the session is.
    pub fn connect(&mut self, app: AppHandle, params: SpiceConnectParams, forward: Option<Forward>, frames: Channel<InvokeResponseBody>) {
        let id = params.id.clone();

        if let Some(open) = self.open.get(&id) {
            if !open.task.is_finished() {
                tracing::info!("SPICE {id}: panel re-attached to the running session");
                *open.frames.lock().unwrap_or_else(|e| e.into_inner()) = frames;
                open.flow.reattached();
                let _ = app.emit(&format!("spice-status-{id}"), RdpStatus::Connected);
                return;
            }
        }
        if let Some(previous) = self.open.remove(&id) {
            let _ = previous.input.try_send(Input::Close);
        }

        let (input, rx) = mpsc::channel(INPUT_QUEUE);
        let flow = Arc::new(Flow::new());
        let frames = Arc::new(Mutex::new(frames));
        let clipboard = Arc::new(OfferedClipboard::new(app.clone()));
        let task = tokio::spawn(session(app, params, forward, rx, Arc::clone(&frames), Arc::clone(&flow), Arc::clone(&clipboard)));
        self.open.insert(id, Open { input, task, flow, frames, clipboard });
    }

    pub fn send(&self, id: &str, event: Input) -> Result<(), String> {
        let open = self.open.get(id).ok_or_else(|| format!("no SPICE session {id}"))?;
        open.input.try_send(event).map_err(|e| e.to_string())
    }

    pub fn ack(&self, id: &str) -> Result<(), String> {
        self.open.get(id).map(|open| open.flow.ack()).ok_or_else(|| format!("no SPICE session {id}"))
    }

    /// The desktop gained focus: offer `text` (the local clipboard) to the
    /// guest.
    pub fn offer_clipboard(&self, id: &str, text: String) -> Result<(), String> {
        let open = self.open.get(id).ok_or_else(|| format!("no SPICE session {id}"))?;
        open.clipboard.offer(text);
        Ok(())
    }

    pub fn disconnect(&mut self, id: &str) -> Result<(), String> {
        let open = self.open.remove(id).ok_or_else(|| format!("no SPICE session {id}"))?;
        if open.input.try_send(Input::Close).is_err() {
            open.task.abort();
        }
        Ok(())
    }

    pub fn disconnect_all(&mut self) {
        for (id, open) in self.open.drain() {
            tracing::info!("SPICE {id}: closing on exit");
            if open.input.try_send(Input::Close).is_err() {
                open.task.abort();
            }
        }
    }
}

/// The renderer's view of the clipboard. It polls `get_text` every half
/// second; what it gets is only what the user offered by focusing the
/// desktop (as for VNC and RDP), never the local clipboard as it changes
/// while the user works elsewhere. What the guest copies goes straight to
/// the local clipboard.
struct OfferedClipboard {
    app: AppHandle,
    offered: Mutex<Option<String>>,
    generation: AtomicU64,
}

impl OfferedClipboard {
    fn new(app: AppHandle) -> Self {
        Self { app, offered: Mutex::new(None), generation: AtomicU64::new(0) }
    }

    fn offer(&self, text: String) {
        let mut offered = self.offered.lock().unwrap_or_else(|e| e.into_inner());
        if offered.as_deref() != Some(text.as_str()) {
            *offered = Some(text);
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl ClipboardBackend for OfferedClipboard {
    fn get_text(&self) -> Option<String> {
        self.offered.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_text(&self, text: &str) -> Result<(), String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        // What the guest copied is what the local clipboard now has; offered
        // back on the next focus, it is the same text and not sent again.
        *self.offered.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_string());
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.app.clipboard().write_text(text.to_string()).map_err(|e| e.to_string())
    }

    fn generation(&self) -> Option<u64> {
        Some(self.generation.load(Ordering::Relaxed))
    }
}

/// Protocol traffic is not recorded.
struct NoTraffic(std::time::Instant);

impl TrafficSink for NoTraffic {
    fn record_sent(&self, _: &'static str, _: u16, _: &'static str, _: &[u8]) {}
    fn record_received(&self, _: &'static str, _: u16, _: &'static str, _: &[u8]) {}
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

/// The renderer's connection settings for `params`, reached at `host:port`
/// (the SSH leg's local end, when there is one).
fn connection_config(params: &SpiceConnectParams, host: String, port: u16, through_ssh: bool) -> Result<ConnectionConfig, String> {
    let proxy = match params.proxy.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(_) if through_ssh => return Err("A SPICE proxy and an SSH session cannot both be used".into()),
        Some(p) => Some(shakenfist_spice_protocol::proxy::parse_proxy_uri(p).map_err(|e| format!("SPICE proxy {p}: {e}"))?),
        None => None,
    };
    let text = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    let (port, tls_port) = match params.tls_port {
        // Through SSH the leg goes to the TLS port; it is the only one used.
        Some(_) if through_ssh => (port, Some(port)),
        Some(tls) => (port, Some(tls)),
        None => (port, None),
    };
    Ok(ConnectionConfig {
        host,
        port,
        tls_port,
        password: (!params.password.is_empty()).then(|| params.password.clone()),
        ca_cert: text(&params.ca),
        host_subject: text(&params.host_subject),
        proxy,
    })
}

/// Connect, log in, then run the pump until either side ends it.
async fn session(
    app: AppHandle,
    params: SpiceConnectParams,
    forward: Option<Forward>,
    mut input: mpsc::Receiver<Input>,
    frames: Arc<Mutex<Channel<InvokeResponseBody>>>,
    flow: Arc<Flow>,
    clipboard: Arc<OfferedClipboard>,
) {
    let id = params.id.clone();
    let status = |s: RdpStatus| {
        let _ = app.emit(&format!("spice-status-{id}"), s);
    };

    let (host, port) = match &forward {
        Some(f) => ("127.0.0.1".to_string(), f.port),
        None => (params.host.clone(), params.port),
    };
    let target = params.target_port();
    tracing::info!("SPICE {id}: connecting to {}:{target}{}", params.host, if forward.is_some() { " through SSH" } else { "" });
    let config = match connection_config(&params, host, port, forward.is_some()) {
        Ok(c) => c,
        Err(message) => {
            tracing::warn!("SPICE {id}: {message}");
            status(RdpStatus::Error { message });
            return;
        }
    };

    let (run, link) = start(config, Some(clipboard as Arc<dyn ClipboardBackend>));
    tokio::pin!(run);
    let Link { mut events, input: input_tx, resize: resize_tx, mouse_mode, cancel } = link;
    let mut pump = Pump::new(&id, mouse_mode, input_tx, resize_tx);
    let started = Instant::now();

    let reason = loop {
        let pending = pump.screen.pending();
        let settled = pump.screen.settled();
        let deadline = pump.screen.deadline().unwrap_or_else(Instant::now);
        let timeout = started + CONNECT_TIMEOUT;

        tokio::select! {
            biased;
            cmd = input.recv() => {
                let Some(cmd) = cmd else { break "closed".to_string() };
                if let Err(reason) = pump.command(cmd).await {
                    break reason;
                }
            }
            ended = &mut run => {
                // The channels may have said why before they ended.
                while let Ok(event) = events.try_recv() {
                    let _ = pump.absorb(event, &status);
                }
                break match ended {
                    Ok(()) => "the server closed the session".to_string(),
                    Err(e) => connect_error(&e, &params, target),
                };
            }
            event = events.recv() => match event {
                Some(event) => {
                    if let Err(reason) = pump.absorb(event, &status) {
                        break reason;
                    }
                }
                None => break "the server closed the session".to_string(),
            },
            _ = tokio::time::sleep_until(timeout), if !pump.connected => {
                break format!("No answer from {}:{target} within {} seconds", params.host, CONNECT_TIMEOUT.as_secs());
            }
            _ = flow.notify.notified(), if pending && !flow.can_send() => {}
            _ = tokio::time::sleep_until(deadline), if pending && !settled => {}
        }

        if flow.take_refresh() {
            pump.screen.replaced = true;
            pump.screen.touch();
        }

        if pump.screen.pending() && pump.screen.settled() && flow.can_send() {
            flow.sent();
            let sent = frames.lock().unwrap_or_else(|e| e.into_inner()).send(InvokeResponseBody::Raw(pump.screen.take()));
            if sent.is_err() {
                break "the panel is gone".to_string();
            }
        }
    };

    cancel.store(true, Ordering::SeqCst);
    tracing::info!("SPICE {id}: closed ({reason})");
    if pump.connected {
        status(RdpStatus::Closed { reason, clean: false });
    } else {
        status(RdpStatus::Error { message: reason });
    }
    drop(forward);
}

/// The renderer's side of one session, for the pump.
struct Link {
    events: mpsc::Receiver<ChannelEvent>,
    input: mpsc::Sender<InputEvent>,
    resize: mpsc::Sender<DisplaySize>,
    mouse_mode: tokio::sync::watch::Receiver<u32>,
    /// Raised to end the session's channels.
    cancel: Arc<AtomicBool>,
}

/// The renderer's session for `config`: a future that runs the channels
/// until the session ends, and the queues to and from them.
fn start(
    config: ConnectionConfig,
    clipboard: Option<Arc<dyn ClipboardBackend>>,
) -> (impl std::future::Future<Output = anyhow::Result<()>> + Send, Link) {
    let (event_tx, events) = mpsc::channel(EVENT_QUEUE);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE);
    let (resize_tx, resize_rx) = mpsc::channel(8);
    let (_usb_tx, usb_rx) = mpsc::channel(1);
    let (_webdav_tx, webdav_rx) = mpsc::channel(1);
    let session_state = SessionState::new();
    let state = session_state.subscribe();
    let cancel = Arc::new(AtomicBool::new(false));
    let run = shakenfist_spice_renderer::run_connection(
        config,
        event_tx,
        session_state,
        Arc::new(tokio::sync::Notify::new()),
        input_rx,
        usb_rx,
        webdav_rx,
        Vec::new(),
        None,
        None,
        Arc::new(ByteCounter::new()),
        Arc::new(NoTraffic(std::time::Instant::now())),
        ChannelSnapshots::default(),
        1,
        resize_rx,
        VolumeControl::new(),
        true,
        LogConfig::new(false, false),
        Arc::clone(&cancel),
        clipboard,
        // Built without audio: the playback channel is not opened.
        PlaybackOutput::Local,
        shakenfist_spice_protocol::link::H264Offer::Off,
        CACHE_BYTES,
        CACHE_BYTES,
        COMPRESSION_AUTO_GLZ,
    );
    (run, Link { events, input: input_tx, resize: resize_tx, mouse_mode: state.mouse_mode, cancel })
}

/// What to tell the user about a session that could not be opened.
fn connect_error(e: &anyhow::Error, params: &SpiceConnectParams, port: u16) -> String {
    let text = format!("{e:#}");
    let lower = text.to_ascii_lowercase();
    if lower.contains("permission denied") || lower.contains("authentication failed") || lower.contains("auth") && lower.contains("fail") {
        return if params.password.is_empty() {
            "This SPICE server asks for a password".to_string()
        } else {
            "Wrong SPICE password (a Proxmox ticket is valid for about 30 seconds)".to_string()
        };
    }
    if lower.contains("requires tls") || lower.contains("need_secured") || lower.contains("needsecured") {
        return "This SPICE server only takes TLS: set its TLS port in the session".to_string();
    }
    if lower.contains("connection refused") || lower.contains("dial") || lower.contains("timed out") || lower.contains("did not complete") {
        return format!("Could not reach {}:{port}: {text}", params.host);
    }
    text
}

struct Pump<'a> {
    id: &'a str,
    mirror: SurfaceMirror,
    screen: Screen,
    pointer: PointerTranslator,
    /// SPICE button bits held down.
    buttons: u32,
    /// The guest's cursor shape, kept to show again after a hide.
    shape: Option<Arc<DecodedPointer>>,
    connected: bool,
    mouse_mode: tokio::sync::watch::Receiver<u32>,
    input: mpsc::Sender<InputEvent>,
    resize: mpsc::Sender<DisplaySize>,
}

impl<'a> Pump<'a> {
    fn new(id: &'a str, mouse_mode: tokio::sync::watch::Receiver<u32>, input: mpsc::Sender<InputEvent>, resize: mpsc::Sender<DisplaySize>) -> Self {
        Self {
            id,
            mirror: SurfaceMirror::new(),
            screen: Screen::default(),
            pointer: PointerTranslator::new(),
            buttons: 0,
            shape: None,
            connected: false,
            mouse_mode,
            input,
            resize,
        }
    }

    async fn send(&self, event: InputEvent) -> Result<(), String> {
        self.input.send(event).await.map_err(|_| "the session ended".to_string())
    }

    async fn command(&mut self, cmd: Input) -> Result<(), String> {
        use crate::vnc::MouseAction;
        match cmd {
            Input::Close => return Err("closed".into()),
            Input::Key { scancode, extended, down } => {
                let base = if extended { 0xE000 | u32::from(scancode) } else { u32::from(scancode) };
                let code = make_scancode(base, !down);
                self.send(if down { InputEvent::KeyDown(code) } else { InputEvent::KeyUp(code) }).await?;
            }
            Input::Type(text) => {
                self.send(InputEvent::PasteText { text, char_delay_ms: 0, request_id: None, cancel: None }).await?;
            }
            Input::Mouse { x, y, action, button, delta } => {
                let mode = *self.mouse_mode.borrow();
                let (x, y) = (u32::from(x), u32::from(y));
                let bit = match button {
                    1 => mouse_buttons::MIDDLE,
                    2 => mouse_buttons::RIGHT,
                    _ => mouse_buttons::LEFT,
                };
                let events = match action {
                    MouseAction::Move => self.pointer.motion(mode, x, y).into_iter().collect(),
                    MouseAction::Down => {
                        self.buttons |= bit;
                        self.pointer.button(mode, bit, true, Some((x, y)))
                    }
                    MouseAction::Up => {
                        self.buttons &= !bit;
                        self.pointer.button(mode, bit, false, Some((x, y)))
                    }
                    MouseAction::Wheel => {
                        let mut events: Vec<_> = self.pointer.motion(mode, x, y).into_iter().collect();
                        events.extend(self.pointer.wheel(if delta > 0 { 1 } else { -1 }));
                        events
                    }
                };
                for event in events {
                    self.send(event).await?;
                }
            }
            Input::Resize { width, height } => {
                // Through the guest's agent, which takes sizes on an 8-pixel
                // grid; without an agent it is held and nothing changes.
                let align = |v: u16| u32::from(v.max(8)) & !7;
                let _ = self.resize.try_send(DisplaySize::Requested(align(width), align(height)));
            }
        }
        Ok(())
    }

    /// Fold one channel event into the screen, or act on it.
    fn absorb(&mut self, event: ChannelEvent, status: &dyn Fn(RdpStatus)) -> Result<(), String> {
        self.mirror.apply_event(&event);
        match event {
            ChannelEvent::SurfaceCreated { .. } | ChannelEvent::MonitorsConfig { .. } => {
                if !self.connected {
                    self.connected = true;
                    tracing::info!("SPICE {}: connected", self.id);
                    status(RdpStatus::Connected);
                }
            }
            ChannelEvent::CursorShape(image) => {
                if image.width > 0 && image.height > 0 && !image.pixels.is_empty() {
                    let shape = Arc::new(DecodedPointer {
                        width: image.width,
                        height: image.height,
                        hotspot_x: image.hot_spot_x,
                        hotspot_y: image.hot_spot_y,
                        bitmap_data: image.pixels,
                    });
                    self.shape = Some(Arc::clone(&shape));
                    self.screen.set_cursor(Cursor::Shape(shape));
                }
            }
            ChannelEvent::CursorPosition { visible, .. } => match (&self.shape, visible) {
                (_, false) => self.screen.set_cursor(Cursor::Hidden),
                (Some(shape), true) => self.screen.set_cursor(Cursor::Shape(Arc::clone(shape))),
                (None, true) => {}
            },
            ChannelEvent::Error { channel, message } => {
                tracing::warn!("SPICE {}: {} channel: {message}", self.id, channel.name());
                if channel == shakenfist_spice_protocol::ChannelType::Main {
                    return Err(message);
                }
            }
            ChannelEvent::Disconnected(channel, reason) => {
                tracing::info!("SPICE {}: {} channel ended: {reason:?}", self.id, channel.name());
                if channel == shakenfist_spice_protocol::ChannelType::Main {
                    return Err("the server closed the session".into());
                }
            }
            ChannelEvent::Notification(note) => tracing::info!("SPICE {}: {}", self.id, note.message),
            _ => {}
        }
        self.copy_damage();
        Ok(())
    }

    /// What changed on the guest's primary surface, onto the screen.
    fn copy_damage(&mut self) {
        let Some(surface) = self.mirror.primary_surface_mut() else { return };
        let (width, height) = surface.size();
        let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else { return };
        let damage = surface.take_damage();
        if damage.rects().is_empty() {
            return;
        }
        if self.screen.size_is(w, h) {
            let pixels = surface.pixels();
            let stride = width as usize * 4;
            for r in damage.rects() {
                let (left, top) = (r.left.min(width), r.top.min(height));
                let (right, bottom) = (r.right.min(width), r.bottom.min(height));
                if right <= left || bottom <= top {
                    continue;
                }
                let row = (right - left) as usize * 4;
                let mut rect = Vec::with_capacity(row * (bottom - top) as usize);
                for y in top..bottom {
                    let start = y as usize * stride + left as usize * 4;
                    rect.extend_from_slice(&pixels[start..start + row]);
                }
                self.screen.blit_rgba(left as u16, top as u16, (right - left) as u16, (bottom - top) as u16, &rect);
            }
        } else {
            // A new size: the whole surface, once.
            self.screen.resize(w, h);
            self.screen.blit_rgba(0, 0, w, h, surface.pixels());
        }
    }
}

#[cfg(test)]
#[path = "spice_tests.rs"]
mod tests;
