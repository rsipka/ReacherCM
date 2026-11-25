//! The websocket a Proxmox console runs over (`vncwebsocket`), as a plain
//! byte stream: what the VNC client and the terminal read and write.
//!
//! The socket is opened here with the session's pinned TLS and the console
//! login's cookie, and kept alive with a ping every 30 seconds. Each write
//! goes out as one binary message; pveproxy hands the bytes to the console
//! process behind it as they come.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::client::{url_host, Client, ProxmoxError, Ticket};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_EVERY: Duration = Duration::from_secs(30);
/// Messages from the server that may wait for the reader.
const QUEUE: usize = 256;

/// How the link keeps itself alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keepalive {
    /// A websocket ping (VNC).
    Ping,
    /// termproxy's own ping message, `2`.
    Termproxy,
}

enum Out {
    Data(Vec<u8>),
    Close,
}

/// A console's websocket as a byte stream.
pub struct WsLink {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
    tx: mpsc::UnboundedSender<Out>,
}

impl AsyncRead for WsLink {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        loop {
            if self.pos < self.buf.len() {
                let n = (self.buf.len() - self.pos).min(out.remaining());
                let start = self.pos;
                out.put_slice(&self.buf[start..start + n]);
                self.pos += n;
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(data)) => {
                    self.buf = data;
                    self.pos = 0;
                }
                // The socket closed: the end of the stream.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for WsLink {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.tx.send(Out::Data(data.to_vec())) {
            Ok(()) => Poll::Ready(Ok(data.len())),
            Err(_) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "The console connection closed"))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let _ = self.tx.send(Out::Close);
        Poll::Ready(Ok(()))
    }
}

impl Drop for WsLink {
    fn drop(&mut self) {
        let _ = self.tx.send(Out::Close);
    }
}

/// The websocket URL of a console: `path` is an API path ending in
/// `/vncwebsocket`, `port` and `ticket` what `vncproxy`/`termproxy` gave.
pub fn socket_url(base: &str, path: &str, port: u16, ticket: &str) -> Result<url::Url, String> {
    let wss = base.replacen("https://", "wss://", 1);
    let mut url = url::Url::parse(&format!("{wss}{path}")).map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("port", &port.to_string()).append_pair("vncticket", ticket);
    Ok(url)
}

/// Open a console's websocket with the console login.
pub async fn open(client: &Client, login: &Ticket, path: &str, port: u16, ticket: &str, keepalive: Keepalive) -> Result<WsLink, ProxmoxError> {
    if !super::client::valid_path(path) || !path.ends_with("/vncwebsocket") {
        return Err(ProxmoxError::Other(format!("Not a console path: {path}")));
    }
    let url = socket_url(client.base(), path, port, ticket).map_err(ProxmoxError::Other)?;
    let (config, refused) = crate::ftp::tls::pinning_config(client.trusted()).map_err(ProxmoxError::Other)?;
    let host = client.host.clone();
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect((host.as_str(), client.port)))
        .await
        .map_err(|_| ProxmoxError::Other(format!("{}:{} did not answer in time", url_host(&host), client.port)))?
        .map_err(|e| ProxmoxError::Other(format!("Could not reach {}:{}: {e}", url_host(&host), client.port)))?;
    let _ = tcp.set_nodelay(true);
    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| ProxmoxError::Other(format!("{host}: {e}")))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| {
        match refused.lock().ok().and_then(|mut r| r.take()) {
            Some(fp) => ProxmoxError::UntrustedCert(fp),
            None => ProxmoxError::Other(format!("TLS: {e}")),
        }
    })?;
    let mut req = url.as_str().into_client_request().map_err(|e| ProxmoxError::Other(e.to_string()))?;
    let cookie = format!("{}={}", if client.product == super::client::Product::Pbs { "PBSAuthCookie" } else { "PVEAuthCookie" }, login.ticket);
    let headers = req.headers_mut();
    headers.insert("Cookie", cookie.parse().map_err(|_| ProxmoxError::Other("Bad login ticket".into()))?);
    // Proxmox's websocket proxy speaks the "binary" subprotocol only.
    headers.insert("Sec-WebSocket-Protocol", "binary".parse().expect("static header"));
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::client_async(req, tls))
        .await
        .map_err(|_| ProxmoxError::Other("The console did not answer in time".into()))?
        .map_err(|e| match e {
            tokio_tungstenite::tungstenite::Error::Http(r) => {
                let code = r.status().as_u16();
                if code == 401 {
                    ProxmoxError::Login("The console login was refused".into())
                } else {
                    ProxmoxError::Status { code, message: "The console could not be opened".into() }
                }
            }
            other => ProxmoxError::Other(format!("Console: {other}")),
        })?;

    let (to_reader, rx) = mpsc::channel(QUEUE);
    let (tx, mut from_writer) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut sink, mut stream) = ws.split();
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                msg = stream.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if to_reader.send(b.to_vec()).await.is_err() { break; }
                    }
                    Some(Ok(Message::Text(t))) => {
                        if to_reader.send(t.as_bytes().to_vec()).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::info!("Proxmox console websocket: {e}");
                        break;
                    }
                },
                out = from_writer.recv() => match out {
                    Some(Out::Data(d)) => {
                        if sink.send(Message::binary(d)).await.is_err() { break; }
                    }
                    Some(Out::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                },
                _ = ping.tick() => {
                    let m = match keepalive {
                        Keepalive::Ping => Message::Ping(Vec::new().into()),
                        Keepalive::Termproxy => Message::binary(b"2".to_vec()),
                    };
                    if sink.send(m).await.is_err() { break; }
                }
            }
        }
    });
    Ok(WsLink { rx, buf: Vec::new(), pos: 0, tx })
}

/// termproxy's login on its own socket: `user:ticket` and a newline, which
/// it answers with `OK`.
pub async fn termproxy_login(link: &mut WsLink, user: &str, ticket: &str) -> Result<(), ProxmoxError> {
    link.write_all(format!("{user}:{ticket}\n").as_bytes())
        .await
        .map_err(|e| ProxmoxError::Other(format!("Console: {e}")))?;
    let mut ok = [0u8; 2];
    tokio::time::timeout(CONNECT_TIMEOUT, link.read_exact(&mut ok))
        .await
        .map_err(|_| ProxmoxError::Other("The console did not answer in time".into()))?
        .map_err(|_| ProxmoxError::Login("The console refused the login".into()))?;
    if &ok != b"OK" {
        return Err(ProxmoxError::Login("The console refused the login".into()));
    }
    Ok(())
}

/// What the terminal types, as termproxy takes it: `0:<length>:<bytes>`.
pub fn term_input(data: &[u8]) -> Vec<u8> {
    let mut out = format!("0:{}:", data.len()).into_bytes();
    out.extend_from_slice(data);
    out
}

/// A new terminal size, as termproxy takes it: `1:<cols>:<rows>:`.
pub fn term_resize(cols: u16, rows: u16) -> Vec<u8> {
    format!("1:{cols}:{rows}:").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termproxy_framing() {
        assert_eq!(term_input(b"ls\r"), b"0:3:ls\r");
        assert_eq!(term_input("č".as_bytes()), [b"0:2:".as_slice(), "č".as_bytes()].concat());
        assert_eq!(term_resize(120, 40), b"1:120:40:");
    }

    #[test]
    fn socket_urls_carry_the_ticket_encoded() {
        let u = socket_url("https://pve.example.com:8006", "/api2/json/nodes/pve1/qemu/100/vncwebsocket", 5900, "PVEVNC:ab+c/d=").unwrap();
        assert_eq!(u.scheme(), "wss");
        assert_eq!(u.path(), "/api2/json/nodes/pve1/qemu/100/vncwebsocket");
        assert_eq!(u.query(), Some("port=5900&vncticket=PVEVNC%3Aab%2Bc%2Fd%3D"));
        let v6 = socket_url("https://[fd00::1]:8006", "/api2/json/nodes/n/vncwebsocket", 5901, "t").unwrap();
        assert_eq!(v6.host_str(), Some("[fd00::1]"));
    }

    #[tokio::test]
    async fn the_link_reads_messages_as_one_stream() {
        let (to_reader, rx) = mpsc::channel(4);
        let (tx, mut out) = mpsc::unbounded_channel();
        let mut link = WsLink { rx, buf: Vec::new(), pos: 0, tx };
        to_reader.send(b"OKhel".to_vec()).await.unwrap();
        to_reader.send(b"lo".to_vec()).await.unwrap();
        drop(to_reader);
        let mut ok = [0u8; 2];
        link.read_exact(&mut ok).await.unwrap();
        assert_eq!(&ok, b"OK");
        let mut rest = Vec::new();
        link.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"hello");
        link.write_all(b"0:1:a").await.unwrap();
        assert!(matches!(out.recv().await, Some(Out::Data(d)) if d == b"0:1:a"));
    }
}
