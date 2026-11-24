//! A WireGuard profile as wg-quick writes it (`[Interface]` and `[Peer]`),
//! parsed into what the in-app tunnel needs. Keys that only matter to a
//! system tunnel (`Table`, `PostUp`, `SaveConfig`...) are accepted and left
//! out, so a file exported from any WireGuard app imports as it is.

use std::net::IpAddr;

use base64::Engine;

/// An address with its prefix length: `10.0.0.2/32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl Cidr {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (text, None),
        };
        let addr: IpAddr = addr.parse().map_err(|_| format!("\"{text}\" is not an IP address"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| format!("\"{text}\" has a wrong prefix length"))?,
            None => max,
        };
        Ok(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix)) };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(self.prefix)) };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgPeer {
    pub public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    /// `host:port` or `[v6]:port`, as written; resolved when the tunnel starts.
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<Cidr>,
    pub persistent_keepalive: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgConfig {
    pub private_key: [u8; 32],
    /// The tunnel's own addresses (`Address`).
    pub addresses: Vec<Cidr>,
    /// DNS servers inside the tunnel; names given as search domains are left out.
    pub dns: Vec<IpAddr>,
    pub mtu: Option<u16>,
    pub listen_port: Option<u16>,
    pub peer: WgPeer,
}

pub fn decode_key(text: &str, what: &str) -> Result<[u8; 32], String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| format!("{what} is not a WireGuard key"))?;
    bytes.try_into().map_err(|_| format!("{what} is not a WireGuard key"))
}

pub fn encode_key(key: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(key)
}

/// The public key that goes with a private key.
pub fn public_key(private: &[u8; 32]) -> [u8; 32] {
    let secret = boringtun::x25519::StaticSecret::from(*private);
    boringtun::x25519::PublicKey::from(&secret).to_bytes()
}

fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|v| !v.is_empty())
}

/// Splits `host:port`, `[v6]:port`; the port is required.
pub fn split_endpoint(endpoint: &str) -> Result<(String, u16), String> {
    let e = endpoint.trim();
    let bad = || format!("Endpoint \"{e}\" needs a host and a port (vpn.example.com:51820)");
    let (host, port) = if let Some(rest) = e.strip_prefix('[') {
        let (h, p) = rest.split_once("]:").ok_or_else(bad)?;
        (h, p)
    } else {
        e.rsplit_once(':').ok_or_else(bad)?
    };
    if host.is_empty() || host.contains(':') && !e.starts_with('[') {
        return Err(bad());
    }
    Ok((host.to_string(), port.parse().map_err(|_| bad())?))
}

/// A `[Peer]` while it is read: public key, preshared key, endpoint,
/// allowed IPs, keepalive.
type PeerFields = (Option<[u8; 32]>, Option<[u8; 32]>, Option<String>, Vec<Cidr>, Option<u16>);

impl WgConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        #[derive(PartialEq)]
        enum Section {
            None,
            Interface,
            Peer,
            Other,
        }
        let mut section = Section::None;
        let mut private_key = None;
        let mut addresses = Vec::new();
        let mut dns = Vec::new();
        let mut mtu = None;
        let mut listen_port = None;
        let mut peers: Vec<PeerFields> = Vec::new();

        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = match line[1..line.len() - 1].trim().to_ascii_lowercase().as_str() {
                    "interface" => Section::Interface,
                    "peer" => {
                        peers.push((None, None, None, Vec::new(), None));
                        Section::Peer
                    }
                    _ => Section::Other,
                };
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("Line {} is not \"Key = Value\"", n + 1));
            };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match section {
                Section::Interface => match key.as_str() {
                    "privatekey" => private_key = Some(decode_key(value, "PrivateKey")?),
                    "address" => {
                        for a in list(value) {
                            addresses.push(Cidr::parse(a)?);
                        }
                    }
                    "dns" => dns.extend(list(value).filter_map(|v| v.parse::<IpAddr>().ok())),
                    "mtu" => mtu = Some(value.parse::<u16>().ok().filter(|m| *m >= 576).ok_or("MTU must be a number from 576")?),
                    "listenport" => listen_port = Some(value.parse::<u16>().map_err(|_| "ListenPort must be a port number")?),
                    _ => {}
                },
                Section::Peer => {
                    let peer = peers.last_mut().expect("a [Peer] section was opened");
                    match key.as_str() {
                        "publickey" => peer.0 = Some(decode_key(value, "PublicKey")?),
                        "presharedkey" => peer.1 = Some(decode_key(value, "PresharedKey")?),
                        "endpoint" => {
                            split_endpoint(value)?;
                            peer.2 = Some(value.to_string());
                        }
                        "allowedips" => {
                            for a in list(value) {
                                peer.3.push(Cidr::parse(a)?);
                            }
                        }
                        "persistentkeepalive" => {
                            peer.4 = match value.to_ascii_lowercase().as_str() {
                                "off" | "0" => None,
                                v => Some(v.parse::<u16>().map_err(|_| "PersistentKeepalive must be a number of seconds")?),
                            }
                        }
                        _ => {}
                    }
                }
                Section::None => return Err("The profile must start with [Interface]".into()),
                Section::Other => {}
            }
        }

        let private_key = private_key.ok_or("[Interface] has no PrivateKey")?;
        if addresses.is_empty() {
            return Err("[Interface] has no Address".into());
        }
        let peer = match peers.len() {
            0 => return Err("The profile has no [Peer]".into()),
            1 => peers.pop().expect("one peer"),
            _ => return Err("Profiles with more than one [Peer] are not supported yet".into()),
        };
        let (public_key, preshared_key, endpoint, allowed_ips, persistent_keepalive) = peer;
        Ok(Self {
            private_key,
            addresses,
            dns,
            mtu,
            listen_port,
            peer: WgPeer { public_key: public_key.ok_or("[Peer] has no PublicKey")?, preshared_key, endpoint, allowed_ips, persistent_keepalive },
        })
    }

    /// Whether traffic to `ip` belongs in this tunnel by its AllowedIPs. An
    /// empty list counts as everything.
    pub fn allows(&self, ip: IpAddr) -> bool {
        self.peer.allowed_ips.is_empty() || self.peer.allowed_ips.iter().any(|c| c.contains(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIV: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const PUB: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

    fn sample() -> String {
        format!(
            "# exported\n[Interface]\nPrivateKey = {PRIV}\nAddress = 10.8.0.2/24, fd00::2/64\nDNS = 10.8.0.1, corp.example\nMTU = 1380\nPostUp = iptables -A ...\n\n[Peer]\nPublicKey = {PUB}\nPresharedKey = {PRIV}\nEndpoint = vpn.example.com:51820\nAllowedIPs = 10.8.0.0/24, 192.168.10.0/24\nPersistentKeepalive = 25\n"
        )
    }

    #[test]
    fn parses_a_wg_quick_file() {
        let c = WgConfig::parse(&sample()).unwrap();
        assert_eq!(encode_key(&c.private_key), PRIV);
        assert_eq!(c.addresses.len(), 2);
        assert_eq!(c.addresses[0].to_string(), "10.8.0.2/24");
        assert_eq!(c.dns, vec!["10.8.0.1".parse::<IpAddr>().unwrap()]);
        assert_eq!(c.mtu, Some(1380));
        assert_eq!(encode_key(&c.peer.public_key), PUB);
        assert!(c.peer.preshared_key.is_some());
        assert_eq!(c.peer.endpoint.as_deref(), Some("vpn.example.com:51820"));
        assert_eq!(c.peer.persistent_keepalive, Some(25));
        assert!(c.allows("192.168.10.7".parse().unwrap()));
        assert!(!c.allows("192.168.11.7".parse().unwrap()));
    }

    #[test]
    fn refuses_what_it_cannot_use() {
        assert!(WgConfig::parse("[Peer]\nPublicKey = x").is_err());
        let no_peer = format!("[Interface]\nPrivateKey = {PRIV}\nAddress = 10.0.0.2/32\n");
        assert_eq!(WgConfig::parse(&no_peer).unwrap_err(), "The profile has no [Peer]");
        let two = format!("{}\n[Peer]\nPublicKey = {PUB}\n", sample());
        assert!(WgConfig::parse(&two).unwrap_err().contains("more than one"));
        let bad_key = sample().replace(PUB, "abc");
        assert_eq!(WgConfig::parse(&bad_key).unwrap_err(), "PublicKey is not a WireGuard key");
        assert!(WgConfig::parse(&sample().replace(":51820", "")).is_err());
    }

    #[test]
    fn endpoints_and_cidrs() {
        assert_eq!(split_endpoint("[2001:db8::1]:51820").unwrap(), ("2001:db8::1".to_string(), 51820));
        assert_eq!(split_endpoint("203.0.113.5:443").unwrap(), ("203.0.113.5".to_string(), 443));
        assert!(split_endpoint("2001:db8::1:51820").is_err());
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains("8.8.8.8".parse().unwrap()));
        assert!(Cidr::parse("::/0").unwrap().contains("2001:db8::1".parse().unwrap()));
        assert!(!Cidr::parse("::/0").unwrap().contains("8.8.8.8".parse().unwrap()));
        assert!(Cidr::parse("10.0.0.1/33").is_err());
    }

    #[test]
    fn public_key_matches_wg_pubkey() {
        // `echo yAnz... | wg pubkey` (from the wg(8) man page example pair).
        let private = decode_key(PRIV, "k").unwrap();
        assert_eq!(encode_key(&public_key(&private)), "HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=");
    }
}
