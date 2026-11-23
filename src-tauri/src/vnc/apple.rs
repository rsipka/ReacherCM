//! Logging in to a Mac: macOS Screen Sharing and Apple Remote Desktop.
//!
//! A Mac speaks VNC, but it logs in with a macOS account instead of a VNC
//! password, through security types vnc-rs does not have:
//!
//! * **33, RSA + SRP.** The user name goes RSA-encrypted; the password never
//!   leaves this machine (SRP-6a over the RFC 5054 4096-bit group, SHA-512,
//!   the password stretched with PBKDF2), and the Mac has to prove that it
//!   knows the account as well. Used whenever the Mac offers it, which every
//!   macOS on record does.
//! * **30, Diffie-Hellman** ("Apple Remote Desktop"). User name and password
//!   are AES-encrypted under a key from an exchange nobody authenticates, so
//!   whoever sits in the middle can read them. Used only when the connection
//!   cannot have anyone in the middle: through an SSH tunnel, or to this
//!   machine's own loopback.
//!
//! Neither one encrypts the desktop that follows; that is what connecting
//! through SSH is for.
//!
//! All of it happens here, before vnc-rs sees the stream: [`negotiate`] does
//! the version and security handshake, and [`Shim`] then shows vnc-rs a
//! server that wants no login (or, when the plain VNC password is the one to
//! use, a server that offers only that, so vnc-rs answers it as before).
//! A session without a user name is left alone entirely.
//!
//! The wire format of type 33 is Apple's and undocumented. The framing below
//! is the one macOS 26 is known to send; what the length fields say is
//! checked, so a Mac that answers differently fails with a message instead of
//! a hang. macOS wants each login message in one piece, so each is written
//! with a single `write_all` of a complete buffer (Nagle is off).

use std::pin::Pin;
use std::task::{Context, Poll};

use num_bigint::BigUint;
use rsa::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha512};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use zeroize::Zeroizing;

const RFB_38: &[u8; 12] = b"RFB 003.008\n";
/// What macOS announces, and wants back to offer its own logins.
const RFB_APPLE: &[u8; 12] = b"RFB 003.889\n";

const NONE: u8 = 1;
const VNC_AUTH: u8 = 2;
const APPLE_DH: u8 = 30;
const APPLE_RSA_SRP: u8 = 33;

/// SRP's group: RFC 5054 appendix A, 4096 bits, generator 5. A Mac that
/// names another group is refused rather than trusted with a weaker one.
const SRP_N: &str = include_str!("apple_srp_n4096.hex");
const SRP_G: u8 = 5;
const SRP_BYTES: usize = 512;
const HASH_BYTES: usize = 64;

/// Who to log in as, and whether the way there can have anyone in the middle.
pub(super) struct Login<'a> {
    pub username: &'a str,
    pub password: &'a str,
    /// Through an SSH tunnel or to loopback: the older login (type 30) is
    /// allowed only then.
    pub private_path: bool,
}

/// What vnc-rs is to read before the server's own bytes, and how many of
/// the bytes it writes are answers to that and go nowhere.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Prelude {
    replay: Vec<u8>,
    swallow: usize,
}

/// Do the RFB handshake as far as the login, and the login itself when it is
/// a Mac's. Returns what vnc-rs then needs to see; see [`Shim`].
pub(super) async fn negotiate<S>(stream: &mut S, login: &Login<'_>) -> Result<Prelude, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut version = [0u8; 12];
    stream.read_exact(&mut version).await.map_err(io)?;
    let apple = &version == RFB_APPLE;
    // Without a user name there is no Mac login to do, and a server older
    // than 3.8 has none of these types: vnc-rs does it all, as before.
    if login.username.is_empty() || !(apple || &version == RFB_38) {
        return Ok(Prelude { replay: version.to_vec(), swallow: 0 });
    }
    stream.write_all(if apple { RFB_APPLE } else { RFB_38 }).await.map_err(io)?;

    let count = stream.read_u8().await.map_err(io)?;
    if count == 0 {
        let reason = read_reason(stream).await.unwrap_or_default();
        return Err(format!("The server refused the connection: {reason}"));
    }
    let mut offered = vec![0u8; usize::from(count)];
    stream.read_exact(&mut offered).await.map_err(io)?;
    tracing::debug!("VNC: security types offered {offered:?}");

    match choose(&offered, login.private_path)? {
        // vnc-rs takes over from the type on: it reads the server's answer
        // itself. It is shown a 3.8 server offering just this type, and its
        // version and choice are dropped, as they were already sent.
        t @ (NONE | VNC_AUTH) => {
            stream.write_all(&[t]).await.map_err(io)?;
            Ok(Prelude { replay: [&RFB_38[..], &[1, t]].concat(), swallow: 13 })
        }
        APPLE_RSA_SRP => {
            rsa_srp(stream, login.username, login.password).await?;
            logged_in()
        }
        APPLE_DH => {
            stream.write_all(&[APPLE_DH]).await.map_err(io)?;
            diffie_hellman(stream, login.username, login.password).await?;
            security_result(stream).await?;
            logged_in()
        }
        _ => unreachable!("choose returns one of the four"),
    }
}

/// After a Mac login: vnc-rs is shown a 3.8 server that wants none and
/// says so, then the desktop.
fn logged_in() -> Result<Prelude, String> {
    Ok(Prelude { replay: [&RFB_38[..], &[1, NONE], &[0, 0, 0, 0]].concat(), swallow: 13 })
}

/// Which login, for a session with a user name.
fn choose(offered: &[u8], private_path: bool) -> Result<u8, String> {
    let has = |t: u8| offered.contains(&t);
    if has(APPLE_RSA_SRP) {
        Ok(APPLE_RSA_SRP)
    } else if has(APPLE_DH) && private_path {
        Ok(APPLE_DH)
    } else if has(NONE) {
        Ok(NONE)
    } else if has(VNC_AUTH) {
        Ok(VNC_AUTH)
    } else if has(APPLE_DH) {
        Err("This Mac offers only its older login, which anyone on the way could read the password from. \
             Connect through an SSH session to use it."
            .to_string())
    } else {
        Err(format!("The server offers no login Reacher CM supports (security types {offered:?})"))
    }
}

async fn security_result<S: AsyncRead + Unpin>(stream: &mut S) -> Result<(), String> {
    match stream.read_u32().await.map_err(io)? {
        0 => Ok(()),
        // macOS also writes 1 little-endian. A reason may follow; it adds
        // nothing to what the user is told.
        _ => Err(WRONG_LOGIN.to_string()),
    }
}

const WRONG_LOGIN: &str = "Wrong macOS user name or password";

async fn read_reason<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String, String> {
    let len = stream.read_u32().await.map_err(io)? as usize;
    let mut text = vec![0u8; len.min(4096)];
    stream.read_exact(&mut text).await.map_err(io)?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn io(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        "The server closed the connection during the login".to_string()
    } else {
        format!("Connection failed during the login: {e}")
    }
}

fn unexpected(what: &str) -> String {
    format!("This Mac answered the login in a way Reacher CM does not know ({what})")
}

// ---------------------------------------------------------------------------
// Type 30: Diffie-Hellman, then AES-128-ECB.

/// The server sends a generator, a key length, the prime and its public
/// value; the client answers with 128 encrypted bytes (64 for the user name,
/// 64 for the password, each NUL-terminated, the rest random) and its own
/// public value. The AES key is the MD5 of the shared secret.
async fn diffie_hellman<S>(stream: &mut S, username: &str, password: &str) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let generator = stream.read_u16().await.map_err(io)?;
    let len = usize::from(stream.read_u16().await.map_err(io)?);
    if !(16..=1024).contains(&len) || generator < 2 {
        return Err(unexpected("key exchange parameters"));
    }
    let mut prime = vec![0u8; len];
    stream.read_exact(&mut prime).await.map_err(io)?;
    let mut server_public = vec![0u8; len];
    stream.read_exact(&mut server_public).await.map_err(io)?;

    let mut private = Zeroizing::new(vec![0u8; len]);
    OsRng.fill_bytes(&mut private);
    let (message, _) = dh_answer(generator, &prime, &server_public, &private, username, password)?;
    stream.write_all(&message).await.map_err(io)
}

/// The client's answer to type 30, and the AES key (for tests).
pub(super) fn dh_answer(
    generator: u16,
    prime: &[u8],
    server_public: &[u8],
    private: &[u8],
    username: &str,
    password: &str,
) -> Result<(Zeroizing<Vec<u8>>, [u8; 16]), String> {
    use aes::cipher::{BlockCipherEncrypt, KeyInit};

    let len = prime.len();
    let p = BigUint::from_bytes_be(prime);
    let y = BigUint::from_bytes_be(server_public);
    let one = BigUint::from(1u8);
    if p <= BigUint::from(3u8) || y <= one || y >= &p - &one {
        return Err(unexpected("key exchange values"));
    }
    if username.len() > 63 || password.len() > 63 {
        return Err("The older Mac login takes at most 63 bytes of user name and of password".to_string());
    }
    let x = BigUint::from_bytes_be(private);
    let public = padded(&BigUint::from(generator).modpow(&x, &p), len);
    let shared = Zeroizing::new(padded(&y.modpow(&x, &p), len));
    let key: [u8; 16] = md5::Md5::digest(&*shared).into();

    let mut credentials = Zeroizing::new([0u8; 128]);
    OsRng.fill_bytes(&mut *credentials);
    credentials[..username.len()].copy_from_slice(username.as_bytes());
    credentials[username.len()] = 0;
    credentials[64..64 + password.len()].copy_from_slice(password.as_bytes());
    credentials[64 + password.len()] = 0;
    let cipher = aes::Aes128::new((&key).into());
    for block in credentials.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(block.into());
    }

    let mut message = Zeroizing::new(Vec::with_capacity(128 + len));
    message.extend_from_slice(&*credentials);
    message.extend_from_slice(&public);
    Ok((message, key))
}

/// Big-endian, left-padded with zeros to `len` bytes.
fn padded(n: &BigUint, len: usize) -> Vec<u8> {
    let bytes = n.to_bytes_be();
    let mut out = vec![0u8; len.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

// ---------------------------------------------------------------------------
// Type 33: RSA for the user name, SRP for the password.

/// Every message after the type byte is a u32 length and a body that starts
/// with a version (0x0100) and the tag `RSA1`. In order:
///
/// 1. client: type 33 and a request for the key (one write);
/// 2. server: its RSA-2048 key, DER SubjectPublicKeyInfo;
/// 3. client: the user name, RSA PKCS#1 v1.5;
/// 4. server: the SRP challenge (group, salt, B, PBKDF2 rounds, options);
/// 5. client: A, M1, the options echoed, a nonce;
/// 6. server: M2 (or a failed result in its place), then the result.
async fn rsa_srp<S>(stream: &mut S, username: &str, password: &str) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut request = Vec::with_capacity(15);
    request.push(APPLE_RSA_SRP);
    request.extend_from_slice(&10u32.to_be_bytes());
    request.extend_from_slice(&rsa1_head());
    request.extend_from_slice(&[0, 0, 0, 0]);
    stream.write_all(&request).await.map_err(io)?;

    let reply = read_message(stream, 4096).await?;
    let key = parse_key_reply(&reply)?;
    stream.write_all(&username_message(&key, username)?).await.map_err(io)?;

    let challenge_bytes = read_message(stream, 8192).await?;
    let challenge = parse_challenge(&challenge_bytes)?;
    let password = Zeroizing::new(password.as_bytes().to_vec());
    // PBKDF2 runs up to a few hundred thousand rounds: not on the runtime.
    let proof = tokio::task::spawn_blocking(move || {
        let mut a = Zeroizing::new([0u8; 64]);
        OsRng.fill_bytes(&mut *a);
        a[0] |= 0x80;
        let nonce: [u8; 16] = rand::random();
        srp_proof(&challenge, &password, &*a, &nonce)
    })
    .await
    .map_err(|e| e.to_string())??;
    stream.write_all(&proof_message(&proof.response)?).await.map_err(io)?;

    let len = stream.read_u32().await.map_err(io)?;
    // A wrong password: the result arrives where M2 would.
    if len <= 6 {
        return Err(WRONG_LOGIN.to_string());
    }
    if len > 4096 {
        return Err(unexpected("final message too long"));
    }
    let mut last = vec![0u8; len as usize];
    stream.read_exact(&mut last).await.map_err(io)?;
    if !server_proof_ok(&last, &proof.m2) {
        return Err("This Mac could not prove it knows the account. Reacher CM did not finish the login.".to_string());
    }
    security_result(stream).await
}

/// Version 0x0100 and the tag `RSA1`.
fn rsa1_head() -> [u8; 6] {
    [0x01, 0x00, b'R', b'S', b'A', b'1']
}

async fn read_message<S: AsyncRead + Unpin>(stream: &mut S, max: u32) -> Result<Vec<u8>, String> {
    let len = stream.read_u32().await.map_err(io)?;
    if len <= 6 {
        // A refusal (a result of 1) in place of the message.
        return Err(WRONG_LOGIN.to_string());
    }
    if len > max {
        return Err(unexpected("message too long"));
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).await.map_err(io)?;
    Ok(body)
}

/// u16 1, u32 the DER length, the DER, a trailing byte.
pub(super) fn parse_key_reply(reply: &[u8]) -> Result<rsa::RsaPublicKey, String> {
    use rsa::pkcs8::DecodePublicKey;
    use rsa::traits::PublicKeyParts;

    if reply.len() < 6 || reply[..2] != [0, 1] {
        return Err(unexpected("key reply"));
    }
    let der_len = u32::from_be_bytes([reply[2], reply[3], reply[4], reply[5]]) as usize;
    let der = reply.get(6..6 + der_len).ok_or_else(|| unexpected("key length"))?;
    let key = rsa::RsaPublicKey::from_public_key_der(der).map_err(|_| unexpected("key"))?;
    if key.size() < 256 {
        return Err(unexpected("RSA key under 2048 bits"));
    }
    Ok(key)
}

/// The user name as macOS frames it (u32 length, u16 0, u16 length, the
/// name, u16 0, a zero byte), RSA-encrypted, in an `RSA1` message.
pub(super) fn username_message(key: &rsa::RsaPublicKey, username: &str) -> Result<Vec<u8>, String> {
    use rsa::traits::PublicKeyParts;

    let name = username.as_bytes();
    let room = key.size() - 11 - 11;
    if name.is_empty() || name.len() > room || name.contains(&0) {
        return Err(format!("A macOS user name here can be 1 to {room} bytes"));
    }
    let mut plain = Zeroizing::new(Vec::with_capacity(11 + name.len()));
    plain.extend_from_slice(&((7 + name.len()) as u32).to_be_bytes());
    plain.extend_from_slice(&0u16.to_be_bytes());
    plain.extend_from_slice(&(name.len() as u16).to_be_bytes());
    plain.extend_from_slice(name);
    plain.extend_from_slice(&[0, 0, 0]);
    let sealed = key.encrypt(&mut OsRng, rsa::Pkcs1v15Encrypt, &plain).map_err(|e| e.to_string())?;

    // 4 + 650, the size macOS sends and expects, zero-padded.
    let mut message = Vec::with_capacity(654);
    message.extend_from_slice(&650u32.to_be_bytes());
    message.extend_from_slice(&rsa1_head());
    message.extend_from_slice(&2u16.to_be_bytes());
    message.extend_from_slice(&(sealed.len() as u16).to_be_bytes());
    message.extend_from_slice(&sealed);
    if message.len() > 654 {
        return Err(unexpected("RSA key too large"));
    }
    message.resize(654, 0);
    Ok(message)
}

/// The SRP challenge, as the Mac sent it.
#[derive(Debug, Clone)]
pub(super) struct Challenge {
    pub salt: Vec<u8>,
    pub server_public: Vec<u8>,
    pub iterations: u32,
    pub options: Vec<u8>,
}

/// `RSA1`'s envelope (u32 2, u16 length, u16 0, u16 length), then: a zero
/// byte, N (u16 length), g (u16 length), the salt (u8 length), B (u16
/// length), the PBKDF2 rounds (u64) and the options (u16 length).
pub(super) fn parse_challenge(bytes: &[u8]) -> Result<Challenge, String> {
    let bad = || unexpected("login challenge");
    let mut rest = bytes;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let (head, tail) = rest.split_at_checked(n).ok_or_else(bad)?;
        rest = tail;
        Ok(head)
    };
    let be16 = |b: &[u8]| usize::from(u16::from_be_bytes([b[0], b[1]]));

    let envelope = take(10)?;
    if envelope[..4] != 2u32.to_be_bytes() || be16(&envelope[4..6]) != bytes.len() - 6 || be16(&envelope[8..10]) != bytes.len() - 10 {
        return Err(bad());
    }
    if take(1)? != [0] {
        return Err(bad());
    }
    let n_len = be16(take(2)?);
    let n = take(n_len)?;
    if hex(n) != SRP_N.trim() {
        return Err(unexpected("unknown SRP group"));
    }
    let g_len = be16(take(2)?);
    if take(g_len)? != [SRP_G] {
        return Err(unexpected("unknown SRP generator"));
    }
    let salt_len = usize::from(take(1)?[0]);
    let salt = take(salt_len)?.to_vec();
    let b_len = be16(take(2)?);
    if b_len != SRP_BYTES {
        return Err(bad());
    }
    let server_public = take(b_len)?.to_vec();
    let rounds = take(8)?;
    let iterations = u64::from_be_bytes(rounds.try_into().map_err(|_| bad())?);
    if !(1_000..=10_000_000).contains(&iterations) {
        return Err(unexpected("PBKDF2 rounds"));
    }
    let options_len = be16(take(2)?);
    let options = take(options_len)?.to_vec();
    if !rest.is_empty() || salt.is_empty() {
        return Err(bad());
    }
    Ok(Challenge { salt, server_public, iterations: iterations as u32, options })
}

/// What the client sends, and the M2 the Mac must answer with.
pub(super) struct Proof {
    pub response: Vec<u8>,
    pub m2: [u8; HASH_BYTES],
}

/// SRP-6a as macOS does it, with SHA-512 and N and g padded to 512 bytes:
///
/// * the password is first PBKDF2-HMAC-SHA512'd with the salt (128 bytes);
/// * x = H(salt | H(":" | that)), the identity being empty;
/// * k = H(N | g), u = H(A | B), S = (B − k·gˣ)^(a + u·x), K = H(S);
/// * M1 = H(H(N) ⊕ H(g) | H("") | salt | A | B | K), M2 = H(A | M1 | K).
pub(super) fn srp_proof(challenge: &Challenge, password: &[u8], a: &[u8], nonce: &[u8; 16]) -> Result<Proof, String> {
    let n_bytes = from_hex(SRP_N.trim());
    let n = BigUint::from_bytes_be(&n_bytes);
    let g = BigUint::from(SRP_G);
    let g_pad = padded(&g, SRP_BYTES);
    let b = BigUint::from_bytes_be(&challenge.server_public);
    if b == BigUint::ZERO || b >= n || (&b % &n) == BigUint::ZERO {
        return Err(unexpected("SRP value B"));
    }

    let a = BigUint::from_bytes_be(a);
    let a_pub = padded(&g.modpow(&a, &n), SRP_BYTES);

    let stretched = pbkdf2_sha512(password, &challenge.salt, challenge.iterations, 128);
    let inner = h(&[b":", &stretched]);
    let x = BigUint::from_bytes_be(&h(&[&challenge.salt, &inner]));
    let k = BigUint::from_bytes_be(&h(&[&n_bytes, &g_pad]));
    let u = BigUint::from_bytes_be(&h(&[&a_pub, &challenge.server_public]));
    if u == BigUint::ZERO {
        return Err(unexpected("SRP value u"));
    }

    let v = g.modpow(&x, &n);
    let kv = (&k * &v) % &n;
    let base = ((&b + &n) - kv) % &n;
    if base == BigUint::ZERO {
        return Err(unexpected("SRP value B"));
    }
    let shared = Zeroizing::new(padded(&base.modpow(&(&a + &u * &x), &n), SRP_BYTES));
    let key = h(&[&shared]);

    let h_n = h(&[&n_bytes]);
    let h_g = h(&[&g_pad]);
    let xor: Vec<u8> = h_n.iter().zip(h_g.iter()).map(|(a, b)| a ^ b).collect();
    let m1 = h(&[&xor, &h(&[b""]), &challenge.salt, &a_pub, &challenge.server_public, &key]);
    let m2 = h(&[&a_pub, &m1, &key]);

    let mut response = Vec::with_capacity(2 + SRP_BYTES + 1 + HASH_BYTES + 2 + challenge.options.len() + 17);
    response.extend_from_slice(&(SRP_BYTES as u16).to_be_bytes());
    response.extend_from_slice(&a_pub);
    response.push(HASH_BYTES as u8);
    response.extend_from_slice(&m1);
    response.extend_from_slice(&(challenge.options.len() as u16).to_be_bytes());
    response.extend_from_slice(&challenge.options);
    response.push(16);
    response.extend_from_slice(nonce);
    Ok(Proof { response, m2 })
}

/// The client's proof in its `RSA1` envelope. macOS 26 takes it as a
/// 1080-byte message, zero-padded; a longer one gets its true length.
pub(super) fn proof_message(response: &[u8]) -> Result<Vec<u8>, String> {
    let inner = u16::try_from(response.len()).map_err(|_| unexpected("proof too long"))?;
    let mut body = Vec::with_capacity(1080);
    body.extend_from_slice(&rsa1_head());
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&(inner + 4).to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&inner.to_be_bytes());
    body.extend_from_slice(response);
    if body.len() < 1076 {
        body.resize(1076, 0);
    }
    let mut message = Vec::with_capacity(4 + body.len());
    message.extend_from_slice(&(body.len() as u32).to_be_bytes());
    message.extend_from_slice(&body);
    Ok(message)
}

/// The Mac's last message: u32 2, u16, u16 0, u16, then M2 behind its
/// length byte (64). The proof is compared in constant time.
pub(super) fn server_proof_ok(last: &[u8], m2: &[u8; HASH_BYTES]) -> bool {
    last.len() >= 11 + HASH_BYTES
        && last[..4] == 2u32.to_be_bytes()
        && usize::from(last[10]) == HASH_BYTES
        && bool::from(last[11..11 + HASH_BYTES].ct_eq(m2))
}

fn h(parts: &[&[u8]]) -> [u8; HASH_BYTES] {
    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
    }
    let mut out = [0u8; HASH_BYTES];
    out.copy_from_slice(&hasher.finalize());
    out
}

pub(super) fn pbkdf2_sha512(password: &[u8], salt: &[u8], rounds: u32, len: usize) -> Zeroizing<Vec<u8>> {
    use hmac::{Hmac, KeyInit, Mac};
    type HmacSha512 = Hmac<Sha512>;

    let prf = |input: &[&[u8]]| -> [u8; HASH_BYTES] {
        let mut mac = <HmacSha512 as KeyInit>::new_from_slice(password).expect("HMAC takes any key length");
        for part in input {
            mac.update(part);
        }
        let mut out = [0u8; HASH_BYTES];
        out.copy_from_slice(&mac.finalize().into_bytes());
        out
    };
    let mut out = Zeroizing::new(Vec::with_capacity(len));
    let mut block = 1u32;
    while out.len() < len {
        let mut u = prf(&[salt, &block.to_be_bytes()]);
        let mut t = u;
        for _ in 1..rounds {
            u = prf(&[&u]);
            t.iter_mut().zip(u.iter()).for_each(|(t, u)| *t ^= u);
        }
        let take = (len - out.len()).min(HASH_BYTES);
        out.extend_from_slice(&t[..take]);
        block += 1;
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("constant hex")).collect()
}

// ---------------------------------------------------------------------------

/// The stream vnc-rs is given: what [`negotiate`] left for it to read
/// first, then the server; its first `swallow` bytes written go nowhere.
pub(super) struct Shim<S> {
    inner: S,
    replay: Vec<u8>,
    at: usize,
    swallow: usize,
}

impl<S> Shim<S> {
    pub fn new(inner: S, prelude: Prelude) -> Self {
        Shim { inner, replay: prelude.replay, at: 0, swallow: prelude.swallow }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Shim<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.at < this.replay.len() {
            let n = buf.remaining().min(this.replay.len() - this.at);
            buf.put_slice(&this.replay[this.at..this.at + n]);
            this.at += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Shim<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.swallow > 0 && !buf.is_empty() {
            let n = this.swallow.min(buf.len());
            this.swallow -= n;
            return Poll::Ready(Ok(n));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "apple_tests.rs"]
mod tests;
