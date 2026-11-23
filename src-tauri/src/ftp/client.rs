//! One FTP control connection (RFC 959), with FTPS (RFC 4217) explicit or
//! implicit, passive data connections (EPSV, else PASV) and the RFC 3659
//! extras where the server has them (MLSD, SIZE, UTF8).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::tls::Tls;

/// How long the server may take to answer a command.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REPLY_LINE: usize = 64 * 1024;
const MAX_REPLY_BYTES: usize = 256 * 1024;
const MAX_LISTING_BYTES: u64 = 16 * 1024 * 1024;
/// How long connecting, and each TLS handshake, may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A data connection with no bytes for this long is given up.
pub const DATA_IDLE: Duration = Duration::from_secs(60);

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

#[derive(Debug)]
pub enum FtpError {
    /// The connection itself failed or closed: worth one more try on a new one.
    Io(String),
    /// The server answered no.
    Reply { code: u16, text: String },
    /// TLS failed; with the fingerprint of a certificate nothing trusts.
    Tls { message: String, untrusted: Option<String> },
    Cancelled,
    Other(String),
}

impl FtpError {
    pub fn is_connection(&self) -> bool {
        matches!(self, FtpError::Io(_)) || matches!(self, FtpError::Reply { code: 421, .. })
    }
}

impl std::fmt::Display for FtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FtpError::Io(m) | FtpError::Other(m) => write!(f, "{m}"),
            FtpError::Reply { code, text } => write!(f, "{text} ({code})"),
            FtpError::Tls { message, .. } => write!(f, "{message}"),
            FtpError::Cancelled => write!(f, "Transfer cancelled"),
        }
    }
}

fn io(e: impl std::fmt::Display) -> FtpError {
    FtpError::Io(format!("FTP connection failed: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub code: u16,
    pub text: String,
}

impl Reply {
    fn ok(&self, codes: &[u16]) -> Result<(), FtpError> {
        if codes.contains(&self.code) {
            Ok(())
        } else {
            Err(FtpError::Reply { code: self.code, text: self.text.clone() })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Security {
    /// Plain FTP: the password goes unencrypted.
    None,
    /// AUTH TLS on the normal port; refused if the server cannot.
    #[default]
    Explicit,
    /// TLS from the first byte (port 990).
    Implicit,
}

impl Security {
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("none") => Security::None,
            Some("implicit") => Security::Implicit,
            _ => Security::Explicit,
        }
    }
}

/// What one connects with.
#[derive(Clone)]
pub struct Params {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub security: Security,
    /// Fingerprints of certificates the session trusts.
    pub trusted: Vec<String>,
}

impl std::fmt::Debug for Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Params").field("host", &self.host).field("port", &self.port).field("security", &self.security).finish()
    }
}

/// Read a reply: one line `123 text`, or several from `123-` to `123 `.
pub async fn read_reply<S: AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> Result<Reply, FtpError> {
    tokio::time::timeout(REPLY_TIMEOUT, read_reply_inner(s, buf))
        .await
        .map_err(|_| FtpError::Io("The FTP server did not complete its reply in time".into()))?
}

async fn read_reply_inner<S: AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> Result<Reply, FtpError> {
    let first = read_line(s, buf).await?;
    let code = reply_code(&first).ok_or_else(|| FtpError::Io(format!("Not an FTP server: \"{}\"", first.trim())))?;
    let mut text = first.get(4..).unwrap_or("").to_string();
    if first.as_bytes().get(3) == Some(&b'-') {
        let end_code = code.to_string();
        let end = format!("{end_code} ");
        loop {
            let line = read_line(s, buf).await?;
            if text.len() + line.len() + 1 > MAX_REPLY_BYTES {
                return Err(FtpError::Io("The FTP server sent a reply that exceeds the size limit".into()));
            }
            if line.starts_with(&end) || line == end_code {
                text.push('\n');
                text.push_str(line.get(4..).unwrap_or(""));
                break;
            }
            text.push('\n');
            text.push_str(&line);
        }
    }
    Ok(Reply { code, text })
}

fn reply_code(line: &str) -> Option<u16> {
    let code = line.get(..3)?;
    if !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match line.as_bytes().get(3) {
        None | Some(b' ') | Some(b'-') => code.parse().ok(),
        _ => None,
    }
}

async fn read_line<S: AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> Result<String, FtpError> {
    loop {
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            if i > MAX_REPLY_LINE {
                return Err(FtpError::Io("The server sent a line too long to be FTP".into()));
            }
            let line: Vec<u8> = buf.drain(..=i).collect();
            let text = String::from_utf8_lossy(&line);
            return Ok(text.trim_end_matches(['\r', '\n']).to_string());
        }
        if buf.len() > MAX_REPLY_LINE {
            return Err(FtpError::Io("The server sent a line too long to be FTP".into()));
        }
        let mut chunk = [0u8; 4096];
        let n = s.read(&mut chunk).await.map_err(io)?;
        if n == 0 {
            return Err(FtpError::Io("The FTP server closed the connection".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// The port of an EPSV reply: `229 Entering Extended Passive Mode (|||6446|)`.
pub fn parse_epsv(text: &str) -> Option<u16> {
    let start = text.find('(')?;
    let end = text[start..].find(')')? + start;
    let inner = &text[start + 1..end];
    let d = inner.chars().next()?;
    let parts: Vec<&str> = inner.split(d).collect();
    parts.get(3)?.parse().ok()
}

/// The address of a PASV reply: `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)`.
pub fn parse_pasv(text: &str) -> Option<(IpAddr, u16)> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let nums: Vec<u16> = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(6)
        .map(|s| s.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if nums.len() != 6 || nums.iter().any(|&n| n > 255) {
        return None;
    }
    let ip = IpAddr::from([nums[0] as u8, nums[1] as u8, nums[2] as u8, nums[3] as u8]);
    Some((ip, nums[4] * 256 + nums[5]))
}

/// The folder of a PWD reply: `257 "/home/a ""b""" is current`.
pub fn parse_pwd(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let mut out = String::new();
    let mut chars = text[start..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                out.push('"');
            } else {
                return Some(out);
            }
        } else {
            out.push(c);
        }
    }
    None
}

fn validate_command(line: &str) -> Result<(), FtpError> {
    if line.contains(['\r', '\n', '\0']) {
        return Err(FtpError::Other("An FTP command argument contains a line break or NUL".into()));
    }
    Ok(())
}

pub struct Client {
    ctrl: Box<dyn Stream>,
    buf: Vec<u8>,
    peer: IpAddr,
    tls: Option<Tls>,
    pub mlsd: bool,
    epsv: bool,
    pub home: String,
    /// Set once something went wrong mid-command: not put back for reuse.
    pub broken: bool,
}

async fn tls_wrap(tls: &Tls, tcp: TcpStream) -> Result<Box<dyn Stream>, FtpError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, tls.connector.connect(tls.name.clone(), tcp)).await {
        Ok(Ok(s)) => Ok(Box::new(s)),
        Ok(Err(e)) => Err(FtpError::Tls { message: format!("TLS failed: {e}"), untrusted: tls.refused() }),
        Err(_) => Err(FtpError::Tls { message: "TLS handshake timed out".into(), untrusted: None }),
    }
}

impl Client {
    pub async fn connect(p: &Params) -> Result<Self, FtpError> {
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((p.host.trim_matches(['[', ']']), p.port)))
            .await
            .map_err(|_| FtpError::Io(format!("{}:{}: no answer", p.host, p.port)))?
            .map_err(|e| FtpError::Io(format!("{}:{}: {e}", p.host, p.port)))?;
        let _ = tcp.set_nodelay(true);
        let peer = tcp.peer_addr().map_err(io)?.ip();
        let tls = match p.security {
            Security::None => None,
            _ => Some(Tls::new(&p.host, &p.trusted).map_err(FtpError::Other)?),
        };
        let mut buf = Vec::new();
        let ctrl: Box<dyn Stream> = match p.security {
            Security::Implicit => {
                let mut s = tls_wrap(tls.as_ref().expect("TLS set up"), tcp).await?;
                read_reply(&mut s, &mut buf).await?.ok(&[220])?;
                s
            }
            Security::Explicit => {
                let mut tcp = tcp;
                read_reply(&mut tcp, &mut buf).await?.ok(&[220])?;
                tcp.write_all(b"AUTH TLS\r\n").await.map_err(io)?;
                let r = read_reply(&mut tcp, &mut buf).await?;
                if r.code != 234 {
                    return Err(FtpError::Reply {
                        code: r.code,
                        text: format!("The server does not offer TLS: {}. Choose plain FTP in the session to connect without encryption.", r.text.trim().trim_end_matches('.')),
                    });
                }
                tls_wrap(tls.as_ref().expect("TLS set up"), tcp).await?
            }
            Security::None => {
                let mut tcp = tcp;
                read_reply(&mut tcp, &mut buf).await?.ok(&[220])?;
                Box::new(tcp)
            }
        };
        let mut c = Client { ctrl, buf, peer, tls, mlsd: false, epsv: true, home: "/".into(), broken: false };
        c.login(p).await?;
        Ok(c)
    }

    async fn login(&mut self, p: &Params) -> Result<(), FtpError> {
        let (user, pass) = if p.username.trim().is_empty() {
            ("anonymous".to_string(), if p.password.is_empty() { "anonymous@".to_string() } else { p.password.clone() })
        } else {
            (p.username.clone(), p.password.clone())
        };
        let r = self.cmd(&format!("USER {user}")).await?;
        let r = if r.code == 331 { self.cmd_secret("PASS", &pass).await? } else { r };
        if r.code == 332 {
            return Err(FtpError::Other("The server asks for an account (ACCT), which is not supported".into()));
        }
        r.ok(&[230, 202])?;
        if self.tls.is_some() {
            self.ok("PBSZ 0", &[200]).await?;
            self.ok("PROT P", &[200]).await.map_err(|e| {
                FtpError::Other(format!("The server refused encrypted FTP data connections: {e}"))
            })?;
        }
        let feat = self.cmd("FEAT").await?;
        if feat.code == 211 {
            let upper = feat.text.to_ascii_uppercase();
            let has = |f: &str| upper.lines().any(|l| l.trim_start().starts_with(f));
            self.mlsd = has("MLST");
            if has("UTF8") {
                let _ = self.cmd("OPTS UTF8 ON").await?;
            }
        }
        self.cmd("TYPE I").await?.ok(&[200])?;
        let pwd = self.cmd("PWD").await?;
        if pwd.code == 257 {
            if let Some(home) = parse_pwd(&pwd.text).filter(|h| h.starts_with('/')) {
                self.home = home;
            }
        }
        Ok(())
    }

    async fn send(&mut self, line: &str) -> Result<(), FtpError> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.extend_from_slice(b"\r\n");
        let sent = async {
            self.ctrl.write_all(&bytes).await?;
            self.ctrl.flush().await
        };
        if let Err(e) = sent.await {
            self.broken = true;
            return Err(io(e));
        }
        Ok(())
    }

    pub async fn reply(&mut self) -> Result<Reply, FtpError> {
        let r = read_reply(&mut self.ctrl, &mut self.buf).await;
        match r {
            Err(e) => {
                self.broken = true;
                Err(e)
            }
            Ok(r) if r.code == 421 => {
                self.broken = true;
                Err(FtpError::Reply { code: r.code, text: r.text })
            }
            Ok(r) => Ok(r),
        }
    }

    pub async fn cmd(&mut self, line: &str) -> Result<Reply, FtpError> {
        validate_command(line)?;
        tracing::debug!("FTP > {line}");
        self.send(line).await?;
        self.reply().await
    }

    async fn cmd_secret(&mut self, verb: &str, secret: &str) -> Result<Reply, FtpError> {
        let line = format!("{verb} {secret}");
        validate_command(&line)?;
        tracing::debug!("FTP > {verb} ****");
        self.send(&line).await?;
        self.reply().await
    }

    /// A command that must be answered with one of `codes`.
    pub async fn ok(&mut self, line: &str, codes: &[u16]) -> Result<Reply, FtpError> {
        let r = self.cmd(line).await?;
        r.ok(codes)?;
        Ok(r)
    }

    async fn passive(&mut self) -> Result<TcpStream, FtpError> {
        let mut addr = None;
        if self.epsv {
            let r = self.cmd("EPSV").await?;
            match (r.code, parse_epsv(&r.text)) {
                (229, Some(port)) => addr = Some(SocketAddr::new(self.peer, port)),
                _ => self.epsv = false,
            }
        }
        let addr = match addr {
            Some(a) => a,
            None => {
                let r = self.ok("PASV", &[227]).await?;
                let (_, port) = parse_pasv(&r.text).ok_or_else(|| FtpError::Other(format!("Cannot read the server's passive address: {}", r.text)))?;
                // The address in the reply is often a private one behind
                // NAT: the server's own, as connected to, is used instead.
                SocketAddr::new(self.peer, port)
            }
        };
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| FtpError::Io(format!("Data connection to port {} timed out", addr.port())))?
            .map_err(|e| FtpError::Io(format!("Data connection to port {} failed: {e}", addr.port())))?;
        let _ = tcp.set_nodelay(true);
        Ok(tcp)
    }

    /// Open a data connection for `line` (RETR, STOR, LIST...). The caller
    /// reads or writes it to the end, drops it, then calls [`finish`].
    pub async fn data(&mut self, line: &str) -> Result<Box<dyn Stream>, FtpError> {
        validate_command(line)?;
        let tcp = self.passive().await?;
        let r = self.cmd(line).await?;
        r.ok(&[125, 150])?;
        if let Some(tls) = &self.tls {
            match tls_wrap(tls, tcp).await {
                Ok(s) => Ok(s),
                Err(e) => {
                    self.broken = true;
                    Err(e)
                }
            }
        } else {
            Ok(Box::new(tcp))
        }
    }

    /// The server's word after a data connection ended.
    pub async fn finish(&mut self) -> Result<(), FtpError> {
        self.reply().await?.ok(&[226, 250])
    }

    /// A whole listing's text.
    pub async fn list_text(&mut self, path: &str) -> Result<(String, bool), FtpError> {
        if self.mlsd {
            match self.data(&format!("MLSD {path}")).await {
                Ok(s) => return Ok((self.read_all(s).await?, true)),
                Err(FtpError::Reply { code, .. }) if code == 500 || code == 501 || code == 502 => self.mlsd = false,
                Err(e) => return Err(e),
            }
        }
        // LIST takes its argument as `ls` options on many servers: go there first.
        self.ok(&format!("CWD {path}"), &[250]).await?;
        let s = self.data("LIST").await?;
        Ok((self.read_all(s).await?, false))
    }

    async fn read_all(&mut self, mut s: Box<dyn Stream>) -> Result<String, FtpError> {
        let mut out = Vec::new();
        let read = tokio::time::timeout(DATA_IDLE, (&mut s).take(MAX_LISTING_BYTES + 1).read_to_end(&mut out)).await;
        drop(s);
        match read {
            Ok(Ok(n)) if n as u64 > MAX_LISTING_BYTES => {
                self.broken = true;
                return Err(FtpError::Other("FTP listing exceeds the size limit".into()));
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                self.broken = true;
                return Err(io(e));
            }
            Err(_) => {
                self.broken = true;
                return Err(FtpError::Io("The listing stopped coming".into()));
            }
        }
        self.finish().await?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    pub async fn quit(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.cmd("QUIT")).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_multiline_reply_has_a_total_size_limit() {
        let line = "x".repeat(MAX_REPLY_LINE / 2);
        let response = format!("211-Status\r\n{}211 End\r\n", format!("{line}\r\n").repeat(9));
        let mut bytes = response.as_bytes();
        let error = read_reply(&mut bytes, &mut Vec::new()).await.unwrap_err();
        assert!(error.to_string().contains("size limit"));
    }

    #[test]
    fn command_validation_errors_do_not_include_arguments() {
        assert!(validate_command("CWD ordinary folder").is_ok());
        let error = validate_command("PASS example\n").unwrap_err().to_string();
        assert!(error.contains("line break"));
        assert!(!error.contains("example"));
    }

    #[tokio::test]
    async fn ftps_requires_protection_before_continuing_login() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (ctrl, peer) = tokio::io::duplex(1024);
        let mut client = Client {
            ctrl: Box::new(ctrl),
            buf: Vec::new(),
            peer: "127.0.0.1".parse().unwrap(),
            tls: Some(Tls::new("localhost", &[]).unwrap()),
            mlsd: false,
            epsv: true,
            home: "/".into(),
            broken: false,
        };
        let params = Params {
            host: "localhost".into(),
            port: 21,
            username: "example".into(),
            password: String::new(),
            security: Security::Explicit,
            trusted: Vec::new(),
        };
        let server = async {
            let mut peer = BufReader::new(peer);
            for (expected, reply) in [
                ("USER example\r\n", "230 Logged in\r\n"),
                ("PBSZ 0\r\n", "200 Buffer size accepted\r\n"),
                ("PROT P\r\n", "536 Private data protection unavailable\r\n"),
            ] {
                let mut line = String::new();
                peer.read_line(&mut line).await.unwrap();
                assert_eq!(line, expected);
                peer.write_all(reply.as_bytes()).await.unwrap();
            }
        };
        let (result, ()) = tokio::join!(client.login(&params), server);
        assert!(matches!(result, Err(FtpError::Other(message)) if message.contains("refused encrypted")));
    }

    #[test]
    fn passive_replies() {
        assert_eq!(parse_epsv("Entering Extended Passive Mode (|||6446|)"), Some(6446));
        assert_eq!(parse_pasv("Entering Passive Mode (192,0,2,10,19,137)."), Some((IpAddr::from([192, 0, 2, 10]), 19 * 256 + 137)));
        assert_eq!(parse_pasv("Entering Passive Mode 192,0,2,10,19,137"), Some((IpAddr::from([192, 0, 2, 10]), 5001)));
        assert_eq!(parse_pasv("nothing"), None);
    }

    #[test]
    fn pwd_replies() {
        assert_eq!(parse_pwd("\"/home/a \"\"b\"\"\" is current directory").as_deref(), Some("/home/a \"b\""));
        assert_eq!(parse_pwd("\"/\"").as_deref(), Some("/"));
        assert_eq!(parse_pwd("no quotes"), None);
    }

    #[tokio::test]
    async fn multi_line_replies() {
        let mut input: &[u8] = b"211-Features:\r\n MLST type*;size*;\r\n UTF8\r\n211 End\r\n220 next\r\n";
        let mut buf = Vec::new();
        let r = read_reply(&mut input, &mut buf).await.unwrap();
        assert_eq!(r.code, 211);
        assert!(r.text.contains("MLST") && r.text.contains("UTF8"));
        let r = read_reply(&mut input, &mut buf).await.unwrap();
        assert_eq!(r, Reply { code: 220, text: "next".into() });
    }
}
