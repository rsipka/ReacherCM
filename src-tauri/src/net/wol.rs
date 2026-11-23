//! Wake-on-LAN: the "magic packet", six 0xFF bytes and then the MAC address
//! sixteen times, sent as one UDP broadcast.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// Where a magic packet goes unless told: every host on this network, port 9.
pub const DEFAULT_BROADCAST: Ipv4Addr = Ipv4Addr::BROADCAST;
pub const DEFAULT_PORT: u16 = 9;

/// A MAC address as people write it: `aa:bb:cc:dd:ee:ff`, with dashes, as
/// Cisco does (`aabb.ccdd.eeff`) or with no separators.
pub fn parse_mac(text: &str) -> Result<[u8; 6], String> {
    let hex: String = text.chars().filter(|c| !matches!(c, ':' | '-' | '.' | ' ')).collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Not a MAC address: {}", text.trim()));
    }
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(mac)
}

pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut p = [0xffu8; 102];
    for i in 0..16 {
        p[6 + i * 6..12 + i * 6].copy_from_slice(&mac);
    }
    p
}

/// Send the magic packet for `mac` to `to` (a broadcast address, or a
/// directed one such as 10.0.0.255 for another subnet).
pub fn send(mac: [u8; 6], to: SocketAddr) -> std::io::Result<()> {
    let bind: SocketAddr = match to.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(bind)?;
    if to.is_ipv4() {
        socket.set_broadcast(true)?;
    }
    let packet = magic_packet(mac);
    // Sent three times: it is one UDP datagram with no answer, and a
    // sleeping card's link may still be settling.
    for _ in 0..3 {
        socket.send_to(&packet, to)?;
    }
    Ok(())
}
