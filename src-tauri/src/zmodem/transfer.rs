//! One ZMODEM session, driving [`zmodem2`]'s state machines with real files.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use cap_std::fs::Dir;
use crate::local_paths::{download_subdir, open_root, plain_name, PendingFile};
use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};

/// Where a transfer is, for the progress bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// The file going now.
    pub file: String,
    /// Which one it is, from 1.
    pub index: usize,
    /// How many there are, when known (an upload).
    pub count: Option<usize>,
    /// Bytes of this file so far.
    pub done: u64,
    /// Its size, when told.
    pub total: Option<u64>,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// All went; the files done (names as saved, for a download).
    Done(Vec<String>),
    /// Called off, here or by the remote.
    Cancelled,
    Failed(String),
}

struct Incoming {
    file: BufWriter<File>,
    pending: PendingFile,
    name: String,
    modified: Option<u32>,
}

struct Outgoing {
    file: File,
    name: String,
}

enum Role {
    Send {
        sender: Box<Sender>,
        /// Files still to go, with the name each is sent under ("dir/sub/x"
        /// for one inside a folder sent whole).
        queue: VecDeque<(PathBuf, String)>,
        current: Option<Outgoing>,
    },
    Receive {
        receiver: Box<Receiver>,
        folder: Dir,
        current: Option<Incoming>,
    },
}

pub struct Transfer {
    role: Role,
    progress: Progress,
    done_files: Vec<String>,
    end: Option<End>,
    failure: Option<String>,
}

/// File sizes travel as 32-bit offsets.
const MAX_SIZE: u64 = u32::MAX as u64;

impl Transfer {
    /// Send `files` to a remote `rz`; a folder goes with everything in it,
    /// under its own name (`rz` makes the folders). `window` is how many
    /// 1 KiB subpackets go out before waiting for the receiver to
    /// acknowledge them.
    pub fn send(paths: Vec<PathBuf>, window: usize) -> Result<Self, String> {
        let files = expand(&paths)?;
        if files.is_empty() {
            return Err("No files to send (the folders are empty)".into());
        }
        let mut sender = Box::new(Sender::new().map_err(err)?);
        sender.set_streaming_window(window);
        let count = files.len();
        let mut t = Self {
            role: Role::Send { sender, queue: files.into(), current: None },
            progress: Progress { count: Some(count), ..Default::default() },
            done_files: Vec::new(),
            end: None,
            failure: None,
        };
        t.next_upload()?;
        Ok(t)
    }

    /// Receive from a remote `sz` into `folder`.
    pub fn receive(folder: PathBuf) -> Result<Self, String> {
        let folder = open_root(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        // Nonstop, written while receiving: what lets `sz` stream.
        let receiver = Box::new(Receiver::with_flow_control(0, true).map_err(err)?);
        Ok(Self {
            role: Role::Receive { receiver, folder, current: None },
            progress: Progress::default(),
            done_files: Vec::new(),
            end: None,
            failure: None,
        })
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// Set once the session is over and its last bytes are out.
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// Take what the remote sent. Returns how much of `input` belonged to
    /// the session: once it ends, the rest is ordinary terminal output.
    pub fn feed(&mut self, input: &[u8], wire: &mut Vec<u8>) -> usize {
        let mut used = 0;
        let mut stalled = 0;
        while self.end.is_none() {
            self.pump(wire);
            if self.end.is_some() || used >= input.len() {
                break;
            }
            let fed = match &mut self.role {
                Role::Send { sender, .. } => sender.submit_wire(&input[used..]),
                Role::Receive { receiver, .. } => receiver.submit_wire(&input[used..]),
            };
            match fed {
                Ok(0) => {
                    stalled += 1;
                    if stalled > 3 {
                        // Nothing will take these bytes: line noise.
                        used = input.len();
                    }
                }
                Ok(n) => {
                    stalled = 0;
                    used += n;
                }
                Err(e) => {
                    // A damaged header: the state machine has asked for it
                    // again (ZNAK); the rest of this read is not trusted.
                    tracing::debug!("ZMODEM: {e}");
                    used = input.len();
                }
            }
        }
        self.pump(wire);
        if self.end.is_some() { used } else { input.len() }
    }

    /// The remote has said nothing for a while: ask again.
    pub fn timeout(&mut self, wire: &mut Vec<u8>) {
        let _ = match &mut self.role {
            Role::Send { sender, .. } => sender.timeout(),
            Role::Receive { receiver, .. } => receiver.timeout(),
        };
        self.pump(wire);
    }

    /// Call it off: the cancel sequence goes out, and a partial download is
    /// removed.
    pub fn abort(&mut self, wire: &mut Vec<u8>, why: Option<String>) {
        if self.end.is_some() {
            return;
        }
        self.failure = why;
        match &mut self.role {
            Role::Send { sender, .. } => sender.abort(),
            Role::Receive { receiver, .. } => {
                let _ = receiver.abort();
            }
        }
        self.pump(wire);
    }

    /// The remote sent its cancel: nothing more goes out.
    pub fn cancelled_by_remote(&mut self) {
        if self.end.is_none() {
            self.discard_partial();
            self.end = Some(End::Cancelled);
        }
    }

    /// Work through what the state machine wants until it waits on the remote.
    fn pump(&mut self, wire: &mut Vec<u8>) {
        while self.end.is_none() {
            let step = match &mut self.role {
                Role::Send { sender, current, .. } => match sender.poll() {
                    Action::WriteWire(b) => {
                        wire.extend_from_slice(b);
                        let n = b.len();
                        sender.wire_written(n);
                        Step::Continue
                    }
                    Action::ReadFile { offset, max_len } => {
                        let read = current
                            .as_mut()
                            .ok_or_else(|| "no file open".to_string())
                            .and_then(|c| read_at(&mut c.file, offset.get(), max_len).map_err(|e| format!("{}: {e}", c.name)));
                        match read.and_then(|data| sender.submit_file(&data).map(|_| data.len()).map_err(err)) {
                            Ok(n) => {
                                self.progress.done = u64::from(offset.get()) + n as u64;
                                Step::Continue
                            }
                            Err(e) => Step::Fail(e),
                        }
                    }
                    Action::Event(Event::FileCompleted) => Step::FileDone,
                    Action::Event(Event::SessionCompleted) => Step::SessionDone,
                    Action::Event(Event::Aborted) => Step::Aborted,
                    Action::Event(_) | Action::WriteFile(_) => Step::Continue,
                    Action::Idle => Step::Idle,
                    _ => Step::Idle,
                },
                Role::Receive { receiver, folder, current } => match receiver.poll() {
                    Action::WriteWire(b) => {
                        wire.extend_from_slice(b);
                        let n = b.len();
                        receiver.wire_written(n);
                        Step::Continue
                    }
                    Action::WriteFile(b) => {
                        let n = b.len();
                        let wrote = match current.as_mut() {
                            Some(c) => c.file.write_all(b).map_err(|e| format!("{}: {e}", c.name)),
                            None => Ok(()),
                        };
                        match wrote.and_then(|_| receiver.file_written(n).map_err(err)) {
                            Ok(()) => {
                                self.progress.done += n as u64;
                                Step::Continue
                            }
                            Err(e) => Step::Fail(e),
                        }
                    }
                    Action::Event(Event::FileStarted(info)) => {
                        let name = String::from_utf8_lossy(info.name).into_owned();
                        let total = info.size.map(|s| u64::from(s.get()));
                        let modified = info.modified;
                        match create_unique(folder, &name) {
                            Ok((file, pending, saved)) => {
                                *current = Some(Incoming { file: BufWriter::with_capacity(64 * 1024, file), pending, name: saved.clone(), modified });
                                self.progress = Progress { file: saved, index: self.progress.index + 1, count: None, done: 0, total };
                                Step::Continue
                            }
                            Err(e) => Step::Fail(e),
                        }
                    }
                    Action::Event(Event::FileCompleted) => Step::FileDone,
                    Action::Event(Event::SessionCompleted) => Step::SessionDone,
                    Action::Event(Event::Aborted) => Step::Aborted,
                    Action::Event(_) | Action::ReadFile { .. } => Step::Continue,
                    Action::Idle => Step::Idle,
                    _ => Step::Idle,
                },
            };
            match step {
                Step::Continue => {}
                Step::Idle => break,
                Step::FileDone => {
                    if let Err(e) = self.file_done() {
                        self.fail(wire, e);
                    }
                }
                Step::SessionDone => {
                    // Flush what is still queued ("OO" after ZFIN) first.
                    self.drain(wire);
                    self.end = Some(End::Done(std::mem::take(&mut self.done_files)));
                }
                Step::Aborted => {
                    self.drain(wire);
                    self.discard_partial();
                    self.end = Some(match self.failure.take() {
                        Some(e) => End::Failed(e),
                        None => End::Cancelled,
                    });
                }
                Step::Fail(e) => self.fail(wire, e),
            }
        }
    }

    /// Out with any bytes still queued, once the session is over.
    fn drain(&mut self, wire: &mut Vec<u8>) {
        for _ in 0..8 {
            let written = match &mut self.role {
                Role::Send { sender, .. } => match sender.poll() {
                    Action::WriteWire(b) => {
                        wire.extend_from_slice(b);
                        let n = b.len();
                        sender.wire_written(n);
                        true
                    }
                    _ => false,
                },
                Role::Receive { receiver, .. } => match receiver.poll() {
                    Action::WriteWire(b) => {
                        wire.extend_from_slice(b);
                        let n = b.len();
                        receiver.wire_written(n);
                        true
                    }
                    _ => false,
                },
            };
            if !written {
                break;
            }
        }
    }

    fn fail(&mut self, wire: &mut Vec<u8>, why: String) {
        tracing::warn!("ZMODEM: {why}");
        self.abort(wire, Some(why));
    }

    /// The file in hand is complete: close it, and start the next upload or
    /// say there are no more.
    fn file_done(&mut self) -> Result<(), String> {
        match &mut self.role {
            Role::Receive { current, .. } => {
                if let Some(mut c) = current.take() {
                    c.file.flush().map_err(|e| format!("{}: {e}", c.name))?;
                    if let Some(t) = c.modified {
                        let _ = c.file.get_ref().set_modified(UNIX_EPOCH + Duration::from_secs(u64::from(t)));
                    }
                    c.pending.keep();
                    self.done_files.push(c.name);
                }
                Ok(())
            }
            Role::Send { current, .. } => {
                if let Some(c) = current.take() {
                    self.done_files.push(c.name);
                }
                self.next_upload()
            }
        }
    }

    /// Offer the next file in the queue, or end the session.
    fn next_upload(&mut self) -> Result<(), String> {
        let Role::Send { sender, queue, current } = &mut self.role else {
            return Ok(());
        };
        let Some((path, name)) = queue.pop_front() else {
            return sender.finish().map_err(err);
        };
        let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
        let size = u32::try_from(meta.len()).map_err(|_| format!("{name} is larger than 4 GB"))?;
        let mut info = FileInfo::new(name.as_bytes(), Some(Position::new(size))).with_mode(0o100_644);
        if let Some(t) = meta.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).and_then(|d| u32::try_from(d.as_secs()).ok()) {
            info = info.with_modified(t);
        }
        sender.start_file(info).map_err(err)?;
        self.progress = Progress {
            file: name.clone(),
            index: self.progress.index + 1,
            count: self.progress.count,
            done: 0,
            total: Some(u64::from(size)),
        };
        *current = Some(Outgoing { file, name });
        Ok(())
    }

    /// A download that did not finish is not left behind half written.
    fn discard_partial(&mut self) {
        if let Role::Receive { current, .. } = &mut self.role {
            *current = None;
        }
    }
}

enum Step {
    Continue,
    Idle,
    FileDone,
    SessionDone,
    Aborted,
    Fail(String),
}

fn err(e: zmodem2::Error) -> String {
    format!("ZMODEM: {e}")
}

fn read_at(file: &mut File, offset: u32, max_len: usize) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut data = vec![0u8; max_len];
    let mut got = 0;
    while got < max_len {
        match file.read(&mut data[got..])? {
            0 => break,
            n => got += n,
        }
    }
    if got == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the file got shorter while sending"));
    }
    data.truncate(got);
    Ok(data)
}

/// A name from the remote, made safe to save: no folders, no `.` or `..`,
/// no characters a file system refuses.
pub fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    // Dotfiles keep their dot; trailing dots and spaces go (Windows drops
    // them), and a name of only dots is no name.
    let trimmed = cleaned.trim_start_matches(char::is_whitespace).trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '.') {
        return "download".into();
    }
    // Windows keeps these names for devices.
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return format!("_{trimmed}");
    }
    trimmed.to_string()
}

/// A path from the remote (`sz -f` sends "dir/sub/x"), made safe to save
/// under the chosen folder: its folders, each made safe, without empty,
/// `.` or `..` parts, and the file name.
pub fn safe_path(raw: &str) -> (Vec<String>, String) {
    let mut parts: Vec<String> = raw
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(safe_name)
        .collect();
    let file = parts.pop().unwrap_or_else(|| "download".into());
    // Deeper than any real tree; keeps a hostile name from going on forever.
    parts.truncate(32);
    (parts, file)
}

/// Every file to send: a file as itself, a folder's files (all levels) as
/// "folder/sub/name". Links to folders are not followed.
fn expand(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(PathBuf, String)>) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = format!("{prefix}/{}", e.file_name().to_string_lossy());
            let kind = e.file_type().map_err(|err| format!("{}: {err}", e.path().display()))?;
            let path = e.path();
            if kind.is_dir() {
                walk(&path, &name, out)?;
            } else if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                out.push((path, name));
            }
            if out.len() > 100_000 {
                return Err("More than 100,000 files: too many to send this way".into());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        if meta.is_dir() {
            walk(p, &name, &mut out)?;
        } else {
            out.push((p.clone(), name));
        }
    }
    for (p, _) in &out {
        if std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > MAX_SIZE {
            return Err(format!("{} is larger than 4 GB, which ZMODEM cannot send", p.display()));
        }
    }
    Ok(out)
}

/// Make a new file for `name` (which may have folders in it) in `folder`,
/// as "name (1).ext" and so on when it is taken: a download never
/// overwrites. Folders that are already there are used as they are.
fn create_unique(root: &Dir, name: &str) -> Result<(File, PendingFile, String), String> {
    let (dirs, name) = safe_path(name);
    plain_name(&name).map_err(|e| format!("{name}: {e}"))?;
    let dir = download_subdir(root, &dirs).map_err(|e| format!("{name}: {e}"))?;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    for n in 0..1000 {
        let candidate = if n == 0 { name.clone() } else { format!("{stem} ({n}){ext}") };
        match dir.open_with(&candidate, &options) {
            Ok(file) => {
                let path = dirs.iter().map(String::as_str).chain(std::iter::once(candidate.as_str()))
                    .collect::<Vec<_>>().join("/");
                return Ok((file.into_std(), PendingFile::new(dir, candidate), path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{candidate}: {e}")),
        }
    }
    Err(format!("Could not find a free name for {name}"))
}
