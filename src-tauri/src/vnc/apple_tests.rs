//! The Mac logins against a fake Mac that does the server's half by the
//! same rules, and the stream vnc-rs is given afterwards.

use super::*;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};

const USER: &str = "radek";
const PASSWORD: &str = "correct horse";

fn n() -> BigUint {
    BigUint::from_bytes_be(&from_hex(SRP_N.trim()))
}

async fn read_len_body(s: &mut DuplexStream) -> Vec<u8> {
    let len = s.read_u32().await.unwrap();
    let mut body = vec![0u8; len as usize];
    s.read_exact(&mut body).await.unwrap();
    body
}

/// What the Mac does for type 33 with this account, `password` being the
/// account's real one. Returns once it has sent its last message.
async fn fake_mac_rsa_srp(mut s: DuplexStream, password: &str) -> Result<(), String> {
    use rsa::pkcs8::EncodePublicKey;

    s.write_all(RFB_APPLE).await.unwrap();
    let mut version = [0u8; 12];
    s.read_exact(&mut version).await.unwrap();
    assert_eq!(&version, RFB_APPLE);
    s.write_all(&[3, 30, 33, 36]).await.unwrap();

    // The type and the key request arrive together.
    let mut request = [0u8; 15];
    s.read_exact(&mut request).await.unwrap();
    assert_eq!(request[0], 33);
    assert_eq!(&request[1..5], &10u32.to_be_bytes());
    assert_eq!(&request[5..11], b"\x01\x00RSA1");

    let private = rsa::RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    let der = private.to_public_key().to_public_key_der().unwrap();
    let der = der.as_bytes();
    let mut reply = vec![0, 1];
    reply.extend_from_slice(&(der.len() as u32).to_be_bytes());
    reply.extend_from_slice(der);
    reply.push(0);
    s.write_all(&(reply.len() as u32).to_be_bytes()).await.unwrap();
    s.write_all(&reply).await.unwrap();

    let init = read_len_body(&mut s).await;
    assert_eq!(init.len(), 650);
    assert_eq!(&init[..6], b"\x01\x00RSA1");
    let sealed_len = usize::from(u16::from_be_bytes([init[8], init[9]]));
    let plain = private.decrypt(rsa::Pkcs1v15Encrypt, &init[10..10 + sealed_len]).unwrap();
    let name_len = usize::from(u16::from_be_bytes([plain[6], plain[7]]));
    assert_eq!(&plain[..4], &((7 + name_len) as u32).to_be_bytes());
    assert_eq!(&plain[8..8 + name_len], USER.as_bytes());

    // The account's verifier, as the Mac keeps it.
    let salt: Vec<u8> = (0..32).collect();
    let iterations = 1000u32;
    let n = n();
    let n_bytes = padded(&n, SRP_BYTES);
    let g = BigUint::from(SRP_G);
    let g_pad = padded(&g, SRP_BYTES);
    let stretched = pbkdf2_sha512(password.as_bytes(), &salt, iterations, 128);
    let x = BigUint::from_bytes_be(&h(&[&salt, &h(&[b":", &stretched])]));
    let v = g.modpow(&x, &n);
    let k = BigUint::from_bytes_be(&h(&[&n_bytes, &g_pad]));
    let b = BigUint::from_bytes_be(&[0x5a; 64]);
    let b_pub = padded(&((&k * &v + g.modpow(&b, &n)) % &n), SRP_BYTES);
    let options = vec![0x42u8; 80];

    let mut fields = vec![0u8];
    fields.extend_from_slice(&(SRP_BYTES as u16).to_be_bytes());
    fields.extend_from_slice(&n_bytes);
    fields.extend_from_slice(&[0, 1, SRP_G, 32]);
    fields.extend_from_slice(&salt);
    fields.extend_from_slice(&(SRP_BYTES as u16).to_be_bytes());
    fields.extend_from_slice(&b_pub);
    fields.extend_from_slice(&u64::from(iterations).to_be_bytes());
    fields.extend_from_slice(&80u16.to_be_bytes());
    fields.extend_from_slice(&options);
    let mut challenge = 2u32.to_be_bytes().to_vec();
    challenge.extend_from_slice(&((fields.len() + 4) as u16).to_be_bytes());
    challenge.extend_from_slice(&[0, 0]);
    challenge.extend_from_slice(&(fields.len() as u16).to_be_bytes());
    challenge.extend_from_slice(&fields);
    assert_eq!(challenge.len(), 1165, "the size macOS 26 sends");
    s.write_all(&(challenge.len() as u32).to_be_bytes()).await.unwrap();
    s.write_all(&challenge).await.unwrap();

    let proof = read_len_body(&mut s).await;
    assert_eq!(proof.len(), 1076);
    assert_eq!(&proof[..6], b"\x01\x00RSA1");
    let inner = usize::from(u16::from_be_bytes([proof[12], proof[13]]));
    let r = &proof[14..14 + inner];
    let a_pub = &r[2..2 + SRP_BYTES];
    let m1 = &r[2 + SRP_BYTES + 1..2 + SRP_BYTES + 1 + 64];
    assert_eq!(&r[2 + SRP_BYTES + 65 + 2..2 + SRP_BYTES + 65 + 2 + 80], &options[..]);

    let a = BigUint::from_bytes_be(a_pub);
    let u = BigUint::from_bytes_be(&h(&[a_pub, &b_pub]));
    let shared = padded(&(a * v.modpow(&u, &n)).modpow(&b, &n), SRP_BYTES);
    let key = h(&[&shared]);
    let xor: Vec<u8> = h(&[&n_bytes]).iter().zip(h(&[&g_pad]).iter()).map(|(a, b)| a ^ b).collect();
    let expected = h(&[&xor, &h(&[b""]), &salt, a_pub, &b_pub, &key]);
    if m1 != expected {
        s.write_all(&1u32.to_be_bytes()).await.unwrap();
        return Err("wrong password".into());
    }
    let m2 = h(&[a_pub, m1, &key]);
    let mut last = 2u32.to_be_bytes().to_vec();
    last.extend_from_slice(&92u16.to_be_bytes());
    last.extend_from_slice(&[0, 0]);
    last.extend_from_slice(&88u16.to_be_bytes());
    last.push(64);
    last.extend_from_slice(&m2);
    last.push(16);
    last.extend_from_slice(&[7; 16]);
    last.extend_from_slice(&[0; 6]);
    assert_eq!(last.len(), 98);
    s.write_all(&(last.len() as u32).to_be_bytes()).await.unwrap();
    s.write_all(&last).await.unwrap();
    s.write_all(&0u32.to_be_bytes()).await.unwrap();
    Ok(())
}

#[tokio::test]
async fn rsa_srp_logs_in() {
    let (mut client, server) = duplex(1 << 16);
    let mac = tokio::spawn(fake_mac_rsa_srp(server, PASSWORD));
    let login = Login { username: USER, password: PASSWORD, private_path: false };
    let prelude = negotiate(&mut client, &login).await.unwrap();
    assert_eq!(prelude, logged_in().unwrap());
    mac.await.unwrap().unwrap();
}

#[tokio::test]
async fn rsa_srp_wrong_password() {
    let (mut client, server) = duplex(1 << 16);
    let mac = tokio::spawn(fake_mac_rsa_srp(server, "something else"));
    let login = Login { username: USER, password: PASSWORD, private_path: false };
    assert_eq!(negotiate(&mut client, &login).await.unwrap_err(), WRONG_LOGIN);
    assert!(mac.await.unwrap().is_err());
}

#[test]
fn server_proof_is_checked() {
    let m2 = [9u8; 64];
    let mut last = 2u32.to_be_bytes().to_vec();
    last.extend_from_slice(&[0, 92, 0, 0, 0, 88, 64]);
    last.extend_from_slice(&m2);
    last.extend_from_slice(&[16; 23]);
    assert!(server_proof_ok(&last, &m2));
    last[20] ^= 1;
    assert!(!server_proof_ok(&last, &m2));
    assert!(!server_proof_ok(&last[..40], &m2));
}

#[test]
fn pbkdf2_matches_known_values() {
    // From Python's hashlib.pbkdf2_hmac('sha512', ...).
    assert_eq!(
        hex(&pbkdf2_sha512(b"password", b"salt", 1, 64)),
        "867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252c02d470a285a0501bad999bfe943c08f050235d7d68b1da55e63f73b60a57fce"
    );
    let salt: Vec<u8> = (0..32).collect();
    assert_eq!(
        hex(&pbkdf2_sha512(b"correct horse", &salt, 1000, 128)),
        "7b34d72b3aedbb7b91554546e40b31f86213a18bd9b3a03ca7d553f77879ad34b783837461d823d5e1e1a127c26a425b67ef3206dad971458f257476ee35b10e225eb342bd492b1716ab5b625efe1f61e3caa1387fe38e4ff5a7eb75ee21107ad0159b07eb60d7e72958fd16e242ecbd1fc1ef4993c6e4d106ea948b42c92eeb"
    );
}

#[test]
fn srp_group_is_rfc_5054_4096() {
    // SHA-256 of the 512-byte modulus, as RFC 5054 appendix A gives it.
    let digest = sha2::Sha256::digest(from_hex(SRP_N.trim()));
    assert_eq!(hex(&digest), "4ee95187682bcb230ad26a95205f6920e84708f6251b3894329b09ec23919e33");
}

/// What the Mac does for type 30: a 512-byte group, then it decrypts.
async fn fake_mac_dh(mut s: DuplexStream, and_then_desktop: bool) -> (String, String) {
    use aes::cipher::{BlockCipherDecrypt, KeyInit};

    s.write_all(RFB_APPLE).await.unwrap();
    let mut version = [0u8; 12];
    s.read_exact(&mut version).await.unwrap();
    s.write_all(&[2, 30, 36]).await.unwrap();
    assert_eq!(s.read_u8().await.unwrap(), 30);

    let n = n();
    let len = SRP_BYTES;
    let secret = BigUint::from_bytes_be(&[0x33; 40]);
    let public = padded(&BigUint::from(2u8).modpow(&secret, &n), len);
    s.write_all(&2u16.to_be_bytes()).await.unwrap();
    s.write_all(&(len as u16).to_be_bytes()).await.unwrap();
    s.write_all(&padded(&n, len)).await.unwrap();
    s.write_all(&public).await.unwrap();

    let mut answer = vec![0u8; 128 + len];
    s.read_exact(&mut answer).await.unwrap();
    let theirs = BigUint::from_bytes_be(&answer[128..]);
    let shared = padded(&theirs.modpow(&secret, &n), len);
    let key = md5::Md5::digest(&shared);
    let cipher = aes::Aes128::new(&key);
    let mut creds = answer[..128].to_vec();
    for block in creds.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(block.into());
    }
    let field = |b: &[u8]| String::from_utf8(b[..b.iter().position(|c| *c == 0).unwrap()].to_vec()).unwrap();
    let got = (field(&creds[..64]), field(&creds[64..]));
    s.write_all(&0u32.to_be_bytes()).await.unwrap();

    if and_then_desktop {
        // ClientInit, then a 1×1 desktop called "m".
        assert_eq!(s.read_u8().await.unwrap(), 1);
        let mut init = Vec::new();
        init.extend_from_slice(&1u16.to_be_bytes());
        init.extend_from_slice(&1u16.to_be_bytes());
        init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        init.extend_from_slice(&1u32.to_be_bytes());
        init.push(b'm');
        s.write_all(&init).await.unwrap();
        let mut sink = [0u8; 256];
        while s.read(&mut sink).await.is_ok_and(|n| n > 0) {}
    }
    got
}

#[tokio::test]
async fn older_login_only_on_a_private_path() {
    let (mut client, server) = duplex(1 << 16);
    let mac = tokio::spawn(fake_mac_dh(server, false));
    let login = Login { username: USER, password: PASSWORD, private_path: true };
    assert_eq!(negotiate(&mut client, &login).await.unwrap(), logged_in().unwrap());
    assert_eq!(mac.await.unwrap(), (USER.to_string(), PASSWORD.to_string()));

    let (mut client, mut server) = duplex(1 << 16);
    tokio::spawn(async move {
        server.write_all(RFB_APPLE).await.unwrap();
        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        server.write_all(&[2, 30, 36]).await.unwrap();
        let mut rest = Vec::new();
        let _ = server.read_to_end(&mut rest).await;
        assert!(rest.is_empty(), "nothing is sent for the older login over the open network");
    });
    let login = Login { username: USER, password: PASSWORD, private_path: false };
    assert!(negotiate(&mut client, &login).await.unwrap_err().contains("SSH"));
}

/// The whole way: the Mac login here, then vnc-rs on the shim, reaching the
/// desktop as if the server had wanted no login.
#[tokio::test]
async fn vnc_rs_runs_after_a_mac_login() {
    let (mut client, server) = duplex(1 << 16);
    let mac = tokio::spawn(fake_mac_dh(server, true));
    let login = Login { username: USER, password: PASSWORD, private_path: true };
    let prelude = negotiate(&mut client, &login).await.unwrap();
    let vnc = vnc::VncConnector::new(Shim::new(client, prelude))
        .set_auth_method(std::future::ready(Ok(String::new())))
        .add_encoding(vnc::VncEncoding::Raw)
        .allow_shared(true)
        .set_pixel_format(vnc::PixelFormat::rgba())
        .build()
        .unwrap()
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();
    let _ = vnc.close().await;
    drop(vnc);
    let _ = mac.await;
}

#[tokio::test]
async fn without_a_user_name_nothing_changes() {
    let (mut client, mut server) = duplex(1024);
    server.write_all(RFB_APPLE).await.unwrap();
    let login = Login { username: "", password: "secret", private_path: false };
    let prelude = negotiate(&mut client, &login).await.unwrap();
    assert_eq!(prelude, Prelude { replay: RFB_APPLE.to_vec(), swallow: 0 });
}

#[tokio::test]
async fn a_plain_vnc_server_keeps_its_password() {
    let (mut client, mut server) = duplex(1024);
    server.write_all(RFB_38).await.unwrap();
    server.write_all(&[1, 2]).await.unwrap();
    let login = Login { username: USER, password: "secret", private_path: false };
    let prelude = negotiate(&mut client, &login).await.unwrap();
    assert_eq!(prelude, Prelude { replay: [&RFB_38[..], &[1, 2]].concat(), swallow: 13 });
    let mut sent = [0u8; 13];
    server.read_exact(&mut sent).await.unwrap();
    assert_eq!(&sent[..12], RFB_38);
    assert_eq!(sent[12], 2);
}

#[test]
fn choice_prefers_the_safer_login() {
    assert_eq!(choose(&[30, 33, 36], false).unwrap(), 33);
    assert_eq!(choose(&[30, 33, 36, 2, 35], true).unwrap(), 33);
    assert_eq!(choose(&[30, 2], true).unwrap(), 30);
    assert_eq!(choose(&[30, 2], false).unwrap(), 2);
    assert!(choose(&[30, 36], false).is_err());
    assert!(choose(&[19], true).is_err());
}

#[test]
fn loopback_counts_as_private() {
    assert!(super::super::is_loopback("localhost"));
    assert!(super::super::is_loopback("127.0.0.1"));
    assert!(super::super::is_loopback("[::1]"));
    assert!(!super::super::is_loopback("mac.local"));
}
