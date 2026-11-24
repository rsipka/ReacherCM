//! Key passphrases kept in memory for this run, so a passphrase-protected
//! key is asked for once, not at every connect.
//!
//! Off unless Settings → Security → "Unlock credential keys at start" is
//! on. Then the keys of the saved credentials are asked for right after the
//! vault opens (`saved_credential_commands::credential_keys_unlock`), and a
//! passphrase typed at a connect is kept too. Nothing is written anywhere:
//! the passphrases go when the vault locks, when the setting is turned off
//! and when the app quits. A passphrase is kept by the key it opens: its
//! public key when the key text shows it without the passphrase (OpenSSH,
//! .ppk), and the text itself. So the same key reached another way (a
//! credential, a session's own key or key file, an IdentityFile, another
//! copy or format of it in the key store) is open as well. Only the keys of
//! saved credentials are asked for at start; a key that sessions use
//! directly is kept once it has been typed at a connect.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type KeyHash = [u8; 32];

static ENABLED: AtomicBool = AtomicBool::new(false);
static REVISION: AtomicU64 = AtomicU64::new(0);

fn kept() -> &'static Mutex<HashMap<KeyHash, Zeroizing<String>>> {
    static K: OnceLock<Mutex<HashMap<KeyHash, Zeroizing<String>>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key text, whatever line ends or byte order mark it has.
fn text_hash(material: &str) -> KeyHash {
    Sha256::digest(super::client::normalize_key_text(material).as_bytes()).into()
}

/// The SHA-256 fingerprint of the public key, read without the passphrase:
/// an OpenSSH key carries it in the clear, a .ppk in its `Public-Lines`. An
/// encrypted PEM key does not show it.
fn public_hash(material: &str) -> Option<KeyHash> {
    let text = super::client::normalize_key_text(material);
    if let Ok(key) = russh::keys::ssh_key::PrivateKey::from_openssh(&text) {
        let fp = key.public_key().fingerprint(russh::keys::HashAlg::Sha256);
        return fp.as_bytes().try_into().ok();
    }
    if text.starts_with("PuTTY-User-Key-File-") {
        let mut lines = text.lines();
        let count: usize = lines.by_ref().find_map(|l| l.strip_prefix("Public-Lines:"))?.trim().parse().ok()?;
        let b64: String = lines.take(count).collect();
        let blob = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()?;
        return Some(Sha256::digest(&blob).into());
    }
    None
}

/// Every name the key is kept under: its public key first, if it shows.
fn ids(material: &str) -> Vec<KeyHash> {
    public_hash(material).into_iter().chain(std::iter::once(text_hash(material))).collect()
}

/// Which key this is: two texts with the same id are the same key.
pub(crate) fn hash(material: &str) -> KeyHash {
    ids(material)[0]
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Changes when a pending startup prompt must discard its answer.
pub(crate) fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

/// Turning it off forgets what was kept.
pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if !on {
        forget();
    }
}

/// The passphrase kept for this key, if any. Kept for another copy of the
/// same key, it may not open this one (its passphrase was changed): the
/// caller tries it and asks when it does not.
pub(crate) fn get(material: &str) -> Option<String> {
    let kept = kept().lock().unwrap();
    ids(material).iter().find_map(|id| kept.get(id)).map(|p| p.as_str().to_string())
}

/// Keep a passphrase that has just opened this key. Does nothing while the
/// setting is off.
pub(crate) fn remember(material: &str, passphrase: &str) {
    if enabled() && !passphrase.is_empty() {
        let mut kept = kept().lock().unwrap();
        for id in ids(material) {
            kept.insert(id, Zeroizing::new(passphrase.to_string()));
        }
    }
}

/// Every name this key is kept under, as text, for the passphrases kept
/// behind the device biometric ([`super::biometric_keys`]).
pub(crate) fn key_ids(material: &str) -> Vec<String> {
    ids(material).iter().map(|id| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, id)).collect()
}

/// Keep a passphrase under names from [`key_ids`]: one the device biometric
/// gave back. Does nothing while the setting is off.
pub(crate) fn remember_ids(names: &[String], passphrase: &str, expected_revision: u64) {
    let mut kept = kept().lock().unwrap();
    if !enabled() || revision() != expected_revision || passphrase.is_empty() {
        return;
    }
    for name in names {
        let id = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, name).ok();
        if let Some(id) = id.and_then(|b| KeyHash::try_from(b.as_slice()).ok()) {
            kept.insert(id, Zeroizing::new(passphrase.to_string()));
        }
    }
}

/// The vault locked, the setting went off, or the app is quitting.
pub fn forget() {
    REVISION.fetch_add(1, Ordering::SeqCst);
    kept().lock().unwrap().clear();
}

/// Whether the key can be used as it is: read with the passphrase stored
/// with it (or none) or with one kept here.
pub(crate) fn opens(material: &str, stored: Option<&str>) -> bool {
    let open = |pass: Option<&str>| super::client::decode_key(material, pass).is_ok_and(|k| !k.is_encrypted());
    open(stored) || get(material).is_some_and(|p| open(Some(&p)))
}

/// The setting and the kept passphrases are process-wide: tests that touch
/// them take this first.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    // The throwaway keys of the keystore tests.
    const PLAIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACCwsCZ1pFDo+/0R+iXN9Emzi+emmN0GEbCoIL9hE1eafgAAAJhbzNn6W8zZ\n+gAAAAtzc2gtZWQyNTUxOQAAACCwsCZ1pFDo+/0R+iXN9Emzi+emmN0GEbCoIL9hE1eafg\nAAAEDs2ThleIPEbrfMS5+KvabXLE0113+oUG6NIEaDtPChvLCwJnWkUOj7/RH6Jc30SbOL\n56aY3QYRsKggv2ETV5p+AAAAEnJlYWNoLXRlc3RAZXhhbXBsZQECAw==\n-----END OPENSSH PRIVATE KEY-----\n";
    const ENCRYPTED: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABCcqk0FV4\nKt6ydcd+Fwf0h1AAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIJk+zQU8O+k3l0Y9\nx4epBs/36Jir8BpbhqTJH0+tr864AAAAoPYA20IkAMre5HEKk0iDCkEk72vCMulAvfM7LK\nO3hbWbXJAzOgl5v1AzwoTzi73AvzN7rTxU+wsQ9vuWntsc7WjaqdQeg/ZHi3pNKRkyX+Ay\nGmXQTdgeHjjkQ/mc7GTjpqYFV32+LlFRoC+zP50ivDKINyUXgzd0zz/G2xR5zQjU/boKQU\n719oRMkiGz8P1eib8piYDdiJ3fY0MDqbac8QM=\n-----END OPENSSH PRIVATE KEY-----\n";
    const PASSPHRASE: &str = "hunter2";

    // One test: the setting and the kept passphrases are process-wide.
    #[test]
    fn keeps_a_passphrase_only_while_on_and_until_forgotten() {
        let _alone = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_enabled(false);
        remember(ENCRYPTED, PASSPHRASE);
        assert_eq!(get(ENCRYPTED), None, "nothing is kept while the setting is off");
        assert!(opens(PLAIN, None));
        assert!(!opens(ENCRYPTED, None));
        assert!(opens(ENCRYPTED, Some(PASSPHRASE)));

        set_enabled(true);
        remember(ENCRYPTED, "");
        assert_eq!(get(ENCRYPTED), None);
        remember(ENCRYPTED, PASSPHRASE);
        // The same key with Windows line ends and a byte order mark.
        let messy = format!("\u{feff}{}", ENCRYPTED.replace('\n', "\r\n"));
        assert_eq!(get(&messy).as_deref(), Some(PASSPHRASE));
        assert!(opens(ENCRYPTED, None));
        assert_eq!(get(PLAIN), None);

        let pending_prompt = revision();
        forget();
        remember_ids(&key_ids(ENCRYPTED), PASSPHRASE, pending_prompt);
        assert!(!opens(ENCRYPTED, None));
        remember(ENCRYPTED, PASSPHRASE);
        set_enabled(false);
        assert_eq!(get(ENCRYPTED), None, "turning the setting off forgets");
    }

    /// The same key encrypted again: another salt, so other text.
    fn another_copy(passphrase: &str) -> String {
        let key = russh::keys::ssh_key::PrivateKey::from_openssh(ENCRYPTED).unwrap().decrypt(PASSPHRASE).unwrap();
        let copy = key.encrypt(&mut rand::rng(), passphrase).unwrap();
        copy.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap().to_string()
    }

    #[test]
    fn the_same_key_in_another_copy_is_open_too() {
        let _alone = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_enabled(true);
        forget();
        let copy = another_copy(PASSPHRASE);
        assert_ne!(text_hash(&copy), text_hash(ENCRYPTED), "the copy's text differs");
        assert_eq!(hash(&copy), hash(ENCRYPTED), "but it is the same key");

        // Unlocked as the credential's key, open as a session's own key file.
        remember(ENCRYPTED, PASSPHRASE);
        assert!(opens(&copy, None));

        // A copy with another passphrase gets the kept one, which does not
        // open it: the connect asks, as before.
        let changed = another_copy("other");
        assert_eq!(get(&changed).as_deref(), Some(PASSPHRASE));
        assert!(!opens(&changed, None));

        // A key of another identity is not open.
        assert_eq!(get(PLAIN), None);
        forget();
        set_enabled(false);
    }

    #[test]
    fn a_ppk_is_known_by_its_public_key() {
        let key = russh::keys::ssh_key::PrivateKey::from_openssh(ENCRYPTED).unwrap();
        let blob = key.public_key().to_bytes().unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, blob);
        let (a, b) = b64.split_at(40);
        let ppk = format!("PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: aes256-cbc\nComment: test\nPublic-Lines: 2\n{a}\n{b}\nPrivate-Lines: 1\nAAAA\n");
        assert_eq!(public_hash(&ppk), public_hash(ENCRYPTED));
        assert!(public_hash(&ppk).is_some());
        // An encrypted PEM key does not show its public key: known by its text.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert_eq!(public_hash(pem), None);
        assert_eq!(hash(pem), text_hash(pem));
    }
}
