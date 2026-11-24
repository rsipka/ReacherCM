//! Network tools: ping, a TCP port scan and Wake-on-LAN, run from this
//! device (Network tools in the sidebar, Ping / Scan ports / Wake in a
//! session's menu).
//!
//! Nothing here logs in anywhere or sends a session's secrets: ping sends
//! ICMP echo requests (or opens and closes a TCP connection), the scan opens
//! and closes TCP connections, and Wake-on-LAN sends one UDP broadcast.

pub mod icmp;
pub mod scan;
pub mod wol;

use serde::Serialize;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// The addresses a name resolves to, IPv4 first (what most ping tools pick),
/// each once. An address is returned as it is.
pub async fn resolve(host: &str) -> Result<Vec<IpAddr>, String> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err("No address given".into());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    let found = tokio::net::lookup_host((host, 0)).await.map_err(|e| format!("{host}: {e}"))?;
    let mut out: Vec<IpAddr> = Vec::new();
    for a in found {
        if !out.contains(&a.ip()) {
            out.push(a.ip());
        }
    }
    out.sort_by_key(|ip| ip.is_ipv6());
    if out.is_empty() {
        return Err(format!("{host}: no address found"));
    }
    Ok(out)
}

/// One ping's answer.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PingReply {
    /// The host answered.
    pub ok: bool,
    /// Round trip in milliseconds, when it answered.
    pub ms: Option<f64>,
    /// `icmp` or `tcp`: what was sent.
    pub method: &'static str,
    /// The reply's TTL, where the platform says it.
    pub ttl: Option<u8>,
    /// Why there is no answer, when it is not a plain timeout.
    pub error: Option<String>,
}

/// TCP "ping": the time a TCP handshake to `port` takes. A refused
/// connection still answers, so the host is up.
pub async fn tcp_ping(ip: IpAddr, port: u16, timeout: Duration) -> PingReply {
    let start = Instant::now();
    let attempt = tcp_connect(ip, port, timeout).await;
    tcp_ping_reply(attempt, start.elapsed())
}

/// `None` is the application's deadline. Socket errors keep their OS details.
async fn tcp_connect(ip: IpAddr, port: u16, timeout: Duration) -> Option<io::Result<()>> {
    tokio::time::timeout(timeout, tokio::net::TcpStream::connect(SocketAddr::new(ip, port)))
        .await.ok().map(|result| result.map(drop))
}

fn tcp_ping_reply(attempt: Option<io::Result<()>>, elapsed: Duration) -> PingReply {
    let reply = |ok: bool, error: Option<String>| PingReply {
        ok,
        ms: ok.then(|| elapsed.as_secs_f64() * 1000.0),
        method: "tcp",
        ttl: None,
        error,
    };
    match attempt {
        Some(Ok(())) => reply(true, None),
        Some(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => reply(true, None),
        Some(Err(e)) => reply(false, Some(e.to_string())),
        None => reply(false, None),
    }
}

/// How a ping goes: ICMP echo, else TCP when this device does not let an
/// app send ICMP (`auto`); ICMP only; or TCP to a port only.
pub async fn ping(ip: IpAddr, method: &str, port: u16, timeout: Duration, seq: u16) -> PingReply {
    if method == "tcp" {
        return tcp_ping(ip, port, timeout).await;
    }
    let echo = tokio::task::spawn_blocking(move || icmp::echo(ip, timeout, seq)).await;
    match echo {
        Ok(Ok(r)) => PingReply { ok: r.is_some(), ms: r.as_ref().map(|r| r.ms), method: "icmp", ttl: r.and_then(|r| r.ttl), error: None },
        Ok(Err(icmp::IcmpError::Unavailable(why))) if method == "auto" => {
            tracing::debug!("ICMP not available ({why}); TCP ping to port {port}");
            tcp_ping(ip, port, timeout).await
        }
        Ok(Err(e)) => PingReply { ok: false, ms: None, method: "icmp", ttl: None, error: Some(e.to_string()) },
        Err(e) => PingReply { ok: false, ms: None, method: "icmp", ttl: None, error: Some(e.to_string()) },
    }
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;
