//! The Telnet protocol itself, kept free of sockets and Tauri so it can be
//! tested on its own (see the tests at the bottom).
//!
//! [`TelnetCodec`] turns what the server sends into terminal output plus the
//! replies the protocol owes it, and turns what the user types into what goes
//! on the wire. [`AutoLogin`] watches the output for a login prompt.
//!
//! Option negotiation follows RFC 854/855 with the loop protection of RFC 1143
//! kept simple: an option's state only changes once per request, and a request
//! that would not change it is not answered. Reach offers what a terminal
//! needs and refuses everything else:
//!
//! - we do: NAWS (window size, RFC 1073), TERMINAL-TYPE (RFC 1091),
//!   SUPPRESS-GO-AHEAD and BINARY;
//! - the server may: ECHO, SUPPRESS-GO-AHEAD and BINARY.

use serde::{Deserialize, Serialize};

pub const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_BINARY: u8 = 0;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

/// Terminal types offered, in order, each time the server asks again
/// (RFC 1091): the last one repeats once the list is used up. Old gear that
/// does not know xterm usually picks VT100.
const TERMINAL_TYPES: [&str; 3] = ["XTERM-256COLOR", "XTERM", "VT100"];

/// How the connection talks to the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelnetMode {
    /// Telnet, and Reach asks for the options it wants as soon as it connects.
    #[default]
    Active,
    /// Telnet, but Reach only answers what the server asks for. For devices
    /// that get confused when the client speaks first.
    Passive,
    /// No Telnet at all: bytes go through untouched. For the raw TCP ports
    /// of console servers in front of serial-only gear.
    Raw,
    /// A Proxmox text console (termproxy): no Telnet, what is typed goes as
    /// `0:<length>:<bytes>` and a new size as `1:<cols>:<rows>:`. Set by
    /// the backend for a console tab only.
    Proxmox,
}

/// What the Enter key sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelnetNewline {
    /// CR LF, the Telnet end of line (RFC 854). Works almost everywhere.
    #[default]
    Crlf,
    /// CR alone (sent as CR NUL in Telnet text mode, as RFC 854 asks).
    Cr,
    /// LF alone.
    Lf,
}

/// What the Backspace key sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelnetBackspace {
    /// DEL (0x7f), as the terminal sends it.
    #[default]
    Del,
    /// Ctrl+H (0x08), for gear that erases with that.
    CtrlH,
}

/// A Telnet session's settings, stored with the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TelnetOptions {
    #[serde(default)]
    pub mode: TelnetMode,
    #[serde(default)]
    pub newline: TelnetNewline,
    #[serde(default)]
    pub backspace: TelnetBackspace,
}

#[derive(Debug, Clone, Copy, Default)]
struct OptState {
    enabled: bool,
    /// We asked and have not heard back.
    pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parse {
    Data,
    /// Just read a CR in text mode: a NUL after it is dropped.
    Cr,
    Iac,
    Verb(u8),
    Sub,
    SubIac,
}

/// The result of reading from the server.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Received {
    /// What the terminal shows.
    pub data: Vec<u8>,
    /// What to send back to the server (negotiation answers).
    pub reply: Vec<u8>,
}

pub struct TelnetCodec {
    options: TelnetOptions,
    /// Options on our side (we WILL), by number.
    local: [OptState; 256],
    /// Options on the server's side (it WILL), by number.
    remote: [OptState; 256],
    parse: Parse,
    sub: Vec<u8>,
    cols: u16,
    rows: u16,
    ttype_sent: usize,
}

impl TelnetCodec {
    pub fn new(options: TelnetOptions, cols: u16, rows: u16) -> Self {
        Self {
            options,
            local: [OptState::default(); 256],
            remote: [OptState::default(); 256],
            parse: Parse::Data,
            sub: Vec::new(),
            cols,
            rows,
            ttype_sent: 0,
        }
    }

    fn telnet(&self) -> bool {
        !matches!(self.options.mode, TelnetMode::Raw | TelnetMode::Proxmox)
    }

    fn proxmox(&self) -> bool {
        self.options.mode == TelnetMode::Proxmox
    }

    /// What to send as soon as the connection is up: in active mode, the
    /// options Reach wants. Nothing otherwise.
    pub fn start(&mut self) -> Vec<u8> {
        // A Proxmox console starts at 80x24: it is told the terminal's size.
        if self.proxmox() {
            return crate::proxmox::ws::term_resize(self.cols, self.rows);
        }
        if self.options.mode != TelnetMode::Active {
            return Vec::new();
        }
        let mut out = Vec::new();
        for opt in [OPT_NAWS, OPT_TTYPE, OPT_SGA] {
            self.local[opt as usize].pending = true;
            out.extend_from_slice(&[IAC, WILL, opt]);
        }
        for opt in [OPT_SGA, OPT_ECHO] {
            self.remote[opt as usize].pending = true;
            out.extend_from_slice(&[IAC, DO, opt]);
        }
        out
    }

    /// Whether the server echoes what is typed (it said WILL ECHO).
    pub fn server_echoes(&self) -> bool {
        self.remote[OPT_ECHO as usize].enabled
    }

    /// Read bytes from the server.
    pub fn receive(&mut self, input: &[u8]) -> Received {
        let mut out = Received::default();
        if !self.telnet() {
            out.data.extend_from_slice(input);
            return out;
        }
        let binary_in = self.remote[OPT_BINARY as usize].enabled;
        for &b in input {
            match self.parse {
                Parse::Data | Parse::Cr => {
                    let after_cr = self.parse == Parse::Cr;
                    self.parse = Parse::Data;
                    if b == IAC {
                        self.parse = Parse::Iac;
                    } else if after_cr && b == 0 {
                        // CR NUL is a bare carriage return.
                    } else {
                        out.data.push(b);
                        if b == b'\r' && !binary_in {
                            self.parse = Parse::Cr;
                        }
                    }
                }
                Parse::Iac => match b {
                    IAC => {
                        out.data.push(IAC);
                        self.parse = Parse::Data;
                    }
                    WILL | WONT | DO | DONT => self.parse = Parse::Verb(b),
                    SB => {
                        self.sub.clear();
                        self.parse = Parse::Sub;
                    }
                    // NOP, Go Ahead, Data Mark, Are You There and the rest:
                    // nothing for a terminal to do.
                    _ => self.parse = Parse::Data,
                },
                Parse::Verb(verb) => {
                    self.parse = Parse::Data;
                    self.negotiate(verb, b, &mut out.reply);
                }
                Parse::Sub => {
                    if b == IAC {
                        self.parse = Parse::SubIac;
                    } else if self.sub.len() < 1024 {
                        self.sub.push(b);
                    }
                }
                Parse::SubIac => match b {
                    SE => {
                        self.parse = Parse::Data;
                        let sub = std::mem::take(&mut self.sub);
                        self.subnegotiation(&sub, &mut out.reply);
                    }
                    IAC => {
                        if self.sub.len() < 1024 {
                            self.sub.push(IAC);
                        }
                        self.parse = Parse::Sub;
                    }
                    // A broken subnegotiation: drop it, read on as data.
                    _ => {
                        self.sub.clear();
                        self.parse = Parse::Data;
                    }
                },
            }
        }
        out
    }

    fn negotiate(&mut self, verb: u8, opt: u8, reply: &mut Vec<u8>) {
        let i = opt as usize;
        match verb {
            WILL => {
                let st = &mut self.remote[i];
                if !matches!(opt, OPT_ECHO | OPT_SGA | OPT_BINARY) {
                    reply.extend_from_slice(&[IAC, DONT, opt]);
                } else if !st.enabled {
                    st.enabled = true;
                    if !st.pending {
                        reply.extend_from_slice(&[IAC, DO, opt]);
                    }
                    st.pending = false;
                }
            }
            WONT => {
                let st = &mut self.remote[i];
                if st.enabled || st.pending {
                    if !st.pending {
                        reply.extend_from_slice(&[IAC, DONT, opt]);
                    }
                    *st = OptState::default();
                }
            }
            DO => {
                let st = &mut self.local[i];
                if !matches!(opt, OPT_NAWS | OPT_TTYPE | OPT_SGA | OPT_BINARY) {
                    reply.extend_from_slice(&[IAC, WONT, opt]);
                } else if !st.enabled {
                    st.enabled = true;
                    if !st.pending {
                        reply.extend_from_slice(&[IAC, WILL, opt]);
                    }
                    st.pending = false;
                    if opt == OPT_NAWS {
                        reply.extend_from_slice(&self.naws());
                    }
                }
            }
            DONT => {
                let st = &mut self.local[i];
                if st.enabled || st.pending {
                    if !st.pending {
                        reply.extend_from_slice(&[IAC, WONT, opt]);
                    }
                    *st = OptState::default();
                }
            }
            _ => {}
        }
    }

    fn subnegotiation(&mut self, sub: &[u8], reply: &mut Vec<u8>) {
        if sub.len() >= 2
            && sub[0] == OPT_TTYPE
            && sub[1] == TTYPE_SEND
            && self.local[OPT_TTYPE as usize].enabled
        {
            let name = TERMINAL_TYPES[self.ttype_sent.min(TERMINAL_TYPES.len() - 1)];
            self.ttype_sent += 1;
            reply.extend_from_slice(&[IAC, SB, OPT_TTYPE, TTYPE_IS]);
            reply.extend_from_slice(name.as_bytes());
            reply.extend_from_slice(&[IAC, SE]);
        }
    }

    fn naws(&self) -> Vec<u8> {
        let mut out = vec![IAC, SB, OPT_NAWS];
        for v in [self.cols, self.rows] {
            for b in v.to_be_bytes() {
                out.push(b);
                if b == IAC {
                    out.push(IAC);
                }
            }
        }
        out.extend_from_slice(&[IAC, SE]);
        out
    }

    /// The terminal changed size: what to tell the server, if it asked to know.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        if cols == self.cols && rows == self.rows {
            return Vec::new();
        }
        self.cols = cols;
        self.rows = rows;
        if self.proxmox() {
            return crate::proxmox::ws::term_resize(cols, rows);
        }
        if self.telnet() && self.local[OPT_NAWS as usize].enabled {
            self.naws()
        } else {
            Vec::new()
        }
    }

    /// Binary data (a ZMODEM transfer) as it goes on the wire: only what
    /// Telnet itself asks for, IAC doubled and, in text mode, CR followed
    /// by NUL, which the server takes off again. No newline or backspace
    /// changes.
    pub fn encode_raw(&self, input: &[u8]) -> Vec<u8> {
        if self.proxmox() {
            return crate::proxmox::ws::term_input(input);
        }
        if !self.telnet() {
            return input.to_vec();
        }
        let binary_out = self.local[OPT_BINARY as usize].enabled;
        let mut out = Vec::with_capacity(input.len() + input.len() / 64 + 4);
        for &b in input {
            match b {
                IAC => out.extend_from_slice(&[IAC, IAC]),
                b'\r' if !binary_out => out.extend_from_slice(&[b'\r', 0]),
                _ => out.push(b),
            }
        }
        out
    }

    /// What the user typed, as it goes on the wire.
    pub fn encode(&self, input: &[u8]) -> Vec<u8> {
        // A Proxmox console is a terminal of its own: keys go as typed.
        if self.proxmox() {
            return crate::proxmox::ws::term_input(input);
        }
        let telnet = self.telnet();
        let binary_out = self.local[OPT_BINARY as usize].enabled;
        let mut out = Vec::with_capacity(input.len() + 4);
        for &b in input {
            match b {
                b'\r' => match self.options.newline {
                    TelnetNewline::Crlf => out.extend_from_slice(b"\r\n"),
                    TelnetNewline::Lf => out.push(b'\n'),
                    TelnetNewline::Cr => {
                        out.push(b'\r');
                        if telnet && !binary_out {
                            out.push(0);
                        }
                    }
                },
                0x7f if self.options.backspace == TelnetBackspace::CtrlH => out.push(0x08),
                IAC if telnet => out.extend_from_slice(&[IAC, IAC]),
                _ => out.push(b),
            }
        }
        out
    }
}

/// Types a saved login in when the device asks for it: the user name at a
/// "Username:" or "login:" prompt, then the password at "Password:". Each is
/// sent once; a prompt seen again (a wrong password, say) is left to the user.
pub struct AutoLogin {
    username: Option<String>,
    password: Option<String>,
    /// The last bit of output, to find a prompt split across reads.
    tail: String,
    done: bool,
}

/// What [`AutoLogin::feed`] wants typed.
#[derive(Debug, PartialEq, Eq)]
pub enum LoginStep {
    Username(String),
    Password(String),
}

impl AutoLogin {
    pub fn new(username: Option<String>, password: Option<String>) -> Self {
        let username = username.filter(|u| !u.is_empty());
        let password = password.filter(|p| !p.is_empty());
        let done = username.is_none() && password.is_none();
        Self {
            username,
            password,
            tail: String::new(),
            done,
        }
    }

    pub fn active(&self) -> bool {
        !self.done
    }

    /// Stop looking (the time for a login prompt is over).
    pub fn stop(&mut self) {
        self.done = true;
        self.password = None;
    }

    /// Read terminal output; returns what to type, if a prompt just appeared.
    pub fn feed(&mut self, output: &[u8]) -> Option<LoginStep> {
        if self.done {
            return None;
        }
        self.tail.push_str(&String::from_utf8_lossy(output));
        if self.tail.len() > 256 {
            let cut = self.tail.len() - 256;
            let cut = (cut..self.tail.len())
                .find(|i| self.tail.is_char_boundary(*i))
                .unwrap_or(0);
            self.tail.drain(..cut);
        }
        let prompt = last_line(&self.tail).to_ascii_lowercase();
        let word = prompt
            .strip_suffix(':')
            .or_else(|| prompt.strip_suffix('>'))
            .map(str::trim_end)?;
        let asks_user = ["login", "username", "user name", "user"]
            .iter()
            .any(|w| word.ends_with(w));
        let asks_password = ["password", "passcode"].iter().any(|w| word.ends_with(w));
        if asks_user {
            self.tail.clear();
            match self.username.take() {
                Some(u) => Some(LoginStep::Username(u)),
                None => {
                    // Asked for a user name we do not have, or again after
                    // the one sent: the user takes over.
                    self.stop();
                    None
                }
            }
        } else if asks_password {
            self.tail.clear();
            let step = self.password.take().map(LoginStep::Password);
            self.stop();
            step
        } else {
            None
        }
    }
}

/// The text after the last line break, without escape sequences and
/// surrounding blanks.
fn last_line(s: &str) -> String {
    let line = s.rsplit(['\n', '\r']).next().unwrap_or("");
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ params final; other escapes: ESC and one char.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec(mode: TelnetMode) -> TelnetCodec {
        TelnetCodec::new(
            TelnetOptions {
                mode,
                ..Default::default()
            },
            80,
            24,
        )
    }

    #[test]
    fn active_mode_asks_for_options_and_passive_does_not() {
        let mut c = codec(TelnetMode::Active);
        let start = c.start();
        assert!(start.windows(3).any(|w| w == [IAC, WILL, OPT_NAWS]));
        assert!(start.windows(3).any(|w| w == [IAC, DO, OPT_ECHO]));
        assert!(codec(TelnetMode::Passive).start().is_empty());
        assert!(codec(TelnetMode::Raw).start().is_empty());
    }

    #[test]
    fn plain_text_passes_through() {
        let mut c = codec(TelnetMode::Active);
        let r = c.receive(b"Switch>\r\n");
        assert_eq!(r.data, b"Switch>\r\n");
        assert!(r.reply.is_empty());
    }

    #[test]
    fn escaped_iac_is_data_and_cr_nul_is_cr() {
        let mut c = codec(TelnetMode::Passive);
        let r = c.receive(&[b'a', IAC, IAC, b'b', b'\r', 0, b'c']);
        assert_eq!(r.data, vec![b'a', IAC, b'b', b'\r', b'c']);
    }

    #[test]
    fn sequences_split_across_reads() {
        let mut c = codec(TelnetMode::Passive);
        let a = c.receive(&[b'x', IAC]);
        let b = c.receive(&[DO]);
        let d = c.receive(&[OPT_TTYPE, b'y']);
        assert_eq!([a.data, b.data, d.data].concat(), b"xy");
        assert_eq!(d.reply, vec![IAC, WILL, OPT_TTYPE]);
    }

    #[test]
    fn server_echo_accepted_once() {
        let mut c = codec(TelnetMode::Passive);
        assert_eq!(
            c.receive(&[IAC, WILL, OPT_ECHO]).reply,
            vec![IAC, DO, OPT_ECHO]
        );
        assert!(c.server_echoes());
        // Said again: no answer, so no loop.
        assert!(c.receive(&[IAC, WILL, OPT_ECHO]).reply.is_empty());
        assert_eq!(
            c.receive(&[IAC, WONT, OPT_ECHO]).reply,
            vec![IAC, DONT, OPT_ECHO]
        );
        assert!(!c.server_echoes());
    }

    #[test]
    fn answer_to_our_own_request_is_not_answered() {
        let mut c = codec(TelnetMode::Active);
        c.start();
        assert!(c.receive(&[IAC, WILL, OPT_ECHO]).reply.is_empty());
        assert!(c.server_echoes());
    }

    #[test]
    fn unknown_options_are_refused() {
        let mut c = codec(TelnetMode::Passive);
        assert_eq!(c.receive(&[IAC, DO, 39]).reply, vec![IAC, WONT, 39]);
        assert_eq!(c.receive(&[IAC, WILL, 5]).reply, vec![IAC, DONT, 5]);
    }

    #[test]
    fn window_size_sent_when_asked_and_on_resize() {
        let mut c = codec(TelnetMode::Passive);
        assert!(c.resize(100, 30).is_empty(), "not asked yet");
        let r = c.receive(&[IAC, DO, OPT_NAWS]);
        assert_eq!(
            r.reply,
            vec![IAC, WILL, OPT_NAWS, IAC, SB, OPT_NAWS, 0, 100, 0, 30, IAC, SE]
        );
        assert_eq!(
            c.resize(255, 40),
            vec![IAC, SB, OPT_NAWS, 0, 255, 255, 0, 40, IAC, SE]
        );
        assert!(c.resize(255, 40).is_empty(), "same size");
    }

    #[test]
    fn terminal_type_cycles_then_repeats() {
        let mut c = codec(TelnetMode::Active);
        c.start();
        c.receive(&[IAC, DO, OPT_TTYPE]);
        let ask = [IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE];
        let mut names = Vec::new();
        for _ in 0..4 {
            let r = c.receive(&ask).reply;
            names.push(String::from_utf8_lossy(&r[4..r.len() - 2]).to_string());
        }
        assert_eq!(names, ["XTERM-256COLOR", "XTERM", "VT100", "VT100"]);
    }

    #[test]
    fn subnegotiation_does_not_leak_into_output() {
        let mut c = codec(TelnetMode::Passive);
        let r = c.receive(&[b'a', IAC, SB, 99, 1, 2, IAC, IAC, 3, IAC, SE, b'b']);
        assert_eq!(r.data, b"ab");
    }

    #[test]
    fn enter_and_backspace_per_settings() {
        let opts = |newline, backspace| TelnetOptions {
            mode: TelnetMode::Active,
            newline,
            backspace,
        };
        let c = TelnetCodec::new(opts(TelnetNewline::Crlf, TelnetBackspace::Del), 80, 24);
        assert_eq!(c.encode(b"ls\r"), b"ls\r\n");
        assert_eq!(c.encode(&[0x7f, IAC]), vec![0x7f, IAC, IAC]);
        let c = TelnetCodec::new(opts(TelnetNewline::Cr, TelnetBackspace::CtrlH), 80, 24);
        assert_eq!(c.encode(b"x\r"), vec![b'x', b'\r', 0]);
        assert_eq!(c.encode(&[0x7f]), vec![0x08]);
        let c = TelnetCodec::new(opts(TelnetNewline::Lf, TelnetBackspace::Del), 80, 24);
        assert_eq!(c.encode(b"\r"), b"\n");
    }

    #[test]
    fn binary_data_keeps_its_bytes() {
        // ZMODEM data: only Telnet's own escaping, never the Enter or
        // Backspace settings.
        let opts = TelnetOptions { mode: TelnetMode::Active, newline: TelnetNewline::Crlf, backspace: TelnetBackspace::CtrlH };
        let c = TelnetCodec::new(opts, 80, 24);
        assert_eq!(c.encode_raw(&[b'\r', 0x7f, IAC, b'\n']), vec![b'\r', 0, 0x7f, IAC, IAC, b'\n']);
        let raw = TelnetCodec::new(TelnetOptions { mode: TelnetMode::Raw, ..opts }, 80, 24);
        assert_eq!(raw.encode_raw(&[b'\r', 0x7f, IAC]), vec![b'\r', 0x7f, IAC]);
    }

    #[test]
    fn raw_mode_touches_nothing_but_enter() {
        let mut c = TelnetCodec::new(
            TelnetOptions {
                mode: TelnetMode::Raw,
                newline: TelnetNewline::Cr,
                ..Default::default()
            },
            80,
            24,
        );
        let r = c.receive(&[IAC, DO, OPT_NAWS, b'\r', 0]);
        assert_eq!(r.data, vec![IAC, DO, OPT_NAWS, b'\r', 0]);
        assert!(r.reply.is_empty());
        assert_eq!(c.encode(&[IAC, b'\r']), vec![IAC, b'\r']);
    }

    #[test]
    fn auto_login_types_user_then_password_once() {
        let mut a = AutoLogin::new(Some("admin".into()), Some("pw".into()));
        assert_eq!(a.feed(b"\r\nUser Access Verification\r\n\r\nUser"), None);
        assert_eq!(a.feed(b"name: "), Some(LoginStep::Username("admin".into())));
        assert_eq!(
            a.feed(b"admin\r\nPassword: "),
            Some(LoginStep::Password("pw".into()))
        );
        assert!(!a.active());
        assert_eq!(a.feed(b"\r\n% Login invalid\r\nUsername: "), None);
    }

    #[test]
    fn auto_login_password_only_and_escapes() {
        let mut a = AutoLogin::new(None, Some("pw".into()));
        assert_eq!(
            a.feed(b"\x1b[1mPassword:\x1b[0m "),
            Some(LoginStep::Password("pw".into()))
        );
        let mut a = AutoLogin::new(None, Some("pw".into()));
        assert_eq!(a.feed(b"login: "), None, "no user name to give");
        assert!(!a.active());
        assert!(!AutoLogin::new(None, None).active());
    }

    #[test]
    fn auto_login_ignores_text_that_only_mentions_login() {
        let mut a = AutoLogin::new(Some("u".into()), None);
        assert_eq!(a.feed(b"Last login: Tue from 10.0.0.1\r\n"), None);
        assert_eq!(
            a.feed(b"router login: "),
            Some(LoginStep::Username("u".into()))
        );
    }

    #[test]
    fn a_proxmox_console_frames_what_is_typed() {
        let opts = TelnetOptions { mode: TelnetMode::Proxmox, newline: TelnetNewline::Crlf, backspace: TelnetBackspace::CtrlH };
        let mut c = TelnetCodec::new(opts, 100, 30);
        assert_eq!(c.start(), b"1:100:30:");
        assert_eq!(c.encode(b"ls\r\x7f"), b"0:4:ls\r\x7f");
        assert_eq!(c.encode_raw(&[0xff]), [b"0:1:".as_slice(), &[0xff]].concat());
        assert_eq!(c.receive(&[0xff, b'a']).data, vec![0xff, b'a']);
        assert_eq!(c.resize(120, 40), b"1:120:40:");
        assert!(c.resize(120, 40).is_empty());
    }
}
