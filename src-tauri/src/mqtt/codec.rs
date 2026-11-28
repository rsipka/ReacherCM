//! MQTT 3.1.1 packets: what an explorer sends (CONNECT, SUBSCRIBE,
//! UNSUBSCRIBE, PUBLISH and its acknowledgements, PINGREQ, DISCONNECT) and
//! reads (CONNACK, SUBACK, UNSUBACK, PUBLISH, PUBACK, PUBREC, PUBREL,
//! PUBCOMP, PINGRESP). See the OASIS MQTT 3.1.1 standard, chapter 3.

/// The most a packet may be: the protocol's own limit is 256 MiB, a
/// browser of messages has no use for more than this.
pub const MAX_PACKET: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub client_id: String,
    pub keep_alive: u16,
    pub clean_session: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publish {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
    pub dup: bool,
    /// Set for QoS 1 and 2.
    pub pkid: u16,
}

/// A packet the broker sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    ConnAck { session_present: bool, code: u8 },
    Publish(Publish),
    PubAck(u16),
    PubRec(u16),
    PubRel(u16),
    PubComp(u16),
    SubAck { pkid: u16, codes: Vec<u8> },
    UnsubAck(u16),
    PingResp,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a packet a broker sends, or not well formed.
    Malformed(&'static str),
    TooLarge(usize),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Malformed(m) => write!(f, "The broker sent a malformed packet ({m})"),
            DecodeError::TooLarge(n) => write!(f, "The broker sent a packet of {n} bytes, more than this app takes"),
        }
    }
}

/// What a CONNACK's return code means.
pub fn connack_reason(code: u8) -> &'static str {
    match code {
        1 => "the broker does not speak MQTT 3.1.1",
        2 => "the broker refused the client ID",
        3 => "the broker is unavailable",
        4 => "the user name or password is wrong",
        5 => "not authorised",
        _ => "refused",
    }
}

fn put_len(out: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

fn packet(header: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(header);
    put_len(&mut out, body.len());
    out.extend_from_slice(body);
    out
}

/// Strings are at most 65535 bytes on the wire.
fn check_str(what: &str, s: &str) -> Result<(), String> {
    if s.len() > u16::MAX as usize {
        return Err(format!("The {what} is too long"));
    }
    Ok(())
}

pub fn connect(c: &Connect) -> Result<Vec<u8>, String> {
    check_str("client ID", &c.client_id)?;
    let mut body = Vec::new();
    put_str(&mut body, b"MQTT");
    body.push(4); // 3.1.1
    let mut flags = 0u8;
    if c.clean_session {
        flags |= 0x02;
    }
    if let Some(u) = &c.username {
        check_str("user name", u)?;
        flags |= 0x80;
    }
    if let Some(p) = &c.password {
        check_str("password", p)?;
        flags |= 0x40;
    }
    body.push(flags);
    body.extend_from_slice(&c.keep_alive.to_be_bytes());
    put_str(&mut body, c.client_id.as_bytes());
    if let Some(u) = &c.username {
        put_str(&mut body, u.as_bytes());
    }
    if let Some(p) = &c.password {
        put_str(&mut body, p.as_bytes());
    }
    Ok(packet(0x10, &body))
}

pub fn publish(p: &Publish) -> Result<Vec<u8>, String> {
    check_str("topic", &p.topic)?;
    if p.qos > 2 {
        return Err("QoS is 0, 1 or 2".into());
    }
    let mut header = 0x30 | (p.qos << 1);
    if p.retain {
        header |= 0x01;
    }
    if p.dup {
        header |= 0x08;
    }
    let mut body = Vec::with_capacity(p.topic.len() + p.payload.len() + 4);
    put_str(&mut body, p.topic.as_bytes());
    if p.qos > 0 {
        body.extend_from_slice(&p.pkid.to_be_bytes());
    }
    body.extend_from_slice(&p.payload);
    if body.len() > MAX_PACKET {
        return Err("The message is too large".into());
    }
    Ok(packet(header, &body))
}

pub fn subscribe(pkid: u16, filters: &[(String, u8)]) -> Result<Vec<u8>, String> {
    let mut body = pkid.to_be_bytes().to_vec();
    for (f, qos) in filters {
        check_str("topic filter", f)?;
        put_str(&mut body, f.as_bytes());
        body.push((*qos).min(2));
    }
    Ok(packet(0x82, &body))
}

pub fn unsubscribe(pkid: u16, filters: &[String]) -> Result<Vec<u8>, String> {
    let mut body = pkid.to_be_bytes().to_vec();
    for f in filters {
        check_str("topic filter", f)?;
        put_str(&mut body, f.as_bytes());
    }
    Ok(packet(0xA2, &body))
}

/// PUBACK, PUBREC, PUBREL or PUBCOMP: one of the constants below.
pub fn ack(kind: u8, pkid: u16) -> Vec<u8> {
    packet(kind, &pkid.to_be_bytes())
}
pub const PUBACK: u8 = 0x40;
pub const PUBREC: u8 = 0x50;
/// PUBREL carries the flags 0010.
pub const PUBREL: u8 = 0x62;
pub const PUBCOMP: u8 = 0x70;

pub fn pingreq() -> Vec<u8> {
    vec![0xC0, 0x00]
}

pub fn disconnect() -> Vec<u8> {
    vec![0xE0, 0x00]
}

/// Take one packet off the front of `buf`, if a whole one is there.
pub fn decode(buf: &mut Vec<u8>) -> Result<Option<Incoming>, DecodeError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let mut len = 0usize;
    let mut shift = 0;
    let mut i = 1;
    loop {
        let Some(&byte) = buf.get(i) else { return Ok(None) };
        len |= ((byte & 0x7F) as usize) << shift;
        i += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 21 {
            return Err(DecodeError::Malformed("length"));
        }
    }
    if len > MAX_PACKET {
        return Err(DecodeError::TooLarge(len));
    }
    if buf.len() < i + len {
        return Ok(None);
    }
    let header = buf[0];
    let body: Vec<u8> = buf[i..i + len].to_vec();
    buf.drain(..i + len);
    parse(header, &body).map(Some)
}

fn u16_at(body: &[u8], at: usize) -> Result<u16, DecodeError> {
    body.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or(DecodeError::Malformed("short packet"))
}

fn parse(header: u8, body: &[u8]) -> Result<Incoming, DecodeError> {
    match header >> 4 {
        2 => {
            let flags = *body.first().ok_or(DecodeError::Malformed("CONNACK"))?;
            let code = *body.get(1).ok_or(DecodeError::Malformed("CONNACK"))?;
            Ok(Incoming::ConnAck { session_present: flags & 1 == 1, code })
        }
        3 => {
            let qos = (header >> 1) & 0x03;
            if qos == 3 {
                return Err(DecodeError::Malformed("QoS 3"));
            }
            let tlen = u16_at(body, 0)? as usize;
            let topic = body.get(2..2 + tlen).ok_or(DecodeError::Malformed("PUBLISH topic"))?;
            let topic = String::from_utf8_lossy(topic).into_owned();
            let mut at = 2 + tlen;
            let pkid = if qos > 0 {
                let id = u16_at(body, at)?;
                at += 2;
                id
            } else {
                0
            };
            Ok(Incoming::Publish(Publish {
                topic,
                payload: body[at..].to_vec(),
                qos,
                retain: header & 0x01 == 1,
                dup: header & 0x08 != 0,
                pkid,
            }))
        }
        4 => Ok(Incoming::PubAck(u16_at(body, 0)?)),
        5 => Ok(Incoming::PubRec(u16_at(body, 0)?)),
        6 => Ok(Incoming::PubRel(u16_at(body, 0)?)),
        7 => Ok(Incoming::PubComp(u16_at(body, 0)?)),
        9 => Ok(Incoming::SubAck { pkid: u16_at(body, 0)?, codes: body[2..].to_vec() }),
        11 => Ok(Incoming::UnsubAck(u16_at(body, 0)?)),
        13 => Ok(Incoming::PingResp),
        _ => Err(DecodeError::Malformed("packet type")),
    }
}

/// Whether `topic` is one `filter` (with `+` and `#`) takes. Topics that
/// start with `$` only match a filter that names that first level.
pub fn matches(filter: &str, topic: &str) -> bool {
    let mut f = filter.split('/');
    let mut t = topic.split('/');
    let mut first = true;
    loop {
        match (f.next(), t.next()) {
            (Some("#"), _) => return !(first && topic.starts_with('$')),
            (Some("+"), Some(level)) => {
                if first && level.starts_with('$') {
                    return false;
                }
            }
            (Some(a), Some(b)) => {
                if a != b {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
        first = false;
    }
}

/// Whether a topic filter is well formed: `#` only last and alone in its
/// level, `+` alone in its level.
pub fn valid_filter(filter: &str) -> bool {
    if filter.is_empty() || filter.contains('\0') {
        return false;
    }
    let levels: Vec<&str> = filter.split('/').collect();
    levels.iter().enumerate().all(|(i, l)| match *l {
        "#" => i == levels.len() - 1,
        "+" => true,
        other => !other.contains(['#', '+']),
    })
}

/// Whether a topic can be published to: no wildcards.
pub fn valid_topic(topic: &str) -> bool {
    !topic.is_empty() && !topic.contains(['#', '+', '\0'])
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
