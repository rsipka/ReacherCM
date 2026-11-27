//! VeNCrypt against a fake server that does the server's half: over an
//! in-memory pipe for the steps before and after TLS, and over real TCP and
//! TLS for the whole of it.

use super::*;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::io::duplex;

const CERT: &[u8] = include_bytes!("../proxmox/testdata/cert.der");
const KEY: &[u8] = include_bytes!("../proxmox/testdata/key.der");
const FINGERPRINT: &str = "D0:D6:4D:85:26:A4:69:2B:1C:29:BA:7A:D8:C4:0A:E7:82:43:88:F1:DB:CB:00:EE:D3:F9:B6:0C:0B:0C:F3:67";

/// The server's half up to TLS: offers `types` and `subtypes`; returns the
/// subtype the client chose.
async fn fake_server_before_tls<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, types: &[u8], subtypes: &[u32]) -> u32 {
    s.write_all(RFB_38).await.unwrap();
    let mut version = [0u8; 12];
    s.read_exact(&mut version).await.unwrap();
    assert_eq!(&version, RFB_38);
    s.write_all(&[types.len() as u8]).await.unwrap();
    s.write_all(types).await.unwrap();
    assert_eq!(s.read_u8().await.unwrap(), VENCRYPT);
    s.write_all(&[0, 2]).await.unwrap();
    let mut theirs = [0u8; 2];
    s.read_exact(&mut theirs).await.unwrap();
    assert_eq!(theirs, [0, 2]);
    s.write_all(&[0, subtypes.len() as u8]).await.unwrap();
    for t in subtypes {
        s.write_all(&t.to_be_bytes()).await.unwrap();
    }
    let chosen = s.read_u32().await.unwrap();
    s.write_all(&[1]).await.unwrap();
    chosen
}

#[test]
fn versions_from_3_8_on() {
    assert!(at_least_38(b"RFB 003.008\n"));
    assert!(at_least_38(b"RFB 003.889\n"));
    assert!(at_least_38(b"RFB 004.001\n"));
    assert!(!at_least_38(b"RFB 003.007\n"));
    assert!(!at_least_38(b"RFB 003.003\n"));
    assert!(!at_least_38(b"HTTP/1.1 400"));
}

#[test]
fn subtypes_in_the_servers_order() {
    assert_eq!(choose(&[X509_VNC, X509_NONE], false), Ok(X509_VNC));
    assert_eq!(choose(&[TLS_VNC, X509_NONE, X509_VNC], false), Ok(X509_NONE));
    assert_eq!(choose(&[X509_PLAIN, X509_VNC], true), Ok(X509_PLAIN));
    // Without a user name, Plain is passed over.
    assert_eq!(choose(&[X509_PLAIN, X509_VNC], false), Ok(X509_VNC));
}

#[test]
fn what_cannot_be_used_says_why() {
    assert!(choose(&[X509_PLAIN], false).unwrap_err().contains("user name"));
    assert!(choose(&[TLS_NONE, TLS_VNC], true).unwrap_err().contains("anonymous TLS"));
    assert!(choose(&[263, 264], true).unwrap_err().contains("[263, 264]"));
}

#[test]
fn plain_login_message() {
    let msg = plain_message("ann", "pw").unwrap();
    assert_eq!(&msg[..], &[0, 0, 0, 3, 0, 0, 0, 2, b'a', b'n', b'n', b'p', b'w']);
}

#[tokio::test]
async fn agrees_on_a_subtype() {
    let (mut client, mut server) = duplex(4096);
    let srv = tokio::spawn(async move { fake_server_before_tls(&mut server, &[2, 19], &[TLS_VNC, X509_VNC]).await });
    assert_eq!(before_tls(&mut client, false).await, Ok(X509_VNC));
    assert_eq!(srv.await.unwrap(), X509_VNC);
}

#[tokio::test]
async fn a_server_without_vencrypt_is_refused() {
    let (mut client, mut server) = duplex(4096);
    tokio::spawn(async move {
        server.write_all(RFB_38).await.unwrap();
        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        server.write_all(&[2, 1, 2]).await.unwrap();
        // Held open until the client gives up.
        let _ = server.read_u8().await;
    });
    let err = before_tls(&mut client, false).await.unwrap_err();
    assert!(err.contains("does not offer VeNCrypt"), "{err}");
}

#[tokio::test]
async fn x509_vnc_hands_the_password_to_vnc_rs() {
    let (mut client, _server) = duplex(64);
    assert_eq!(after_tls(&mut client, X509_VNC, "", "pw").await, Ok(apple::hand_over(VNC_AUTH)));
}

async fn after_plain(result: &[u8]) -> (Result<Prelude, String>, Vec<u8>) {
    let (mut client, mut server) = duplex(4096);
    let result = result.to_vec();
    let srv = tokio::spawn(async move {
        let ulen = server.read_u32().await.unwrap() as usize;
        let plen = server.read_u32().await.unwrap() as usize;
        let mut creds = vec![0u8; ulen + plen];
        server.read_exact(&mut creds).await.unwrap();
        server.write_all(&result).await.unwrap();
        creds
    });
    let out = after_tls(&mut client, X509_PLAIN, "ann", "secret").await;
    (out, srv.await.unwrap())
}

#[tokio::test]
async fn x509_plain_logs_in() {
    let (out, creds) = after_plain(&[0, 0, 0, 0]).await;
    assert_eq!(out, apple::logged_in());
    assert_eq!(creds, b"annsecret");
}

#[tokio::test]
async fn x509_plain_wrong_password_with_reason() {
    let (out, _) = after_plain(&[0, 0, 0, 1, 0, 0, 0, 6, b'd', b'e', b'n', b'i', b'e', b'd']).await;
    assert_eq!(out.unwrap_err(), "Wrong VNC user name or password: denied");
}

// ---------------------------------------------------------------------------
// The whole of it, over TCP and TLS.

/// A VeNCrypt server offering X509Plain; returns its port and what it was
/// sent as the login.
async fn tls_server() -> (u16, tokio::task::JoinHandle<Option<Vec<u8>>>) {
    let certs = vec![CertificateDer::from(CERT.to_vec())];
    let key = PrivatePkcs8KeyDer::from(KEY.to_vec()).into();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        assert_eq!(fake_server_before_tls(&mut tcp, &[19], &[X509_PLAIN]).await, X509_PLAIN);
        let mut tls = acceptor.accept(tcp).await.ok()?;
        let ulen = tls.read_u32().await.unwrap() as usize;
        let plen = tls.read_u32().await.unwrap() as usize;
        let mut creds = vec![0u8; ulen + plen];
        tls.read_exact(&mut creds).await.unwrap();
        tls.write_all(&[0, 0, 0, 0]).await.unwrap();
        tls.flush().await.unwrap();
        Some(creds)
    });
    (port, task)
}

async fn connect(port: u16, trusted: &[String]) -> Result<(TlsStream<TcpStream>, Prelude, Security), Error> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let tls = crate::ftp::tls::Tls::new("127.0.0.1", trusted).unwrap();
    negotiate(tcp, &tls, "ann", "secret").await
}

#[tokio::test]
async fn an_unknown_certificate_is_asked_about() {
    let (port, server) = tls_server().await;
    match connect(port, &[]).await {
        Err(Error::Untrusted(fp)) => assert_eq!(fp, FINGERPRINT),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(server.await.unwrap(), None);
}

#[tokio::test]
async fn a_trusted_certificate_logs_in() {
    let (port, server) = tls_server().await;
    let (_stream, prelude, security) = connect(port, &[FINGERPRINT.to_string()]).await.unwrap();
    assert_eq!(prelude, apple::logged_in().unwrap());
    assert_eq!(security.kind, "vencrypt");
    assert!(security.protocol.is_some());
    assert_eq!(server.await.unwrap().as_deref(), Some(&b"annsecret"[..]));
}
