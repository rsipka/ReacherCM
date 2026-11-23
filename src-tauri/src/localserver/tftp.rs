//! TFTP (RFC 1350) with the option extension (RFC 2347): block size
//! (RFC 2348), transfer size and timeout (RFC 2349). Each transfer runs from
//! a socket of its own, as the protocol wants. "netascii" is sent as is, as
//! most servers do: devices ask for octet in practice.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use super::fsroot::{io_reason, Root};
use super::Shared;

const RRQ: u16 = 1;
const WRQ: u16 = 2;
const DATA: u16 = 3;
const ACK: u16 = 4;
const ERROR: u16 = 5;
const OACK: u16 = 6;

const ERR_NOT_DEFINED: u16 = 0;
const ERR_NOT_FOUND: u16 = 1;
const ERR_ACCESS: u16 = 2;
const ERR_DISK_FULL: u16 = 3;
const ERR_ILLEGAL: u16 = 4;
const ERR_EXISTS: u16 = 6;

const DEFAULT_BLOCK: usize = 512;
const MAX_BLOCK: usize = 65464;
const RETRIES: u32 = 5;

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub write: bool,
    pub filename: String,
    pub mode: String,
    /// Options as sent, names lowercased.
    pub options: Vec<(String, String)>,
}

pub fn parse_request(packet: &[u8]) -> Option<Request> {
    if packet.len() < 4 {
        return None;
    }
    let opcode = u16::from_be_bytes([packet[0], packet[1]]);
    let write = match opcode {
        RRQ => false,
        WRQ => true,
        _ => return None,
    };
    let body = &packet[2..];
    let body = body.strip_suffix(&[0]).unwrap_or(body);
    let mut fields = body.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    let filename = fields.next()?;
    let mode = fields.next()?.to_ascii_lowercase();
    if filename.is_empty() {
        return None;
    }
    let mut options = Vec::new();
    while let (Some(name), Some(value)) = (fields.next(), fields.next()) {
        options.push((name.to_ascii_lowercase(), value));
    }
    Some(Request { write, filename, mode, options })
}

pub fn error_packet(code: u16, message: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(5 + message.len());
    p.extend_from_slice(&ERROR.to_be_bytes());
    p.extend_from_slice(&code.to_be_bytes());
    p.extend_from_slice(message.as_bytes());
    p.push(0);
    p
}

fn data_packet(block: u16, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(4 + data.len());
    p.extend_from_slice(&DATA.to_be_bytes());
    p.extend_from_slice(&block.to_be_bytes());
    p.extend_from_slice(data);
    p
}

fn ack_packet(block: u16) -> Vec<u8> {
    let mut p = ACK.to_be_bytes().to_vec();
    p.extend_from_slice(&block.to_be_bytes());
    p
}

fn oack_packet(options: &[(String, String)]) -> Vec<u8> {
    let mut p = OACK.to_be_bytes().to_vec();
    for (name, value) in options {
        p.extend_from_slice(name.as_bytes());
        p.push(0);
        p.extend_from_slice(value.as_bytes());
        p.push(0);
    }
    p
}

/// The options this transfer runs with, and the ones to acknowledge.
#[derive(Debug, PartialEq, Eq)]
pub struct Negotiated {
    pub block: usize,
    pub timeout: Duration,
    /// The client's transfer size, for an upload.
    pub tsize: Option<u64>,
    pub ack: Vec<(String, String)>,
}

pub fn negotiate(req: &Request, file_size: Option<u64>) -> Negotiated {
    let mut n = Negotiated {
        block: DEFAULT_BLOCK,
        timeout: Duration::from_secs(3),
        tsize: None,
        ack: Vec::new(),
    };
    for (name, value) in &req.options {
        match name.as_str() {
            "blksize" => {
                if let Ok(size) = value.parse::<usize>() {
                    if size >= 8 {
                        n.block = size.min(MAX_BLOCK);
                        n.ack.push((name.clone(), n.block.to_string()));
                    }
                }
            }
            "timeout" => {
                if let Ok(secs) = value.parse::<u64>() {
                    if (1..=255).contains(&secs) {
                        n.timeout = Duration::from_secs(secs);
                        n.ack.push((name.clone(), secs.to_string()));
                    }
                }
            }
            "tsize" => {
                if req.write {
                    if let Ok(size) = value.parse::<u64>() {
                        n.tsize = Some(size);
                        n.ack.push((name.clone(), size.to_string()));
                    }
                } else if let Some(size) = file_size {
                    n.ack.push((name.clone(), size.to_string()));
                }
            }
            _ => {}
        }
    }
    n
}

pub async fn run(socket: UdpSocket, shared: Arc<Shared>, cancel: CancellationToken) {
    let local_ip = socket.local_addr().map(|a| a.ip()).ok();
    let mut buf = vec![0u8; 65536];
    loop {
        let (len, peer) = tokio::select! {
            _ = cancel.cancelled() => return,
            r = socket.recv_from(&mut buf) => match r {
                Ok(r) => r,
                Err(e) => {
                    // A port-unreachable from an earlier reply surfaces here
                    // on Windows; it is not the server's own failure.
                    tracing::debug!("TFTP receive: {e}");
                    continue;
                }
            },
        };
        let Some(req) = parse_request(&buf[..len]) else {
            let _ = socket.send_to(&error_packet(ERR_ILLEGAL, "Illegal TFTP operation"), peer).await;
            continue;
        };
        let shared = shared.clone();
        let cancel = cancel.child_token();
        tokio::spawn(async move {
            let bind = SocketAddr::new(local_ip.unwrap_or([0, 0, 0, 0].into()), 0);
            let sock = match UdpSocket::bind(bind).await {
                Ok(s) => s,
                Err(e) => {
                    shared.log(Some(peer), format!("Could not open a transfer socket: {e}"));
                    return;
                }
            };
            if sock.connect(peer).await.is_err() {
                return;
            }
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = transfer(&sock, &shared, peer, req) => {}
            }
        });
    }
}

async fn transfer(sock: &UdpSocket, shared: &Shared, peer: SocketAddr, req: Request) {
    let action = if req.write { "Upload" } else { "Download" };
    let virt = match Root::normalize("/", &req.filename) {
        Ok(v) => v,
        Err(_) => {
            let _ = sock.send(&error_packet(ERR_ACCESS, "Invalid file name")).await;
            return;
        }
    };
    shared.log(Some(peer), format!("{action} {virt} ({})", req.mode));
    if req.mode != "octet" && req.mode != "netascii" {
        let _ = sock.send(&error_packet(ERR_ILLEGAL, "Only octet and netascii modes")).await;
        shared.log(Some(peer), format!("Refused: mode {}", req.mode));
        return;
    }
    let real = match shared.root.real(&virt) {
        Ok(r) => r,
        Err(e) => {
            let _ = sock.send(&error_packet(ERR_ACCESS, io_reason(&e))).await;
            shared.log(Some(peer), format!("Refused: {}", io_reason(&e)));
            return;
        }
    };
    let started = Instant::now();
    let result = if req.write {
        if !shared.allow_write {
            let _ = sock.send(&error_packet(ERR_ACCESS, "Uploads are not allowed")).await;
            shared.log(Some(peer), "Refused: uploads are turned off");
            return;
        }
        receive(sock, &real, &req).await
    } else {
        send(sock, &real, &req).await
    };
    match result {
        Ok(bytes) => {
            let secs = started.elapsed().as_secs_f64();
            shared.log(Some(peer), format!("{action} of {virt} done: {bytes} bytes in {secs:.1} s"));
        }
        Err(reason) => shared.log(Some(peer), format!("{action} of {virt} failed: {reason}")),
    }
}

enum Reply {
    Ack(u16),
    Data(u16, Vec<u8>),
}

/// Waits for the client's next packet, sending `resend` again on each timeout.
async fn wait(sock: &UdpSocket, resend: &[u8], timeout: Duration, accept: impl Fn(&Reply) -> bool) -> Result<Reply, String> {
    let mut buf = vec![0u8; MAX_BLOCK + 4];
    let mut tries = 0;
    loop {
        match tokio::time::timeout(timeout, sock.recv(&mut buf)).await {
            Err(_) => {
                tries += 1;
                if tries > RETRIES {
                    return Err("timed out".into());
                }
                sock.send(resend).await.map_err(|e| e.to_string())?;
            }
            Ok(Err(e)) => return Err(e.to_string()),
            Ok(Ok(len)) if len >= 4 => {
                let op = u16::from_be_bytes([buf[0], buf[1]]);
                let num = u16::from_be_bytes([buf[2], buf[3]]);
                let reply = match op {
                    ACK => Reply::Ack(num),
                    DATA => Reply::Data(num, buf[4..len].to_vec()),
                    ERROR => {
                        let msg = String::from_utf8_lossy(&buf[4..len]);
                        return Err(format!("client error {num}: {}", msg.trim_end_matches('\0')));
                    }
                    _ => continue,
                };
                if accept(&reply) {
                    return Ok(reply);
                }
            }
            Ok(Ok(_)) => {}
        }
    }
}

async fn send(sock: &UdpSocket, real: &std::path::Path, req: &Request) -> Result<u64, String> {
    let mut file = match File::open(real).await {
        Ok(f) => f,
        Err(e) => {
            let code = if e.kind() == std::io::ErrorKind::NotFound { ERR_NOT_FOUND } else { ERR_ACCESS };
            let _ = sock.send(&error_packet(code, io_reason(&e))).await;
            return Err(io_reason(&e).into());
        }
    };
    let meta = file.metadata().await.map_err(|e| e.to_string())?;
    if meta.is_dir() {
        let _ = sock.send(&error_packet(ERR_NOT_FOUND, "Is a folder")).await;
        return Err("is a folder".into());
    }
    let n = negotiate(req, Some(meta.len()));
    if !n.ack.is_empty() {
        let oack = oack_packet(&n.ack);
        sock.send(&oack).await.map_err(|e| e.to_string())?;
        wait(sock, &oack, n.timeout, |r| matches!(r, Reply::Ack(0))).await?;
    }
    let mut block: u16 = 1;
    let mut total = 0u64;
    let mut buf = vec![0u8; n.block];
    loop {
        let mut filled = 0;
        while filled < n.block {
            let read = file.read(&mut buf[filled..]).await.map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        let packet = data_packet(block, &buf[..filled]);
        sock.send(&packet).await.map_err(|e| e.to_string())?;
        // Only the ACK of this block counts: a repeated older one is
        // ignored rather than answered, which would double every packet
        // from then on (the "Sorcerer's Apprentice" bug of RFC 1350).
        wait(sock, &packet, n.timeout, |r| matches!(r, Reply::Ack(b) if *b == block)).await?;
        total += filled as u64;
        if filled < n.block {
            return Ok(total);
        }
        block = block.wrapping_add(1);
    }
}

async fn receive(sock: &UdpSocket, real: &std::path::Path, req: &Request) -> Result<u64, String> {
    if real.is_dir() {
        let _ = sock.send(&error_packet(ERR_EXISTS, "Is a folder")).await;
        return Err("is a folder".into());
    }
    let mut file = match File::create(real).await {
        Ok(f) => f,
        Err(e) => {
            let _ = sock.send(&error_packet(ERR_ACCESS, io_reason(&e))).await;
            return Err(io_reason(&e).into());
        }
    };
    let n = negotiate(req, None);
    let mut last = if n.ack.is_empty() { ack_packet(0) } else { oack_packet(&n.ack) };
    sock.send(&last).await.map_err(|e| e.to_string())?;
    let mut block: u16 = 1;
    let mut total = 0u64;
    let result: Result<u64, String> = async {
        loop {
            let expected = block;
            let reply = wait(sock, &last, n.timeout, |r| matches!(r, Reply::Data(..))).await?;
            let Reply::Data(got, data) = reply else { continue };
            if got != expected {
                // The previous block again: our ACK was lost.
                if got == expected.wrapping_sub(1) {
                    sock.send(&last).await.map_err(|e| e.to_string())?;
                }
                continue;
            }
            if data.len() > n.block {
                let _ = sock.send(&error_packet(ERR_ILLEGAL, "Block too large")).await;
                return Err("block larger than agreed".into());
            }
            if let Err(e) = file.write_all(&data).await {
                let code = if e.kind() == std::io::ErrorKind::StorageFull { ERR_DISK_FULL } else { ERR_NOT_DEFINED };
                let _ = sock.send(&error_packet(code, io_reason(&e))).await;
                return Err(io_reason(&e).into());
            }
            total += data.len() as u64;
            last = ack_packet(got);
            sock.send(&last).await.map_err(|e| e.to_string())?;
            if data.len() < n.block {
                file.flush().await.map_err(|e| e.to_string())?;
                return Ok(total);
            }
            block = block.wrapping_add(1);
        }
    }
    .await;
    if result.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(real).await;
    }
    result
}
