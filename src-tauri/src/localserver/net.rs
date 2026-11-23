//! Addresses and ports: which interfaces there are, whether a port is free,
//! and a free one to offer instead.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::Serialize;

use super::ServerKind;

/// One IPv4 address of this computer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interface {
    pub name: String,
    pub ip: String,
    /// A private, link-local or carrier-grade NAT (VPN) address: reachable
    /// from the local network, not from the internet.
    pub private: bool,
    pub loopback: bool,
}

pub fn interfaces() -> Vec<Interface> {
    let mut list: Vec<Interface> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) => Some(Interface {
                name: iface.name.clone(),
                ip: ip.to_string(),
                private: is_private(ip),
                loopback: ip.is_loopback(),
            }),
            IpAddr::V6(_) => None,
        })
        .collect();
    // Network interfaces first, private before public, loopback last.
    list.sort_by_key(|i| (i.loopback, !i.private));
    list.dedup_by(|a, b| a.ip == b.ip);
    list
}

pub fn is_private(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_private() || ip.is_link_local() || ip.is_loopback() || (a == 100 && (64..128).contains(&b))
}

/// The addresses a server listening on `bind` can be reached on.
pub fn reachable_addresses(bind: IpAddr) -> Vec<String> {
    if bind.is_unspecified() {
        let list: Vec<String> = interfaces().into_iter().map(|i| i.ip).collect();
        if list.is_empty() {
            vec![bind.to_string()]
        } else {
            list
        }
    } else {
        vec![bind.to_string()]
    }
}

pub fn bind_tcp(addr: SocketAddr) -> io::Result<tokio::net::TcpListener> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

pub fn bind_udp(addr: SocketAddr) -> io::Result<tokio::net::UdpSocket> {
    let socket = std::net::UdpSocket::bind(addr)?;
    socket.set_nonblocking(true)?;
    tokio::net::UdpSocket::from_std(socket)
}

/// Why a port cannot be listened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortProblem {
    /// Another program listens on it.
    InUse,
    /// Below 1024, where this system wants administrator rights.
    Privileged,
    /// The address is not one of this computer's (an interface went away).
    NoAddress,
    Other,
}

fn problem(e: &io::Error) -> PortProblem {
    match e.kind() {
        io::ErrorKind::AddrInUse => PortProblem::InUse,
        io::ErrorKind::PermissionDenied => PortProblem::Privileged,
        io::ErrorKind::AddrNotAvailable => PortProblem::NoAddress,
        _ => PortProblem::Other,
    }
}

/// Whether the port can be listened on right now. The socket is closed again
/// at once; a server started a moment later still races other programs.
pub fn try_port(kind: ServerKind, addr: SocketAddr) -> Result<(), PortProblem> {
    let result = if kind.is_udp() {
        std::net::UdpSocket::bind(addr).map(drop)
    } else {
        std::net::TcpListener::bind(addr).map(drop)
    };
    result.map_err(|e| problem(&e))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortCheck {
    pub free: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<PortProblem>,
    /// A port that is free, offered instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<u16>,
}

pub fn check_port(kind: ServerKind, bind: IpAddr, port: u16) -> PortCheck {
    match try_port(kind, SocketAddr::new(bind, port)) {
        Ok(()) => PortCheck { free: true, problem: None, suggestion: None },
        Err(problem) => PortCheck {
            free: false,
            problem: Some(problem),
            suggestion: if problem == PortProblem::NoAddress {
                None
            } else {
                suggest_port(kind, bind, port)
            },
        },
    }
}

/// A free port to use instead of `taken`: the kind's usual alternatives
/// first (6969 for TFTP, 2121 for FTP, ...), then the ports just above it.
pub fn suggest_port(kind: ServerKind, bind: IpAddr, taken: u16) -> Option<u16> {
    let usual = kind.port_candidates().iter().copied();
    let above = (1..=100u16).filter_map(|n| taken.checked_add(n)).filter(|p| *p >= 1024);
    usual
        .chain(above)
        .filter(|p| *p != taken)
        .find(|p| try_port(kind, SocketAddr::new(bind, *p)).is_ok())
}

/// The message for a server that could not listen.
pub fn bind_error(kind: ServerKind, addr: SocketAddr, e: &io::Error) -> String {
    match problem(e) {
        PortProblem::InUse => format!("Port {} is already in use", addr.port()),
        PortProblem::Privileged => format!(
            "Port {} needs administrator rights on this system; choose one from 1024 up",
            addr.port()
        ),
        PortProblem::NoAddress => format!("{} is not an address of this computer", addr.ip()),
        PortProblem::Other => format!("{} could not listen on {addr}: {e}", kind.label()),
    }
}
