//! A vault as one file, for the stores that hold files (see
//! [`crate::vault::sync_store`]), and the sync against it.
//!
//! The file is JSON: the vault's header and members in the clear, as Turso
//! holds them (a device needs them to unlock the vault, and they carry no
//! secret: the master key in them is wrapped), and a sealed body. The body
//! holds a counter and every record, each sealed on its own as for Turso
//! (see [`crate::vault::sync_record`]). The body's seal binds the header, so
//! a header changed by anyone else does not open, and the counter lets a
//! device notice a store that serves an older file than one it has read.
//!
//! A sync reads the file, merges every record into the local file, and, if
//! this device has changes, writes the file back with them: only if nobody
//! wrote it meanwhile, otherwise it reads and merges again.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use libsql::{Connection, Value};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::vault::error::VaultError;
use crate::vault::sync_engine::{self, merge, mirror_header, outgoing, sent, Report, Target};
use crate::vault::sync_local;
use crate::vault::sync_record::RecordKey;
use crate::vault::sync_store::Store;

const FORMAT: u8 = 1;
const BODY_LABEL: &str = "reach-bundle-v1";

/// `sync_state`: the highest counter read or written.
pub const STATE_COUNTER: &str = "bundle_counter";

/// A deletion is kept this long, for devices that have not synced since.
/// One that comes back later still loses the record: what it holds, unchanged
/// and missing from the file, was deleted elsewhere.
const TOMBSTONE_MS: i64 = 90 * 24 * 3600 * 1000;

const ATTEMPTS: usize = 4;

/// The file as stored.
#[derive(Serialize, Deserialize)]
struct File {
    format: u8,
    header: Vec<serde_json::Value>,
    #[serde(default)]
    members: Vec<Vec<serde_json::Value>>,
    body: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Body {
    counter: i64,
    /// Remote id and sealed record, base64.
    records: Vec<(String, String)>,
}

/// A header row and its member rows, as the local file stores them.
pub type HeaderRows = (Vec<Value>, Vec<Vec<Value>>);

fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Integer(i) => serde_json::json!({ "i": i }),
        Value::Real(r) => serde_json::json!({ "r": r }),
        Value::Text(t) => serde_json::json!({ "t": t }),
        Value::Blob(b) => serde_json::json!({ "b": BASE64.encode(b) }),
    }
}

fn from_json(v: &serde_json::Value) -> Result<Value, VaultError> {
    let bad = || VaultError::SerializationError("the vault file's header is damaged".into());
    if v.is_null() {
        return Ok(Value::Null);
    }
    let obj = v.as_object().ok_or_else(bad)?;
    if let Some(i) = obj.get("i") {
        return i.as_i64().map(Value::Integer).ok_or_else(bad);
    }
    if let Some(r) = obj.get("r") {
        return r.as_f64().map(Value::Real).ok_or_else(bad);
    }
    if let Some(t) = obj.get("t") {
        return t.as_str().map(|s| Value::Text(s.to_string())).ok_or_else(bad);
    }
    if let Some(b) = obj.get("b") {
        let s = b.as_str().ok_or_else(bad)?;
        return BASE64.decode(s).map(Value::Blob).map_err(|_| bad());
    }
    Err(bad())
}

fn aad(vault_id: &str, header: &[serde_json::Value], members: &[Vec<serde_json::Value>]) -> Result<Vec<u8>, VaultError> {
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(header)?);
    digest.update([0]);
    digest.update(serde_json::to_vec(members)?);
    let mut out = vec![FORMAT];
    out.extend_from_slice(BODY_LABEL.as_bytes());
    out.push(0);
    out.extend_from_slice(vault_id.as_bytes());
    out.push(0);
    out.extend_from_slice(&digest.finalize());
    Ok(out)
}

fn parse(data: &[u8]) -> Result<File, VaultError> {
    let file: File = serde_json::from_slice(data)
        .map_err(|_| VaultError::SerializationError("the sync location holds a file Reacher CM cannot read".into()))?;
    if file.format != FORMAT {
        return Err(VaultError::SyncError(
            "The vault file was written by a newer Reacher CM. Update Reacher CM on this device.".into(),
        ));
    }
    Ok(file)
}

/// The header and members a vault file holds; no key is needed for them.
pub fn read_header(data: &[u8]) -> Result<HeaderRows, VaultError> {
    let file = parse(data)?;
    let header = file.header.iter().map(from_json).collect::<Result<Vec<_>, _>>()?;
    if header.len() != 7 {
        return Err(VaultError::SerializationError("the vault file's header is damaged".into()));
    }
    let members = file
        .members
        .iter()
        .map(|m| m.iter().map(from_json).collect::<Result<Vec<_>, _>>())
        .collect::<Result<Vec<_>, _>>()?;
    Ok((header, members))
}

fn header_id(header: &[Value]) -> Option<&str> {
    match header.first() {
        Some(Value::Text(id)) => Some(id),
        _ => None,
    }
}

/// Write a vault file.
fn seal(key: &RecordKey, rows: &HeaderRows, body: &Body) -> Result<Vec<u8>, VaultError> {
    let header: Vec<_> = rows.0.iter().map(to_json).collect();
    let members: Vec<Vec<_>> = rows.1.iter().map(|m| m.iter().map(to_json).collect()).collect();
    let body_key = key.subkey(BODY_LABEL)?;
    let plain = Zeroizing::new(serde_json::to_vec(body)?);
    let nonce: [u8; 24] = rand::random();
    let ct = XChaCha20Poly1305::new((&*body_key).into())
        .encrypt(&XNonce::from(nonce), Payload { msg: &plain, aad: &aad(key.vault_id(), &header, &members)? })
        .map_err(|e| VaultError::EncryptionError(e.to_string()))?;
    let mut sealed = Vec::with_capacity(24 + ct.len());
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&ct);
    let file = File { format: FORMAT, header, members, body: BASE64.encode(sealed) };
    Ok(serde_json::to_vec(&file)?)
}

/// Read a vault file: its header and members, and its body.
fn open(key: &RecordKey, data: &[u8]) -> Result<(HeaderRows, Body), VaultError> {
    let rows = read_header(data)?;
    if header_id(&rows.0) != Some(key.vault_id()) {
        return Err(VaultError::SyncError(
            "The sync location holds another vault than this one. Choose an empty folder or repository.".into(),
        ));
    }
    let file = parse(data)?;
    let sealed = BASE64
        .decode(&file.body)
        .map_err(|_| VaultError::SerializationError("the vault file is damaged".into()))?;
    if sealed.len() < 24 {
        return Err(VaultError::SerializationError("the vault file is damaged".into()));
    }
    let body_key = key.subkey(BODY_LABEL)?;
    let nonce: [u8; 24] = sealed[..24].try_into().expect("checked length");
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new((&*body_key).into())
            .decrypt(&XNonce::from(nonce), Payload { msg: &sealed[24..], aad: &aad(key.vault_id(), &file.header, &file.members)? })
            .map_err(|_| VaultError::DecryptionError("the vault file does not open with this vault's key, or was changed".into()))?,
    );
    let body: Body = serde_json::from_slice(&plain)?;
    Ok((rows, body))
}

#[cfg(desktop)]
pub(super) fn reencrypt_row(
    row: &mut sync_local::Row,
    previous: &super::types::Dek,
    next: &super::types::Dek,
) -> Result<(), VaultError> {
    use super::{crypto, types::EncryptedPayload};
    use secrecy::ExposeSecret;

    let nonce = row.nonce.as_slice().try_into()
        .map_err(|_| VaultError::DecryptionError("Invalid record nonce".into()))?;
    let previous_payload = EncryptedPayload {
        nonce,
        ciphertext: std::mem::take(&mut row.ciphertext),
        wrapped_dek: serde_json::from_str(&row.wrapped_dek)?,
    };
    let plain = crypto::decrypt_secret(previous, &previous_payload)?;
    let payload = crypto::encrypt_secret(next, plain.expose_secret())?;
    row.nonce = payload.nonce.to_vec();
    row.ciphertext = payload.ciphertext;
    row.wrapped_dek = serde_json::to_string(&payload.wrapped_dek)?;
    Ok(())
}

#[cfg(desktop)]
pub(super) fn rotate(
    vault: &str,
    data: &[u8],
    previous: &super::types::Dek,
    next: &super::types::Dek,
) -> Result<Vec<u8>, VaultError> {
    let previous_key = RecordKey::derive(previous, vault)?;
    let next_key = RecordKey::derive(next, vault)?;
    let (mut rows, mut body) = open(&previous_key, data)?;
    if !rows.1.is_empty() {
        return Err(VaultError::SyncError("Invalid organization vault member header".into()));
    }
    // Rotated keys use an owner envelope on the server, including when the
    // original creator no longer owns the vault.
    rows.0[6] = Value::Text("null".into());
    body.counter = body.counter.checked_add(1)
        .ok_or_else(|| VaultError::SyncError("Invalid vault bundle counter".into()))?;
    for (id, sealed) in &mut body.records {
        let bytes = BASE64.decode(&*sealed)
            .map_err(|_| VaultError::DecryptionError("Invalid sealed record".into()))?;
        let mut record = previous_key.open(id, &bytes)?;
        if let Some(row) = &mut record.row {
            reencrypt_row(row, previous, next)?;
        }
        *id = next_key.remote_id(&record.id);
        *sealed = BASE64.encode(next_key.seal(&record)?);
    }
    seal(&next_key, &rows, &body)
}

pub(crate) fn verify(key: &RecordKey, data: &[u8]) -> Result<HeaderRows, VaultError> {
    open(key, data).map(|(rows, _)| rows)
}

async fn counter_seen(local: &Connection) -> Result<i64, VaultError> {
    Ok(sync_local::state(local, STATE_COUNTER).await?.and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Sync a vault with the file in `store`.
pub async fn sync(target: &Target, local: &Connection, store: &mut Store) -> Result<Report, VaultError> {
    let mut report = Report::default();
    #[cfg(desktop)]
    let organization = matches!(&target.backend, sync_engine::Backend::Account(account) if account.organization.is_some());
    #[cfg(desktop)]
    if let Store::Account(account) = store {
        account.key_epoch = sync_local::state(local, sync_local::KEY_EPOCH_STATE)
            .await?.and_then(|value| value.parse().ok()).unwrap_or(0);
    }
    #[cfg(desktop)]
    let read_only = if organization {
        let mut roles = local.query("SELECT role FROM organization_vault_access WHERE singleton = 1", ()).await?;
        let role = roles.next().await?
            .ok_or_else(|| VaultError::AccessDenied("Open the organization vault from Settings → Account".into()))?
            .get::<String>(0)?;
        let pending = sync_local::state(local, sync_local::ROTATION_PENDING_STATE).await?;
        role == "viewer" || pending.as_deref() == Some("1")
    } else {
        false
    };
    #[cfg(not(desktop))]
    let read_only = false;
    sync_local::set_state(local, sync_engine::STATE_FORMAT, "file").await?;
    for _ in 0..ATTEMPTS {
        let seen = counter_seen(local).await?;
        let (rows, body, revision) = match store.get().await? {
            Some((data, revision)) => {
                let (rows, body) = open(&target.key, &data)?;
                #[cfg(desktop)]
                if organization && !matches!(
                    rows.0.get(1),
                    Some(Value::Text(name)) if crate::vault::manager::valid_organization_vault_name(name)
                ) {
                    return Err(VaultError::SyncError("Invalid organization vault name".into()));
                }
                (rows, body, Some(revision))
            }
            None if read_only => return Err(VaultError::AccessDenied("This organization vault is unavailable".into())),
            None => {
                // Nothing there yet (or no longer): set it up from here.
                if seen > 0 {
                    tracing::warn!(vault = %target.vault_id, "The vault file is gone from its sync location; writing it again");
                }
                sync_local::mark_all_dirty(local).await?;
                (sync_engine::turso::read_header(local).await?, Body::default(), None)
            }
        };
        if revision.is_some() {
            mirror_header(local, &rows.0, &rows.1).await?;
        }

        // An older file than one read before: a backup put back, or a store
        // that lags. Nothing missing from it is taken as deleted, and all
        // this device holds is written back.
        let rolled_back = revision.is_some() && body.counter < seen;
        if rolled_back {
            tracing::warn!(vault = %target.vault_id, "The sync location holds an older vault file than this device has read; writing it again");
            if !read_only {
                sync_local::mark_all_dirty(local).await?;
            }
        }

        let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut tombstones: BTreeMap<String, i64> = BTreeMap::new();
        let mut records = Vec::new();
        let mut unreadable = false;
        for (remote_id, sealed) in &body.records {
            let Ok(sealed) = BASE64.decode(sealed) else {
                unreadable = true;
                continue;
            };
            match target.key.open(remote_id, &sealed) {
                Ok(record) => {
                    if record.row.is_none() {
                        tombstones.insert(remote_id.clone(), record.version);
                    }
                    records.push(record);
                }
                // Kept as it is: one record that does not open must not stop
                // the rest, nor be lost for a device that can open it.
                Err(e) => {
                    unreadable = true;
                    tracing::error!(vault = %target.vault_id, "Skipping a record: {}", e);
                }
            }
            entries.insert(remote_id.clone(), sealed);
        }
        let merged = merge(target, local, records, revision.is_some() && !rolled_back).await?;
        report.pulled += merged.pulled;
        report.conflicts += merged.conflicts;

        let out = if read_only { Vec::new() } else { outgoing(target, local).await? };
        if read_only || (out.is_empty() && revision.is_some()) {
            sync_local::set_state(local, STATE_COUNTER, &body.counter.max(seen).to_string()).await?;
            if !unreadable && !rolled_back && matches!(store, Store::Account(_)) {
                report.applied_revision = revision;
            }
            return Ok(report);
        }
        for (_, record) in &out {
            let remote_id = target.key.remote_id(&record.id);
            entries.insert(remote_id.clone(), target.key.seal(record)?);
            match record.row {
                None => tombstones.insert(remote_id, record.version),
                Some(_) => tombstones.remove(&remote_id),
            };
        }
        let horizon = sync_engine::now_ms() - TOMBSTONE_MS;
        entries.retain(|id, _| tombstones.get(id).is_none_or(|v| *v >= horizon));

        let counter = body.counter.max(seen) + 1;
        let new_body = Body {
            counter,
            records: entries.into_iter().map(|(id, sealed)| (id, BASE64.encode(sealed))).collect(),
        };
        let data = seal(&target.key, &sync_engine::turso::read_header(local).await?, &new_body)?;
        if store.put(&data, revision.as_deref()).await? {
            sent(local, &out).await?;
            sync_local::set_state(local, STATE_COUNTER, &counter.to_string()).await?;
            report.pushed += out.len();
            if !unreadable && matches!(store, Store::Account(_)) {
                // Account bundle ETags are the SHA-256 of the uploaded bytes.
                report.applied_revision = Some(format!("\"{}\"", super::sync_store::revision_of(&data)));
            }
            return Ok(report);
        }
        // Another device wrote first: read its file and merge again.
    }
    Err(VaultError::SyncError("other devices kept changing the vault; try again".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::types::Dek;

    fn key(vault: &str) -> RecordKey {
        RecordKey::derive(&Dek::from_fn(|k| k.fill(3)), vault).unwrap()
    }

    fn rows(vault: &str) -> HeaderRows {
        (
            vec![
                Value::Text(vault.into()),
                Value::Text("__personal__".into()),
                Value::Blob(vec![1; 32]),
                Value::Text("me".into()),
                Value::Integer(5),
                Value::Text("\"Private\"".into()),
                Value::Text("{}".into()),
            ],
            vec![],
        )
    }

    #[test]
    fn a_file_opens_with_its_key_and_header_only() {
        let k = key("v1");
        let body = Body { counter: 3, records: vec![("r".into(), "AAAA".into())] };
        let data = seal(&k, &rows("v1"), &body).unwrap();
        let (back, opened) = open(&k, &data).unwrap();
        assert_eq!(back, rows("v1"));
        assert_eq!(opened.counter, 3);
        assert_eq!(read_header(&data).unwrap(), rows("v1"));

        assert!(open(&key("v2"), &data).is_err(), "another vault's key");
        let mut file: serde_json::Value = serde_json::from_slice(&data).unwrap();
        file["header"][1] = serde_json::json!({ "t": "renamed" });
        assert!(open(&k, &serde_json::to_vec(&file).unwrap()).is_err(), "a header changed by someone else");
    }
}
