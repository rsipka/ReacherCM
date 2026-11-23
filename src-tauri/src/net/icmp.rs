//! One ICMP echo (a ping) without administrator rights.
//!
//! - Linux, Android, macOS and iOS: an ICMP *datagram* socket, the kind the
//!   system `ping` itself uses without setuid. Linux allows it to the groups
//!   in `net.ipv4.ping_group_range` (every user on current distributions and
//!   on Android); macOS and iOS to every app. Where it is refused, the
//!   caller falls back to a TCP ping.
//! - Windows: `IcmpSendEcho` / `Icmp6SendEcho2` from iphlpapi, which need no
//!   rights at all.
//!
//! Blocking: run it on a blocking thread.

use std::net::IpAddr;
use std::time::Duration;

/// An echo reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Echo {
    pub ms: f64,
    pub ttl: Option<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum IcmpError {
    /// This device does not let the app send ICMP.
    #[error("ICMP is not available here: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Io(String),
}

/// What an echo request carries after its header.
const PAYLOAD: &[u8; 32] = b"Reacher CM network tools ping..!";

/// ICMP echo request, `type` 8 (IPv4) or 128 (IPv6), with the checksum
/// filled in (Linux and IPv6 put their own; macOS needs it).
pub fn echo_request(v6: bool, id: u16, seq: u16) -> Vec<u8> {
    let mut p = Vec::with_capacity(8 + PAYLOAD.len());
    p.push(if v6 { 128 } else { 8 });
    p.push(0);
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(PAYLOAD);
    if !v6 {
        let sum = checksum(&p);
        p[2..4].copy_from_slice(&sum.to_be_bytes());
    }
    p
}

/// The Internet checksum (RFC 1071).
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 { u16::from_be_bytes([chunk[0], chunk[1]]) } else { u16::from_be_bytes([chunk[0], 0]) };
        sum = sum.wrapping_add(u32::from(word));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// A received datagram read as an echo reply: `(id, seq, ttl)`, or `None`
/// for anything else. macOS hands an IPv4 ICMP socket the IP header too
/// (the TTL is read from it); Linux and IPv6 give the ICMP message alone.
pub fn parse_reply(v6: bool, buf: &[u8]) -> Option<(u16, u16, Option<u8>)> {
    let (icmp, ttl) = if !v6 && buf.len() >= 20 && buf[0] >> 4 == 4 {
        let ihl = usize::from(buf[0] & 0x0f) * 4;
        (buf.get(ihl..)?, Some(buf[8]))
    } else {
        (buf, None)
    };
    if icmp.len() < 8 {
        return None;
    }
    let reply_type = if v6 { 129 } else { 0 };
    if icmp[0] != reply_type || icmp[1] != 0 {
        return None;
    }
    Some((u16::from_be_bytes([icmp[4], icmp[5]]), u16::from_be_bytes([icmp[6], icmp[7]]), ttl))
}

/// Ping `ip` once: `Ok(None)` when no reply came within `timeout`.
#[cfg(unix)]
pub fn echo(ip: IpAddr, timeout: Duration, seq: u16) -> Result<Option<Echo>, IcmpError> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    use std::time::Instant;

    let v6 = ip.is_ipv6();
    let (domain, proto) = if v6 { (Domain::IPV6, Protocol::ICMPV6) } else { (Domain::IPV4, Protocol::ICMPV4) };
    let socket = match Socket::new(domain, Type::DGRAM, Some(proto)) {
        Ok(s) => s,
        // Linux lets only the groups in net.ipv4.ping_group_range open one,
        // and some distributions leave that empty ("1 0"). The system's
        // own ping still works there (setuid or CAP_NET_RAW): use it.
        Err(e) => {
            #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
            if let Some(r) = system_ping(ip, timeout) {
                return r;
            }
            return Err(IcmpError::Unavailable(e.to_string()));
        }
    };
    let socket: std::net::UdpSocket = socket.into();
    // Linux puts its own id (the socket's "port") in, and hands the socket
    // only its own replies; elsewhere the id is ours and is checked.
    let id = (std::process::id() as u16) ^ 0x5243;
    let packet = echo_request(v6, id, seq);
    let to = SockAddr::from(std::net::SocketAddr::new(ip, 0));
    let start = Instant::now();
    let sock2 = socket2::SockRef::from(&socket);
    sock2.send_to(&packet, &to).map_err(|e| IcmpError::Io(e.to_string()))?;

    let deadline = start + timeout;
    let mut buf = [0u8; 1500];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        socket.set_read_timeout(Some(left)).map_err(|e| IcmpError::Io(e.to_string()))?;
        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(IcmpError::Io(e.to_string())),
        };
        if from.ip() != ip {
            continue;
        }
        let Some((rid, rseq, ttl)) = parse_reply(v6, &buf[..n]) else { continue };
        let own_id = cfg!(any(target_os = "linux", target_os = "android")) || rid == id;
        if own_id && rseq == seq {
            return Ok(Some(Echo { ms: start.elapsed().as_secs_f64() * 1000.0, ttl }));
        }
    }
}

/// What the system `ping` printed for one echo: the round trip in ms and
/// the TTL, from a line such as `64 bytes from 10.0.0.5: icmp_seq=1
/// ttl=64 time=0.412 ms` (iputils, BusyBox, macOS; run with LC_ALL=C).
pub fn parse_ping_output(out: &str) -> Option<Echo> {
    let line = out.lines().find(|l| l.contains("time=") || l.contains("time<"))?;
    let after = |key: &str| -> Option<&str> {
        let at = line.find(key)? + key.len();
        let rest = &line[at..];
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        Some(&rest[..end])
    };
    let ms = after("time=").or_else(|| after("time<"))?.parse::<f64>().ok()?;
    let ttl = after("ttl=").or_else(|| after("hlim=")).and_then(|t| t.parse::<u8>().ok());
    Some(Echo { ms, ttl })
}

/// One echo through the system's `ping` program, for where the app may not
/// open an ICMP socket itself. `None` when there is no usable `ping`.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn system_ping(ip: IpAddr, timeout: Duration) -> Option<Result<Option<Echo>, IcmpError>> {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let program = if cfg!(target_os = "macos") && ip.is_ipv6() { "ping6" } else { "ping" };
    let mut cmd = Command::new(program);
    cmd.env("LC_ALL", "C").arg("-n").arg("-c").arg("1");
    if cfg!(target_os = "macos") {
        if ip.is_ipv4() {
            // macOS: -W is the wait for a reply, in milliseconds.
            cmd.arg("-W").arg(timeout.as_millis().max(1).to_string());
        }
    } else {
        if ip.is_ipv6() {
            cmd.arg("-6");
        }
        // iputils and BusyBox: whole seconds.
        cmd.arg("-W").arg(timeout.as_secs().max(1).to_string());
    }
    cmd.arg(ip.to_string()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    // Our own limit too, in case ping ignores -W (ping6 on macOS).
    let deadline = Instant::now() + timeout + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(Ok(None));
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let Some(echo) = parse_ping_output(&stdout) {
        return Some(Ok(Some(echo)));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // It ran but could not send at all (not allowed either): say so.
    if !out.status.success() && stdout.trim().is_empty() && !stderr.trim().is_empty() {
        return Some(Err(IcmpError::Unavailable(stderr.trim().to_string())));
    }
    Some(Ok(None))
}

/// Ping `ip` once: `Ok(None)` when no reply came within `timeout`.
#[cfg(windows)]
pub fn echo(ip: IpAddr, timeout: Duration, _seq: u16) -> Result<Option<Echo>, IcmpError> {
    use windows::Win32::NetworkManagement::IpHelper::{
        Icmp6CreateFile, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMPV6_ECHO_REPLY_LH, ICMP_ECHO_REPLY,
    };
    use windows::Win32::Networking::WinSock::{AF_INET6, IN6_ADDR, IN6_ADDR_0, SOCKADDR_IN6};

    let wait = timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
    let mut reply = vec![0u8; 512];
    match ip {
        IpAddr::V4(v4) => unsafe {
            let handle = IcmpCreateFile().map_err(|e| IcmpError::Unavailable(e.to_string()))?;
            let n = IcmpSendEcho(
                handle,
                u32::from_ne_bytes(v4.octets()),
                PAYLOAD.as_ptr().cast(),
                PAYLOAD.len() as u16,
                None,
                reply.as_mut_ptr().cast(),
                reply.len() as u32,
                wait,
            );
            let _ = IcmpCloseHandle(handle);
            if n == 0 {
                return Ok(None);
            }
            let r = std::ptr::read_unaligned(reply.as_ptr().cast::<ICMP_ECHO_REPLY>());
            // 0 is IP_SUCCESS; anything else (unreachable, TTL expired) is
            // no answer from the host.
            if r.Status != 0 {
                return Ok(None);
            }
            Ok(Some(Echo { ms: f64::from(r.RoundTripTime), ttl: Some(r.Options.Ttl) }))
        },
        IpAddr::V6(v6) => unsafe {
            let handle = Icmp6CreateFile().map_err(|e| IcmpError::Unavailable(e.to_string()))?;
            let source = SOCKADDR_IN6 { sin6_family: AF_INET6, ..Default::default() };
            let dest = SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_addr: IN6_ADDR { u: IN6_ADDR_0 { Byte: v6.octets() } },
                ..Default::default()
            };
            let n = Icmp6SendEcho2(
                handle,
                None,
                None,
                None,
                &source,
                &dest,
                PAYLOAD.as_ptr().cast(),
                PAYLOAD.len() as u16,
                None,
                reply.as_mut_ptr().cast(),
                reply.len() as u32,
                wait,
            );
            let _ = IcmpCloseHandle(handle);
            if n == 0 {
                return Ok(None);
            }
            let r = std::ptr::read_unaligned(reply.as_ptr().cast::<ICMPV6_ECHO_REPLY_LH>());
            if r.Status != 0 {
                return Ok(None);
            }
            Ok(Some(Echo { ms: f64::from(r.RoundTripTime), ttl: None }))
        },
    }
}
