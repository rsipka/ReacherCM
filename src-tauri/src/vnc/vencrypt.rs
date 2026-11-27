//! VeNCrypt: TLS negotiated inside RFB (security type 19).
//!
//! QEMU and libvirt (`vnc_tls`), TigerVNC and wayvnc encrypt this way. The
//! server offers type 19; client and server agree on VeNCrypt 0.2 and a
//! subtype, the connection turns into TLS, and the subtype's login follows
//! inside it:
//!
//! * **X509None** (260): no login.
//! * **X509Vnc** (261): the usual VNC password challenge, which vnc-rs
//!   answers as it always does.
//! * **X509Plain** (262): a user name and password, sent as they are; TLS
//!   is what protects them.
//!
//! The TLS* subtypes (257-259) are left out on purpose: they use anonymous
//! Diffie-Hellman TLS, which rustls does not have (and which anyone on the
//! way can sit in the middle of, as no certificate is checked). A server
//! that offers only those is told to have a certificate. The SASL ones (263,
//! 264) are not supported either.
//!
//! The certificate is checked as for FTPS and web sessions: one the system
//! trusts, or one whose fingerprint the session trusts (see
//! `crate::ftp::tls`). Everything up to the desktop happens here, as for a
//! Mac's login (see [`super::apple`]): vnc-rs is then shown a server that
//! wants no login, or for X509Vnc one that wants a VNC password.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use zeroize::Zeroizing;

use super::apple::{self, io, read_reason, Prelude, RFB_38, VNC_AUTH};
use super::Security;

pub(super) const VENCRYPT: u8 = 19;

pub(super) const TLS_NONE: u32 = 257;
pub(super) const TLS_VNC: u32 = 258;
pub(super) const TLS_PLAIN: u32 = 259;
pub(super) const X509_NONE: u32 = 260;
pub(super) const X509_VNC: u32 = 261;
pub(super) const X509_PLAIN: u32 = 262;

/// Why VeNCrypt failed: a certificate nobody trusts yet (its fingerprint),
/// or anything else (the message).
#[derive(Debug)]
pub(super) enum Error {
    Untrusted(String),
    Other(String),
}

impl From<String> for Error {
    fn from(m: String) -> Self {
        Self::Other(m)
    }
}

/// Do the RFB handshake as far as the desktop over `tcp`, inside TLS from
/// VeNCrypt on. Returns the TLS stream, what vnc-rs is to be shown first
/// (see [`apple::Shim`]) and how the connection is protected.
pub(super) async fn negotiate(
    mut tcp: TcpStream,
    tls: &crate::ftp::tls::Tls,
    username: &str,
    password: &str,
) -> Result<(TlsStream<TcpStream>, Prelude, Security), Error> {
    let subtype = before_tls(&mut tcp, !username.is_empty()).await?;
    let mut stream = match tls.connector.connect(tls.name.clone(), tcp).await {
        Ok(s) => s,
        Err(e) => {
            return Err(match tls.refused() {
                Some(fp) => Error::Untrusted(fp),
                None => Error::Other(format!("TLS with the VNC server failed: {e}")),
            });
        }
    };
    let protected = Security::vencrypt(stream.get_ref().1);
    let prelude = after_tls(&mut stream, subtype, username, password).await?;
    Ok((stream, prelude, protected))
}

/// The RFB version, security type 19 and the VeNCrypt version and subtype:
/// everything before TLS starts. Returns the subtype agreed on.
pub(super) async fn before_tls<S>(stream: &mut S, has_username: bool) -> Result<u32, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut version = [0u8; 12];
    stream.read_exact(&mut version).await.map_err(io)?;
    if !at_least_38(&version) {
        return Err(format!(
            "This VNC server speaks {}, too old for VeNCrypt. Turn encryption off in the session.",
            String::from_utf8_lossy(&version[..11]).trim()
        ));
    }
    stream.write_all(RFB_38).await.map_err(io)?;

    let count = stream.read_u8().await.map_err(io)?;
    if count == 0 {
        let reason = read_reason(stream).await.unwrap_or_default();
        return Err(format!("The server refused the connection: {reason}"));
    }
    let mut offered = vec![0u8; usize::from(count)];
    stream.read_exact(&mut offered).await.map_err(io)?;
    tracing::debug!("VNC: security types offered {offered:?}");
    if !offered.contains(&VENCRYPT) {
        return Err(format!(
            "This VNC server does not offer VeNCrypt (security types {offered:?}). Turn it on at the server, or choose no encryption in the session and connect through SSH."
        ));
    }
    stream.write_all(&[VENCRYPT]).await.map_err(io)?;

    // VeNCrypt's own version: 0.2 is the one every server has. 0.1 used
    // other subtype numbers and is not spoken.
    let mut theirs = [0u8; 2];
    stream.read_exact(&mut theirs).await.map_err(io)?;
    if theirs < [0, 2] {
        return Err(format!("This VNC server speaks VeNCrypt {}.{}, which Reacher CM does not support (0.2 and later only)", theirs[0], theirs[1]));
    }
    stream.write_all(&[0, 2]).await.map_err(io)?;
    if stream.read_u8().await.map_err(io)? != 0 {
        return Err("The VNC server refused VeNCrypt 0.2".to_string());
    }

    let count = stream.read_u8().await.map_err(io)?;
    let mut subtypes = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        subtypes.push(stream.read_u32().await.map_err(io)?);
    }
    tracing::debug!("VNC: VeNCrypt subtypes offered {subtypes:?}");
    let chosen = choose(&subtypes, has_username)?;
    stream.write_all(&chosen.to_be_bytes()).await.map_err(io)?;
    if stream.read_u8().await.map_err(io)? != 1 {
        return Err("The VNC server refused the VeNCrypt login it offered".to_string());
    }
    Ok(chosen)
}

/// `RFB 003.008` or later (`RFB 003.889` included).
fn at_least_38(version: &[u8; 12]) -> bool {
    let text = std::str::from_utf8(version).unwrap_or("");
    let Some(rest) = text.strip_prefix("RFB ") else { return false };
    let mut parts = rest.trim_end().split('.');
    let major = parts.next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
    (major, minor) >= (3, 8)
}

/// Which subtype, in the server's order of preference: the first one
/// Reacher CM speaks. X509Plain needs a user name, so without one it is
/// passed over for another.
pub(super) fn choose(offered: &[u32], has_username: bool) -> Result<u32, String> {
    let usable = |t: u32| match t {
        X509_NONE | X509_VNC => true,
        X509_PLAIN => has_username,
        _ => false,
    };
    if let Some(&t) = offered.iter().find(|&&t| usable(t)) {
        return Ok(t);
    }
    if offered.contains(&X509_PLAIN) {
        return Err("This VNC server logs in with a user name and password (VeNCrypt X509Plain). Enter the user name in the session.".to_string());
    }
    if offered.iter().any(|t| matches!(*t, TLS_NONE | TLS_VNC | TLS_PLAIN)) {
        return Err(
            "This VNC server offers only VeNCrypt without a certificate (anonymous TLS), which cannot be checked and which Reacher CM does not support. \
             Give the server an X.509 certificate (for QEMU and libvirt, tls-creds-x509; for TigerVNC, X509Cert and X509Key), or connect through SSH."
                .to_string(),
        );
    }
    Err(format!("This VNC server offers no VeNCrypt login Reacher CM supports (subtypes {offered:?})"))
}

/// The subtype's login inside TLS. Returns what vnc-rs is to be shown.
pub(super) async fn after_tls<S>(stream: &mut S, subtype: u32, username: &str, password: &str) -> Result<Prelude, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match subtype {
        // vnc-rs answers the password challenge and reads the result.
        X509_VNC => Ok(apple::hand_over(VNC_AUTH)),
        X509_NONE => {
            security_result(stream, "The VNC server refused the connection").await?;
            apple::logged_in()
        }
        X509_PLAIN => {
            stream.write_all(&plain_message(username, password)?).await.map_err(io)?;
            security_result(stream, "Wrong VNC user name or password").await?;
            apple::logged_in()
        }
        other => Err(format!("VeNCrypt subtype {other} is not supported")),
    }
}

/// X509Plain's login: the two lengths, then the user name and password,
/// in one message.
pub(super) fn plain_message(username: &str, password: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let len = |s: &str| u32::try_from(s.len()).map_err(|_| "User name or password too long".to_string());
    let mut msg = Zeroizing::new(Vec::with_capacity(8 + username.len() + password.len()));
    msg.extend_from_slice(&len(username)?.to_be_bytes());
    msg.extend_from_slice(&len(password)?.to_be_bytes());
    msg.extend_from_slice(username.as_bytes());
    msg.extend_from_slice(password.as_bytes());
    Ok(msg)
}

/// RFB 3.8's SecurityResult: 0 is in; anything else is `refused`, with the
/// server's reason when it gave one.
async fn security_result<S: AsyncRead + Unpin>(stream: &mut S, refused: &str) -> Result<(), String> {
    match stream.read_u32().await.map_err(io)? {
        0 => Ok(()),
        _ => match read_reason(stream).await {
            Ok(reason) if !reason.trim().is_empty() => Err(format!("{refused}: {}", reason.trim())),
            _ => Err(refused.to_string()),
        },
    }
}

#[cfg(test)]
#[path = "vencrypt_tests.rs"]
mod tests;
