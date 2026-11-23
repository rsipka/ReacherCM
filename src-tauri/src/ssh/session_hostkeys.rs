//! Host keys saved on a session, as Remote Desktop Manager keeps them: the
//! server's fingerprint is part of the session record, so it is encrypted,
//! synced and shared with the rest of the session. When a new or changed key
//! comes up, the person picks Accept once (nothing is kept), Accept and save
//! (kept on the session) or Don't connect.
//!
//! Every hop of a connection is kept on the session being connected, under
//! its own `host:port`, so a jump host's key travels with the session that
//! goes through it. A connection without a saved session (Quick Connect,
//! RDP) keeps using this device's own list, `ssh/known_hosts.json`.
//!
//! The device list is still read: a key found there and not yet on the
//! session is copied onto it the first time the session connects, which is
//! how keys accepted by older builds move over.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

/// One server key a session trusts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedHostKey {
    /// The server as it was reached, `host:port` (a jump host's own).
    pub host: String,
    /// `ssh-ed25519`, `rsa-sha2-512`...; empty when typed in by hand.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key_type: String,
    /// `SHA256:` and the key's SHA-256 in base64, without padding.
    pub fingerprint: String,
    /// When it was saved, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<i64>,
}

/// What the person answered to a host-key question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Reject,
    /// Connect this time, remember nothing.
    Once,
    /// Connect, and remember the key.
    Save,
}

impl Answer {
    /// From the dialog's two values; a reply without `save` is an older
    /// window's plain Accept, which always saved.
    pub fn from_reply(accept: bool, save: Option<bool>) -> Self {
        match (accept, save.unwrap_or(true)) {
            (false, _) => Answer::Reject,
            (true, false) => Answer::Once,
            (true, true) => Answer::Save,
        }
    }

    pub fn accepted(self) -> bool {
        self != Answer::Reject
    }
}

/// `SHA256:` and base64 for display and storage, from the bare base64 the
/// SSH code works with (or a value that already has the prefix).
pub fn with_prefix(fingerprint: &str) -> String {
    let bare = fingerprint.strip_prefix("SHA256:").unwrap_or(fingerprint);
    format!("SHA256:{bare}")
}

/// The bare base64 of a stored or typed fingerprint.
pub fn bare(fingerprint: &str) -> &str {
    fingerprint.trim().strip_prefix("SHA256:").unwrap_or(fingerprint.trim())
}

/// A fingerprint typed in by hand, as `ssh-keygen -lf` prints it
/// (`SHA256:` and 43 characters of base64, a trailing `=` allowed), in the
/// stored form; `None` when it is not one.
pub fn parse_typed(fingerprint: &str) -> Option<String> {
    let s = fingerprint.trim();
    let b = s.strip_prefix("SHA256:").unwrap_or(s).trim_end_matches('=');
    let ok = b.len() == 43 && b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/');
    ok.then(|| with_prefix(b))
}

/// The saved session a connection is for, with the keys it trusts as they
/// were read when the connection started. Shared by every hop.
#[derive(Debug)]
pub struct SessionTrust {
    pub session_id: String,
    saved: Mutex<Vec<SavedHostKey>>,
    /// Every `host:port` the session's own server is reached on (its main
    /// address and its other ones): one server, so a key saved under one of
    /// them is that server's key under all of them.
    addresses: Vec<String>,
}

impl SessionTrust {
    pub fn new(session_id: impl Into<String>, saved: Vec<SavedHostKey>) -> Arc<Self> {
        Self::with_addresses(session_id, saved, Vec::new())
    }

    pub fn with_addresses(session_id: impl Into<String>, saved: Vec<SavedHostKey>, addresses: Vec<String>) -> Arc<Self> {
        Arc::new(Self { session_id: session_id.into(), saved: Mutex::new(saved), addresses })
    }

    /// Whether `fingerprint` (bare) is saved for the session's server under
    /// another of its addresses than `host_id`, itself one of them. A jump
    /// host's key never counts for the server, nor the other way round.
    pub fn known_under_other_address(&self, host_id: &str, fingerprint: &str) -> bool {
        if !self.addresses.iter().any(|a| a == host_id) {
            return false;
        }
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        saved.iter().any(|k| k.host != host_id && self.addresses.contains(&k.host) && bare(&k.fingerprint) == fingerprint)
    }

    /// The bare fingerprint saved for `host_id`, if any.
    pub fn lookup(&self, host_id: &str) -> Option<String> {
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        saved.iter().find(|k| k.host == host_id).map(|k| bare(&k.fingerprint).to_string())
    }

    /// Keep `fingerprint` (bare) for `host_id` on the session: at once for
    /// the rest of this connection, and in its vault in the background, so
    /// the handshake does not wait on the store. The window is told when
    /// it is written, to show the session as it now is.
    pub fn save(&self, app: Option<&tauri::AppHandle>, host_id: &str, key_type: &str, fingerprint: &str) {
        let entry = SavedHostKey {
            host: host_id.to_string(),
            key_type: key_type.to_string(),
            fingerprint: with_prefix(fingerprint),
            saved_at: Some(now()),
        };
        {
            let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
            put(&mut saved, entry.clone());
        }
        let Some(app) = app.cloned() else { return };
        let session_id = self.session_id.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<crate::state::AppState>();
            match crate::ipc::session_commands::store_host_key(&state, &session_id, entry).await {
                Ok(()) => {
                    tracing::info!("Saved a host key on session {session_id}");
                    let _ = app.emit("session-host-keys-changed", &session_id);
                }
                Err(e) => tracing::warn!("Could not save the host key on session {session_id}: {e}"),
            }
        });
    }
}

/// Put `entry` in the list, in place of a key kept for the same server.
pub fn put(list: &mut Vec<SavedHostKey>, entry: SavedHostKey) {
    match list.iter_mut().find(|k| k.host == entry.host) {
        Some(k) => *k = entry,
        None => list.push(entry),
    }
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A key removed from a session (from the editor, or Remove all), kept as a
/// mark so that sync does not bring it back from a device that still has it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovedHostKey {
    /// The server, `host:port`, as in [`SavedHostKey::host`].
    pub host: String,
    /// When it was removed, in seconds since 1970.
    pub removed_at: i64,
}

/// The JSON fields of a session record that hold its keys.
pub const KEYS_FIELD: &str = "host_keys";
pub const REMOVED_FIELD: &str = "host_keys_removed";

fn list_of<T: serde::de::DeserializeOwned>(obj: &serde_json::Map<String, serde_json::Value>, field: &str) -> Vec<T> {
    obj.get(field).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// The keys and removal marks stored on a session record.
pub fn keys_in(obj: &serde_json::Map<String, serde_json::Value>) -> (Vec<SavedHostKey>, Vec<RemovedHostKey>) {
    (list_of(obj, KEYS_FIELD), list_of(obj, REMOVED_FIELD))
}

/// Store `keys` on a session record that held `before`: a server whose key
/// is gone gets a removal mark, one that has a key loses its mark. Empty
/// lists are no field.
pub fn set_keys(obj: &mut serde_json::Map<String, serde_json::Value>, before: &[SavedHostKey], keys: &[SavedHostKey]) {
    let (_, mut removed) = keys_in(obj);
    let at = now();
    for k in before {
        if !keys.iter().any(|n| n.host == k.host) {
            removed.retain(|r| r.host != k.host);
            removed.push(RemovedHostKey { host: k.host.clone(), removed_at: at });
        }
    }
    removed.retain(|r| !keys.iter().any(|k| k.host == r.host));
    write_lists(obj, keys, &removed);
}

fn write_lists(obj: &mut serde_json::Map<String, serde_json::Value>, keys: &[SavedHostKey], removed: &[RemovedHostKey]) {
    for (field, value) in [(KEYS_FIELD, serde_json::to_value(keys)), (REMOVED_FIELD, serde_json::to_value(removed))] {
        match value {
            Ok(serde_json::Value::Array(a)) if !a.is_empty() => {
                obj.insert(field.into(), serde_json::Value::Array(a));
            }
            _ => {
                obj.remove(field);
            }
        }
    }
}

/// Two copies of one session's keys, as two devices have them: for each
/// server the latest of what was saved and what was removed counts, so a
/// key saved on one device is not lost when the other writes the session
/// from a copy that does not have it yet (or with a build that does not
/// know host keys), and a key removed on purpose stays removed. A key
/// without a time (typed in by an older build) is older than any removal.
pub fn merge_keys(
    ours: (&[SavedHostKey], &[RemovedHostKey]),
    theirs: (&[SavedHostKey], &[RemovedHostKey]),
) -> (Vec<SavedHostKey>, Vec<RemovedHostKey>) {
    let mut keys: Vec<SavedHostKey> = Vec::new();
    for k in theirs.0.iter().chain(ours.0) {
        match keys.iter_mut().find(|n| n.host == k.host) {
            Some(n) if k.saved_at.unwrap_or(0) > n.saved_at.unwrap_or(0) => *n = k.clone(),
            Some(_) => {}
            None => keys.push(k.clone()),
        }
    }
    let mut removed: Vec<RemovedHostKey> = Vec::new();
    for r in theirs.1.iter().chain(ours.1) {
        match removed.iter_mut().find(|n| n.host == r.host) {
            Some(n) => n.removed_at = n.removed_at.max(r.removed_at),
            None => removed.push(r.clone()),
        }
    }
    keys.retain(|k| !removed.iter().any(|r| r.host == k.host && r.removed_at >= k.saved_at.unwrap_or(0)));
    removed.retain(|r| !keys.iter().any(|k| k.host == r.host));
    (keys, removed)
}

/// Merge the keys of `theirs`, another copy of the same session record, into
/// `ours`. Returns whether `ours` changed. Not a session object: nothing.
pub fn merge_into(ours: &mut serde_json::Value, theirs: &serde_json::Value) -> bool {
    let (Some(o), Some(t)) = (ours.as_object_mut(), theirs.as_object()) else { return false };
    let (ok, or) = keys_in(o);
    let (tk, tr) = keys_in(t);
    let (keys, removed) = merge_keys((&ok, &or), (&tk, &tr));
    let same = |a: &[SavedHostKey], b: &[SavedHostKey]| a.len() == b.len() && a.iter().all(|k| b.contains(k));
    let same_r = |a: &[RemovedHostKey], b: &[RemovedHostKey]| a.len() == b.len() && a.iter().all(|k| b.contains(k));
    if same(&keys, &ok) && same_r(&removed, &or) {
        return false;
    }
    write_lists(o, &keys, &removed);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(host: &str, fp: &str, at: i64) -> SavedHostKey {
        SavedHostKey { host: host.into(), key_type: "ssh-ed25519".into(), fingerprint: with_prefix(fp), saved_at: Some(at) }
    }
    fn gone(host: &str, at: i64) -> RemovedHostKey {
        RemovedHostKey { host: host.into(), removed_at: at }
    }

    #[test]
    fn sync_keeps_a_key_the_other_copy_lacks() {
        // Saved here; the other device wrote the session without it.
        let (keys, removed) = merge_keys((&[k("a.example.com:22", "A", 10)], &[]), (&[], &[]));
        assert_eq!(keys, vec![k("a.example.com:22", "A", 10)]);
        assert!(removed.is_empty());
        // And the other way round.
        let (keys, _) = merge_keys((&[], &[]), (&[k("a.example.com:22", "A", 10)], &[]));
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn sync_keeps_the_newer_key_and_honours_removals() {
        let old = k("a.example.com:22", "OLD", 10);
        let new = k("a.example.com:22", "NEW", 20);
        assert_eq!(merge_keys((std::slice::from_ref(&old), &[]), (std::slice::from_ref(&new), &[])).0, vec![new.clone()]);
        assert_eq!(merge_keys((std::slice::from_ref(&new), &[]), (std::slice::from_ref(&old), &[])).0, vec![new.clone()]);
        // Removed after it was saved: stays removed, and the mark is kept.
        let (keys, removed) = merge_keys((std::slice::from_ref(&old), &[]), (&[], &[gone("a.example.com:22", 15)]));
        assert!(keys.is_empty());
        assert_eq!(removed, vec![gone("a.example.com:22", 15)]);
        // Saved again after the removal: the key is back, the mark gone.
        let (keys, removed) = merge_keys((std::slice::from_ref(&new), &[]), (&[], &[gone("a.example.com:22", 15)]));
        assert_eq!(keys, vec![new]);
        assert!(removed.is_empty());
        // A key without a time is older than any removal.
        let untimed = SavedHostKey { saved_at: None, ..old };
        assert!(merge_keys((&[untimed], &[]), (&[], &[gone("a.example.com:22", 1)])).0.is_empty());
    }

    #[test]
    fn removal_marks_on_the_record() {
        let mut v = serde_json::json!({ "id": "s", "host_keys": [k("a.example.com:22", "A", 1), k("b.example.com:22", "B", 1)] });
        let o = v.as_object_mut().unwrap();
        let (before, _) = keys_in(o);
        set_keys(o, &before, &[k("b.example.com:22", "B", 1)]);
        let (keys, removed) = keys_in(o);
        assert_eq!(keys.len(), 1);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].host, "a.example.com:22");
        // Saving that server again drops its mark; nothing left is no field.
        let (before, _) = keys_in(o);
        set_keys(o, &before, &[k("a.example.com:22", "A", 2)]);
        let (_, removed) = keys_in(o);
        assert_eq!(removed.iter().map(|r| r.host.as_str()).collect::<Vec<_>>(), vec!["b.example.com:22"]);
        set_keys(o, &[], &[]);
        let mut w = serde_json::json!({ "id": "s" });
        assert!(!merge_into(&mut w, &serde_json::json!({ "id": "s" })), "nothing to merge changes nothing");
        assert!(merge_into(&mut w, &v));
        assert_eq!(keys_in(w.as_object().unwrap()).1.len(), 1);
    }

    #[test]
    fn answers_from_the_dialog() {
        assert_eq!(Answer::from_reply(false, Some(true)), Answer::Reject);
        assert_eq!(Answer::from_reply(true, Some(false)), Answer::Once);
        assert_eq!(Answer::from_reply(true, Some(true)), Answer::Save);
        // An older window only knew Accept, which saved.
        assert_eq!(Answer::from_reply(true, None), Answer::Save);
    }

    #[test]
    fn typed_fingerprints() {
        let b = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789+/AbCdE";
        assert_eq!(b.len(), 43);
        assert_eq!(parse_typed(&format!("SHA256:{b}")).as_deref(), Some(format!("SHA256:{b}").as_str()));
        assert_eq!(parse_typed(&format!("  {b}= ")).as_deref(), Some(format!("SHA256:{b}").as_str()));
        assert_eq!(parse_typed("SHA256:short"), None);
        assert_eq!(parse_typed(&format!("MD5:{b}")), None);
        assert_eq!(parse_typed(&format!("SHA256:{}", b.replace('A', "-"))), None);
    }

    #[test]
    fn a_key_counts_for_every_address_of_the_server() {
        let key = |host: &str, fp: &str| SavedHostKey { host: host.into(), key_type: String::new(), fingerprint: with_prefix(fp), saved_at: None };
        let t = SessionTrust::with_addresses(
            "s1",
            vec![key("10.0.0.5:22", "AAA"), key("bastion.example.com:22", "JJJ")],
            vec!["10.0.0.5:22".into(), "server.example.com:2222".into()],
        );
        assert!(t.known_under_other_address("server.example.com:2222", "AAA"));
        // Another key under the other address: asked about, as a new one.
        assert!(!t.known_under_other_address("server.example.com:2222", "BBB"));
        // A jump host's key is not the server's.
        assert!(!t.known_under_other_address("server.example.com:2222", "JJJ"));
        // Nor is an address outside the session's.
        assert!(!t.known_under_other_address("other.example.com:22", "AAA"));
    }

    #[test]
    fn one_key_per_server() {
        let t = SessionTrust::new("s1", vec![]);
        t.save(None, "example.com:22", "ssh-ed25519", "aaa");
        t.save(None, "jump.example.com:22", "ssh-ed25519", "bbb");
        t.save(None, "example.com:22", "rsa-sha2-512", "SHA256:ccc");
        assert_eq!(t.lookup("example.com:22").as_deref(), Some("ccc"));
        assert_eq!(t.lookup("jump.example.com:22").as_deref(), Some("bbb"));
        assert_eq!(t.lookup("example.com:2222"), None);
        assert_eq!(t.saved.lock().unwrap().len(), 2);
    }
}
