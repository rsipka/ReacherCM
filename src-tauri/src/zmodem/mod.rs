//! ZMODEM (rz/sz) file transfer inside terminal sessions: SSH, Telnet and
//! serial. The point is gear with no SFTP: a serial console, a switch over
//! Telnet, a small host with nothing but a shell.
//!
//! Each session's read loop owns a [`Tap`] and passes everything the remote
//! sends through it. While no transfer runs the bytes come straight back for
//! the terminal; the tap only watches for the header `rz` or `sz` opens
//! with. When it sees one it tells the terminal (`zmodem-{id}` events), which
//! asks for files to send or a folder to save into, and answers through
//! [`decide`]. The transfer then runs on the session's own connection: the
//! loop writes what the tap hands back as `wire`, without the session's
//! newline or Telnet text-mode changes. Keys typed meanwhile are held back
//! ([`intercept_input`]); Ctrl+C cancels.

pub mod detect;
pub mod transfer;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

pub use detect::Direction;
use detect::Detector;
use transfer::{End, Transfer};

/// What the user chose, sent from the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Upload(Vec<PathBuf>),
    Download(PathBuf),
    Cancel,
}

/// Told to the terminal as `zmodem-{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ZmodemEvent {
    /// The remote started `rz` or `sz`: ask the user.
    Offer { direction: Direction },
    Progress {
        direction: Direction,
        file: String,
        index: usize,
        count: Option<usize>,
        done: u64,
        total: Option<u64>,
    },
    Finished {
        direction: Direction,
        files: Vec<String>,
        /// Where a download went.
        folder: Option<String>,
    },
    Cancelled { direction: Direction },
    Failed { direction: Direction, error: String },
}

/// The bytes a tap hands back.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// For the terminal.
    pub display: Vec<u8>,
    /// For the remote, written as is.
    pub wire: Vec<u8>,
}

/// Ten CAN, which make `rz` or `sz` give up, then ten backspaces to erase
/// them from a shell line: what lrzsz sends to cancel.
const CANCEL: [u8; 20] = [
    0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
];
/// The remote has said nothing for this long: ask again.
const RETRY_AFTER: Duration = Duration::from_secs(10);
/// Asking again this many times without an answer ends the transfer.
const MAX_RETRIES: u8 = 4;
/// An offer nobody answers is withdrawn.
const OFFER_TIMEOUT: Duration = Duration::from_secs(600);
/// How often progress goes to the terminal.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);
/// After a cancel, what the remote still had in flight is not shown: up to
/// this long, or until it goes quiet.
const DRAIN_MAX: Duration = Duration::from_millis(1500);
const DRAIN_QUIET: Duration = Duration::from_millis(300);

/// Subpackets (1 KiB each) sent before waiting for an acknowledgement. A
/// network link waits a round trip for each, so it gets a long window; a
/// serial line has no round trip to speak of, and resends less after noise.
pub const WINDOW_NETWORK: usize = 256;
pub const WINDOW_SERIAL: usize = 8;

struct Control {
    decision: Mutex<Option<Decision>>,
    busy: AtomicBool,
}

static TAPS: LazyLock<Mutex<HashMap<String, Arc<Control>>>> = LazyLock::new(Default::default);

fn taps() -> std::sync::MutexGuard<'static, HashMap<String, Arc<Control>>> {
    TAPS.lock().unwrap_or_else(|p| p.into_inner())
}

/// The terminal's answer for session `id`. It takes effect on the
/// session's next tick.
pub fn decide(id: &str, decision: Decision) -> Result<(), String> {
    let control = taps().get(id).cloned().ok_or_else(|| "That session is closed".to_string())?;
    if !control.busy.load(Ordering::SeqCst) {
        return Err("No ZMODEM transfer is waiting".into());
    }
    *control.decision.lock().unwrap_or_else(|p| p.into_inner()) = Some(decision);
    Ok(())
}

/// Keys typed into session `id`: true when a transfer holds the line and
/// they must not go out. Ctrl+C cancels the transfer.
pub fn intercept_input(id: &str, data: &[u8]) -> bool {
    let Some(control) = taps().get(id).cloned() else {
        return false;
    };
    if !control.busy.load(Ordering::SeqCst) {
        return false;
    }
    if data.contains(&0x03) {
        *control.decision.lock().unwrap_or_else(|p| p.into_inner()) = Some(Decision::Cancel);
    }
    true
}

enum State {
    Idle,
    Offered {
        direction: Direction,
        since: Instant,
    },
    Active {
        direction: Direction,
        transfer: Box<Transfer>,
        folder: Option<PathBuf>,
        last_input: Instant,
        retries: u8,
        /// The file and time progress was last told for.
        last_progress: Option<(usize, Instant)>,
        /// CAN bytes in a row from the remote: five are its cancel.
        cans: u8,
    },
    /// Just after a transfer: the "OO" `sz` ends with, or after a cancel
    /// whatever was still on its way, is not shown.
    Draining {
        until: Instant,
        last_input: Instant,
        /// Only an "OO" is eaten, then output shows again.
        over_and_out: bool,
        os_left: u8,
    },
}

type Emit = Box<dyn Fn(ZmodemEvent) + Send + Sync>;

/// One session's ZMODEM watcher and transfer.
pub struct Tap {
    id: String,
    control: Arc<Control>,
    emit: Emit,
    window: usize,
    detector: Detector,
    state: State,
}

impl Tap {
    /// A tap for session `id`, telling the terminal through `emit`.
    pub fn new(id: &str, window: usize, emit: Emit) -> Self {
        let control = Arc::new(Control { decision: Mutex::new(None), busy: AtomicBool::new(false) });
        taps().insert(id.to_string(), control.clone());
        Self { id: id.to_string(), control, emit, window, detector: Detector::new(), state: State::Idle }
    }

    /// A tap telling the terminal through Tauri events.
    pub fn for_app(app: &tauri::AppHandle, id: &str, window: usize) -> Self {
        use tauri::Emitter;
        let app = app.clone();
        let event = format!("zmodem-{id}");
        Self::new(id, window, Box::new(move |e| {
            let _ = app.emit(&event, e);
        }))
    }

    /// Something is going on: the session loop should call [`Tap::poll`]
    /// every 100 ms or so.
    pub fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Bytes held back at the end of the stream (part of what looked like
    /// the start of a header).
    pub fn flush(&mut self) -> Vec<u8> {
        if matches!(self.state, State::Idle) { self.detector.flush() } else { Vec::new() }
    }

    /// What the remote sent.
    pub fn input(&mut self, data: &[u8]) -> Output {
        let mut out = Output::default();
        self.take(data, &mut out);
        // A cancel is seen even while data streams in without a pause.
        if self.busy() {
            let more = self.poll();
            out.display.extend(more.display);
            out.wire.extend(more.wire);
        }
        out
    }

    /// Time passing: the user's answer, retries, progress.
    pub fn poll(&mut self) -> Output {
        let mut out = Output::default();
        let decision = self.control.decision.lock().unwrap_or_else(|p| p.into_inner()).take();
        let now = Instant::now();
        match &mut self.state {
            State::Idle => {}
            State::Offered { direction, since } => {
                let direction = *direction;
                let decision = decision.or_else(|| (since.elapsed() > OFFER_TIMEOUT).then_some(Decision::Cancel));
                match decision {
                    None => {}
                    Some(Decision::Cancel) => {
                        out.wire.extend_from_slice(&CANCEL);
                        (self.emit)(ZmodemEvent::Cancelled { direction });
                        self.drain(false);
                    }
                    Some(d) => {
                        let (started, folder) = match (direction, d) {
                            (Direction::Upload, Decision::Upload(files)) => (Transfer::send(files, self.window), None),
                            (Direction::Download, Decision::Download(folder)) => (Transfer::receive(folder.clone()), Some(folder)),
                            _ => (Err("That is not what the remote asked for".to_string()), None),
                        };
                        match started {
                            Ok(mut transfer) => {
                                // The opening header goes out at once.
                                transfer.timeout(&mut out.wire);
                                self.state = State::Active {
                                    direction,
                                    transfer: Box::new(transfer),
                                    folder,
                                    last_input: now,
                                    retries: 0,
                                    last_progress: None,
                                    cans: 0,
                                };
                            }
                            Err(error) => {
                                out.wire.extend_from_slice(&CANCEL);
                                (self.emit)(ZmodemEvent::Failed { direction, error });
                                self.drain(false);
                            }
                        }
                    }
                }
            }
            State::Active { transfer, last_input, retries, .. } => {
                if decision == Some(Decision::Cancel) {
                    transfer.abort(&mut out.wire, None);
                } else if last_input.elapsed() >= RETRY_AFTER {
                    *retries += 1;
                    *last_input = now;
                    if *retries > MAX_RETRIES {
                        transfer.abort(&mut out.wire, Some("The remote stopped answering".into()));
                    } else {
                        transfer.timeout(&mut out.wire);
                    }
                }
                self.after_transfer();
            }
            State::Draining { until, last_input, over_and_out, .. } => {
                let quiet = !*over_and_out && last_input.elapsed() >= DRAIN_QUIET;
                if quiet || (!*over_and_out && now >= *until) {
                    // A header cut off by the cancel may still end in its
                    // CR LF XON: those are not shown either.
                    self.drain(true);
                } else if now >= *until {
                    self.idle();
                }
            }
        }
        out
    }

    fn take(&mut self, data: &[u8], out: &mut Output) {
        if data.is_empty() {
            return;
        }
        match &mut self.state {
            State::Idle => {
                let scan = self.detector.scan(data);
                let mut shown = scan.display;
                if let Some((direction, _header)) = scan.start {
                    // The header's own `*`s are not text, and the line the
                    // program printed ("rz waiting to receive.", or sz's
                    // "rz") is cleared: the bar over the terminal says what
                    // is going on, and the prompt after the transfer starts
                    // on a clean line.
                    while shown.last() == Some(&b'*') {
                        shown.pop();
                    }
                    shown.extend_from_slice(b"\r\x1b[K");
                    out.display.extend(shown);
                    // What came with the header is the opening handshake,
                    // which the transfer starts again once it is set up.
                    self.set_busy(true);
                    self.state = State::Offered { direction, since: Instant::now() };
                    (self.emit)(ZmodemEvent::Offer { direction });
                } else {
                    out.display.extend(shown);
                }
            }
            // The remote repeats its opening until it is answered.
            State::Offered { .. } => {}
            State::Active { transfer, last_input, retries, cans, .. } => {
                *last_input = Instant::now();
                *retries = 0;
                for &b in data {
                    *cans = if b == 0x18 { cans.saturating_add(1) } else { 0 };
                    if *cans >= 5 {
                        break;
                    }
                }
                let used = if *cans >= 5 {
                    transfer.cancelled_by_remote();
                    data.len()
                } else {
                    transfer.feed(data, &mut out.wire)
                };
                self.after_transfer();
                if used < data.len() {
                    self.take(&data[used..], out);
                }
            }
            State::Draining { last_input, over_and_out, os_left, .. } => {
                *last_input = Instant::now();
                if !*over_and_out {
                    return;
                }
                // What is left of the last header (its CR LF, with the high
                // bit set, and XON) and sz's "OO" are not shown.
                let mut eaten = 0;
                for &b in data {
                    if b == b'O' && *os_left > 0 {
                        *os_left -= 1;
                    } else if !matches!(b, b'\r' | b'\n' | 0x8d | 0x8a | 0x11) {
                        break;
                    }
                    eaten += 1;
                }
                if eaten < data.len() || *os_left == 0 {
                    self.idle();
                    self.take(&data[eaten..], out);
                }
            }
        }
    }

    /// Progress, or the end of a transfer, to the terminal.
    fn after_transfer(&mut self) {
        let State::Active { direction, transfer, folder, last_progress, .. } = &mut self.state else {
            return;
        };
        let direction = *direction;
        let Some(end) = transfer.end().cloned() else {
            let p = transfer.progress();
            let fresh = last_progress.is_none_or(|(i, t)| i != p.index || t.elapsed() >= PROGRESS_EVERY);
            if fresh && !p.file.is_empty() {
                *last_progress = Some((p.index, Instant::now()));
                {
                    (self.emit)(ZmodemEvent::Progress {
                        direction,
                        file: p.file.clone(),
                        index: p.index,
                        count: p.count,
                        done: p.done,
                        total: p.total,
                    });
                }
            }
            return;
        };
        let folder = folder.as_ref().map(|f| f.display().to_string());
        match end {
            End::Done(files) => {
                (self.emit)(ZmodemEvent::Finished { direction, files, folder });
                self.drain(true);
            }
            End::Cancelled => {
                (self.emit)(ZmodemEvent::Cancelled { direction });
                self.drain(false);
            }
            End::Failed(error) => {
                (self.emit)(ZmodemEvent::Failed { direction, error });
                self.drain(false);
            }
        }
    }

    fn drain(&mut self, over_and_out: bool) {
        let now = Instant::now();
        let until = now + if over_and_out { Duration::from_secs(1) } else { DRAIN_MAX };
        self.state = State::Draining { until, last_input: now, over_and_out, os_left: 2 };
    }

    fn idle(&mut self) {
        self.state = State::Idle;
        self.detector = Detector::new();
        self.set_busy(false);
        // An answer that came too late is not kept for the next transfer.
        self.control.decision.lock().unwrap_or_else(|p| p.into_inner()).take();
    }

    fn set_busy(&self, busy: bool) {
        self.control.busy.store(busy, Ordering::SeqCst);
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        let mut taps = taps();
        if taps.get(&self.id).is_some_and(|c| Arc::ptr_eq(c, &self.control)) {
            taps.remove(&self.id);
        }
    }
}
