//! The engine against a "Turso" that is a local libsql file: the SQL is the
//! same, so every rule of the merge and both formats are exercised without a
//! server.

use super::*;
use secrecy::ExposeSecret;

struct World {
    dir: PathBuf,
    remote: Connection,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn master() -> Dek {
    Dek::from_fn(|k| k.fill(42))
}

const VAULT: &str = "vault-1";

async fn world(legacy: bool) -> World {
    let dir = std::env::temp_dir().join(format!("reach-engine-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let remote = open_local(&dir.join("remote.db")).await.unwrap();
    crate::vault::schema::init_schema(&remote).await.unwrap();
    remote
        .execute(
            "INSERT INTO vault_header (id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek) VALUES (?, '__personal__', ?, 'me', 1, '\"Private\"', '{}')",
            (VAULT, vec![0u8; 32]),
        )
        .await
        .unwrap();
    if !legacy {
        turso::init_records(&remote).await.unwrap();
    }
    World { dir, remote }
}

struct Device {
    target: Target,
    local: Connection,
}

async fn device(w: &World, name: &str) -> Device {
    let path = w.dir.join(format!("{name}.db"));
    let local = open_local(&path).await.unwrap();
    crate::vault::schema::init_schema(&local).await.unwrap();
    sync_local::init(&local).await.unwrap();
    let target = Target {
        vault_id: VAULT.into(),
        local_path: path,
        backend: Backend::Turso { url: String::new(), token: String::new() },
        master: master(),
        key: RecordKey::derive(&master(), VAULT).unwrap(),
        device: name.into(),
        my_uuid: "me".into(),
    };
    Device { target, local }
}

impl Device {
    async fn sync(&self, w: &World) -> Report {
        turso::sync(&self.target, &self.local, &w.remote).await.unwrap()
    }

    async fn put(&self, id: &str, name: &str) {
        let json = serde_json::json!({ "id": id, "name": name }).to_string();
        let p = encrypt_secret(&self.target.master, json.as_bytes()).unwrap();
        self.local
            .execute(
                "INSERT OR REPLACE INTO secrets (id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at) VALUES (?, ?, 'session', ?, ?, ?, 1, 2)",
                (id, name, p.nonce.as_slice(), p.ciphertext.as_slice(), serde_json::to_string(&p.wrapped_dek).unwrap()),
            )
            .await
            .unwrap();
    }

    /// A session with a payload of its own.
    async fn put_json(&self, id: &str, json: serde_json::Value) {
        let p = encrypt_secret(&self.target.master, json.to_string().as_bytes()).unwrap();
        self.local
            .execute(
                "INSERT OR REPLACE INTO secrets (id, name, category, nonce, ciphertext, wrapped_dek, created_at, updated_at) VALUES (?, ?, 'session', ?, ?, ?, 1, 2)",
                (id, id, p.nonce.as_slice(), p.ciphertext.as_slice(), serde_json::to_string(&p.wrapped_dek).unwrap()),
            )
            .await
            .unwrap();
    }

    async fn payload(&self, id: &str) -> serde_json::Value {
        let row = sync_local::read_row(&self.local, id).await.unwrap().unwrap();
        open_row(&self.target.master, &row).unwrap()
    }

    async fn delete(&self, id: &str) {
        self.local.execute("DELETE FROM secrets WHERE id = ?", [id]).await.unwrap();
    }

    /// Every session's payload name, by row name order.
    async fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for row in sync_local::all_rows(&self.local).await.unwrap() {
            let payload = EncryptedPayload {
                nonce: row.nonce.try_into().unwrap(),
                ciphertext: row.ciphertext,
                wrapped_dek: serde_json::from_str(&row.wrapped_dek).unwrap(),
            };
            let plain = decrypt_secret(&self.target.master, &payload).unwrap();
            let v: serde_json::Value = serde_json::from_slice(plain.expose_secret()).unwrap();
            out.push(v["name"].as_str().unwrap().to_string());
        }
        out.sort();
        out
    }

    /// Make this device's clock run ahead, so its changes are newer.
    async fn later(&self, ms: i64) {
        self.local.execute("UPDATE sync_dirty SET changed_at = changed_at + ?", [ms]).await.unwrap();
    }
}

#[tokio::test]
async fn a_restore_keeps_overwritten_offline_edits_as_conflict_copies() {
    let world = world(false).await;
    let device = device(&world, "offline").await;
    device.put("original", "my offline edit").await;
    device.put("removed", "my offline edit to a removed record").await;
    device.put("new", "my offline addition").await;
    let restored = encrypt_secret(&device.target.master, br#"{"id":"original","name":"snapshot value"}"#).unwrap();
    let now = now_ms();
    let incoming = vec![
        Record {
            id: "original".into(), version: now + 1, device: "owner".into(),
            row: Some(Row {
                id: "original".into(), name: "snapshot value".into(), category: "session".into(),
                nonce: restored.nonce.to_vec(), ciphertext: restored.ciphertext,
                wrapped_dek: serde_json::to_string(&restored.wrapped_dek).unwrap(), created_at: 1, updated_at: 2,
            }),
        },
        Record { id: "removed".into(), version: now + 1, device: "owner".into(), row: None },
    ];
    let tx = device.local.transaction_with_behavior(TransactionBehavior::Immediate).await.unwrap();
    let report = merge_records(&device.target, &tx, incoming, true, true).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(report.conflicts, 2);
    assert_eq!(device.payload("original").await["name"], "snapshot value");
    assert!(sync_local::read_row(&device.local, "removed").await.unwrap().is_none());
    let names = device.names().await;
    assert!(names.iter().any(|name| name.starts_with("my offline edit (conflict copy")));
    assert!(names.iter().any(|name| name.starts_with("my offline edit to a removed record (conflict copy")));
    assert!(names.iter().any(|name| name == "my offline addition"));
}

#[tokio::test]
async fn a_change_reaches_the_other_device() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put("s1", "web").await;
    assert_eq!(a.sync(&w).await.pushed, 1);
    assert_eq!(b.sync(&w).await.pulled, 1);
    assert_eq!(b.names().await, vec!["web"]);
    assert_eq!(sync_local::dirty_count(&b.local).await.unwrap(), 0, "what came in is not sent back");

    b.put("s1", "web-renamed").await;
    b.sync(&w).await;
    a.sync(&w).await;
    assert_eq!(a.names().await, vec!["web-renamed"]);

    a.delete("s1").await;
    a.sync(&w).await;
    b.sync(&w).await;
    assert!(b.names().await.is_empty());
}

#[tokio::test]
async fn the_backend_sees_no_names() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    a.put("app_settings", "prod-db.example.com").await;
    a.sync(&w).await;
    let mut rows = w.remote.query("SELECT id, sealed FROM reach_records", ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let id: String = row.get(0).unwrap();
    let sealed: Vec<u8> = row.get(1).unwrap();
    assert_ne!(id, "app_settings");
    assert!(!sealed.windows(7).any(|x| x == b"prod-db"));
    assert!(!sealed.windows(7).any(|x| x == b"session"));
}

#[tokio::test]
async fn both_edited_the_newer_wins_and_the_other_is_kept_as_a_copy() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put("s1", "web").await;
    a.sync(&w).await;
    b.sync(&w).await;

    a.put("s1", "web from a").await;
    b.put("s1", "web from b").await;
    a.later(5_000).await;
    assert_eq!(a.sync(&w).await.conflicts, 0); // a's newer edit reaches the backend first
    assert_eq!(b.sync(&w).await.conflicts, 1); // b's older, unsent edit loses and is kept

    let names = b.names().await;
    assert_eq!(names.len(), 2);
    assert_eq!(names[0], "web from a");
    assert!(names[1].starts_with("web from b (conflict copy "), "{names:?}");
    a.sync(&w).await;
    let names = a.names().await;
    assert_eq!(b.names().await, names, "the copy syncs like any record");
    let rows = sync_local::all_rows(&b.local).await.unwrap();
    assert!(rows.iter().any(|r| r.name.starts_with("web from b (conflict copy ")), "the row name says so too");
}

#[tokio::test]
async fn a_conflict_copy_survives_a_first_full_read() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put("s1", "web from a").await;
    b.put("s1", "web from b").await; // never synced, so b reads everything
    a.later(5_000).await;
    a.sync(&w).await;
    assert_eq!(b.sync(&w).await.conflicts, 1);
    let names = b.names().await;
    assert_eq!(names.len(), 2, "{names:?}");
    a.sync(&w).await;
    assert_eq!(a.names().await, names);
}

#[tokio::test]
async fn an_edit_beats_a_delete() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put("s1", "web").await;
    a.sync(&w).await;
    b.sync(&w).await;

    a.delete("s1").await;
    a.later(5_000).await; // even a later delete
    b.put("s1", "web edited").await;
    a.sync(&w).await;
    b.sync(&w).await;
    a.sync(&w).await;
    assert_eq!(a.names().await, vec!["web edited"]);
    assert_eq!(b.names().await, vec!["web edited"]);
}

#[tokio::test]
async fn an_older_copy_is_not_taken() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put("s1", "v1").await;
    a.sync(&w).await;
    let mut rows = w.remote.query("SELECT sealed FROM reach_records", ()).await.unwrap();
    let old: Vec<u8> = rows.next().await.unwrap().unwrap().get(0).unwrap();
    drop(rows);
    a.put("s1", "v2").await;
    a.later(1_000).await;
    a.sync(&w).await;
    b.sync(&w).await;
    assert_eq!(b.names().await, vec!["v2"]);

    // The backend serves the old version again under a new sequence number.
    w.remote.execute("UPDATE reach_records SET sealed = ?, seq = seq + 10", [old]).await.unwrap();
    b.sync(&w).await;
    assert_eq!(b.names().await, vec!["v2"]);
}

#[tokio::test]
async fn a_push_after_someone_elses_is_merged_first() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put("s1", "one").await;
    b.put("s2", "two").await;
    a.sync(&w).await;
    b.sync(&w).await; // b's base is out of date: pulls a's, then pushes
    a.sync(&w).await;
    assert_eq!(a.names().await, vec!["one", "two"]);
    assert_eq!(b.names().await, vec!["one", "two"]);
}

#[tokio::test]
async fn a_stale_local_file_takes_the_backend_as_it_is() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    a.put("s1", "kept").await;
    a.sync(&w).await;
    let b = device(&w, "b").await;
    // Left over from an old version, never synced, no change recorded.
    let stale = Row { id: "old".into(), ..sync_local::all_rows(&a.local).await.unwrap().remove(0) };
    sync_local::apply(&b.local, "old", Some(&stale)).await.unwrap();
    b.sync(&w).await;
    assert_eq!(b.names().await, vec!["kept"]);
}

#[tokio::test]
async fn legacy_then_upgrade() {
    let w = world(true).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;

    // Legacy: rows go to the old table, readable by older versions.
    a.put("s1", "web").await;
    assert!(a.sync(&w).await.legacy);
    let mut rows = w.remote.query("SELECT name FROM secrets", ()).await.unwrap();
    assert_eq!(rows.next().await.unwrap().unwrap().get::<String>(0).unwrap(), "web");
    drop(rows);
    b.sync(&w).await;
    assert_eq!(b.names().await, vec!["web"]);

    // An older version deletes it.
    w.remote.execute("DELETE FROM secrets WHERE id = 's1'", ()).await.unwrap();
    b.put("s2", "db").await;
    b.sync(&w).await;
    assert_eq!(b.names().await, vec!["db"]);

    // Upgrade on b; a follows on its next sync.
    turso::upgrade(&b.target, &b.local, &w.remote).await.unwrap();
    assert!(turso::is_record_format(&w.remote).await.unwrap());
    let report = a.sync(&w).await;
    assert!(!report.legacy);
    assert_eq!(a.names().await, vec!["db"]);
    a.put("s3", "cache").await;
    a.sync(&w).await;
    b.sync(&w).await;
    assert_eq!(b.names().await, vec!["cache", "db"]);
    let mut rows = w.remote.query("SELECT name FROM sqlite_master WHERE name LIKE 'secrets_legacy_%'", ()).await.unwrap();
    assert!(rows.next().await.unwrap().is_some(), "the old table is kept under another name");
}

#[tokio::test]
async fn the_header_is_mirrored_for_offline_unlock() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    a.sync(&w).await;
    let mut rows = a.local.query("SELECT id, name FROM vault_header", ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), VAULT);
}

#[tokio::test]
async fn an_empty_backend_is_set_up_from_the_device() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    a.put("s1", "web").await;
    a.sync(&w).await;

    // Sync moves to a new, empty database.
    let fresh = open_local(&w.dir.join("fresh.db")).await.unwrap();
    let moved = World { dir: w.dir.join("unused"), remote: fresh };
    let report = a.sync(&moved).await;
    assert_eq!(report.pushed, 1);
    assert!(turso::is_record_format(&moved.remote).await.unwrap());
    let b = device(&w, "b").await;
    b.sync(&moved).await;
    assert_eq!(b.names().await, vec!["web"]);
}

#[test]
fn a_backend_is_named_without_its_token() {
    let b = Backend::Turso { url: "libsql://DB-org.turso.io/".into(), token: "secret".into() };
    assert_eq!(b.identity(), "turso:db-org.turso.io");
    let same = Backend::Turso { url: "https://db-org.turso.io".into(), token: "other".into() };
    assert_eq!(b.identity(), same.identity());
}

// ---------------------------------------------------------------------------
// File backends: the same rules, the vault as one file in a folder or a Git
// repository.

struct FileWorld {
    dir: PathBuf,
}

impl Drop for FileWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn file_world() -> FileWorld {
    let dir = std::env::temp_dir().join(format!("reach-files-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("share")).unwrap();
    FileWorld { dir }
}

/// A device syncing to `url`; `first` holds the vault header, as the device
/// that created the vault does.
async fn file_device(w: &FileWorld, name: &str, url: &str, first: bool) -> Device {
    let path = w.dir.join(format!("{name}.db"));
    let local = open_local(&path).await.unwrap();
    crate::vault::schema::init_schema(&local).await.unwrap();
    sync_local::init(&local).await.unwrap();
    if first {
        local
            .execute(
                "INSERT INTO vault_header (id, name, salt, user_uuid, created_at, vault_type, wrapped_master_dek) VALUES (?, '__personal__', ?, 'me', 1, '\"Private\"', '{}')",
                (VAULT, vec![0u8; 32]),
            )
            .await
            .unwrap();
    }
    let target = Target {
        vault_id: VAULT.into(),
        local_path: path,
        backend: Backend::parse(url, "", &w.dir.join(format!("{name}-app"))).unwrap(),
        master: master(),
        key: RecordKey::derive(&master(), VAULT).unwrap(),
        device: name.into(),
        my_uuid: "me".into(),
    };
    Device { target, local }
}

impl Device {
    async fn sync_file(&self) -> Report {
        sync(&self.target).await.unwrap()
    }
}

async fn the_rules_hold(w: &FileWorld, url: &str) {
    let a = file_device(w, "a", url, true).await;
    let b = file_device(w, "b", url, false).await;
    a.put("s1", "web").await;
    assert_eq!(a.sync_file().await.pushed, 1);
    assert_eq!(b.sync_file().await.pulled, 1);
    assert_eq!(b.names().await, vec!["web"]);
    let mut rows = b.local.query("SELECT id FROM vault_header", ()).await.unwrap();
    assert_eq!(rows.next().await.unwrap().unwrap().get::<String>(0).unwrap(), VAULT, "the header comes along");
    drop(rows);
    assert_eq!(sync_local::dirty_count(&b.local).await.unwrap(), 0);

    // Both edit; the newer wins and the other is kept as a copy.
    a.put("s1", "web from a").await;
    b.put("s1", "web from b").await;
    a.later(5_000).await;
    a.sync_file().await;
    assert_eq!(b.sync_file().await.conflicts, 1);
    a.sync_file().await;
    assert_eq!(a.names().await, b.names().await);
    assert_eq!(a.names().await.len(), 2);

    // A delete reaches the other device.
    b.delete("s1").await;
    b.sync_file().await;
    a.sync_file().await;
    assert_eq!(a.names().await.len(), 1);
    assert!(a.names().await[0].starts_with("web from b (conflict copy "));
}

#[tokio::test]
async fn a_folder_carries_the_vault_between_devices() {
    let w = file_world();
    let url = format!("folder:{}", w.dir.join("share").display());
    the_rules_hold(&w, &url).await;

    // The file names nothing. (Look for a long name: three random bytes of
    // ciphertext can spell "web" now and then.)
    let data = std::fs::read(w.dir.join("share").join(crate::vault::sync_store::FILE_NAME)).unwrap();
    assert!(!data.windows(10).any(|x| x == b"web from b"));
}

#[tokio::test]
async fn a_git_repository_carries_the_vault_between_devices() {
    if !std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("git not installed; skipped");
        return;
    }
    let w = file_world();
    let remote = w.dir.join("remote.git");
    assert!(std::process::Command::new("git").args(["init", "--bare", "--quiet"]).arg(&remote).status().unwrap().success());
    the_rules_hold(&w, &format!("git:{}", remote.display())).await;
}

#[tokio::test]
async fn an_older_file_put_back_deletes_nothing() {
    let w = file_world();
    let share = w.dir.join("share");
    let url = format!("folder:{}", share.display());
    let a = file_device(&w, "a", &url, true).await;
    let b = file_device(&w, "b", &url, false).await;
    a.put("s1", "one").await;
    a.sync_file().await;
    let file = share.join(crate::vault::sync_store::FILE_NAME);
    let old = std::fs::read(&file).unwrap();
    a.put("s2", "two").await;
    a.sync_file().await;
    b.sync_file().await;
    assert_eq!(b.names().await, vec!["one", "two"]);

    // A backup of the share from before "two" is put back.
    std::fs::write(&file, &old).unwrap();
    b.sync_file().await;
    assert_eq!(b.names().await, vec!["one", "two"], "nothing is taken as deleted");
    a.sync_file().await;
    assert_eq!(a.names().await, vec!["one", "two"]);
    let c = file_device(&w, "c", &url, false).await;
    c.sync_file().await;
    assert_eq!(c.names().await, vec!["one", "two"], "and the file has it all again");
}

#[tokio::test]
async fn a_folder_holding_another_vault_is_refused() {
    let w = file_world();
    let url = format!("folder:{}", w.dir.join("share").display());
    let a = file_device(&w, "a", &url, true).await;
    a.sync_file().await;
    let mut other = file_device(&w, "x", &url, false).await;
    other.target.key = RecordKey::derive(&master(), "another-vault").unwrap();
    let err = sync(&other.target).await.unwrap_err().to_string();
    assert!(err.contains("another vault"), "{err}");
}

#[test]
fn backends_are_read_from_their_urls() {
    let app = std::path::Path::new("/app");
    let b = Backend::parse("sftp://me%40corp@files.example.com:2222/~/reach?hostkey=SHA256:abc", r#"{"password":"pw"}"#, app).unwrap();
    let Backend::Sftp(t) = &b else { panic!("not sftp") };
    assert_eq!((t.user.as_str(), t.host.as_str(), t.port, t.path.as_str()), ("me@corp", "files.example.com", 2222, "reach"));
    assert_eq!(t.host_key.as_deref(), Some("SHA256:abc"));
    assert_eq!(t.password.as_deref(), Some("pw"));
    let Backend::Sftp(t) = Backend::parse("sftp://me@h/srv/reach", "pw", app).unwrap() else { panic!() };
    assert_eq!((t.port, t.path.as_str(), t.password.as_deref()), (22, "/srv/reach", Some("pw")));
    assert!(!b.identity().contains("pw"));

    let Backend::Smb(t) = Backend::parse("smb://CORP;me@nas.local/Team%20Share/reach/vault", "pw", app).unwrap() else { panic!() };
    assert_eq!((t.user.as_str(), t.server.as_str(), t.port), ("CORP\\me", "nas.local", 445));
    assert_eq!((t.share.as_str(), t.path.as_str(), t.password.as_str()), ("Team Share", "reach\\vault", "pw"));
    assert!(Backend::parse("smb://me@nas", "pw", app).is_err(), "no share");

    let Backend::Git(g) = Backend::parse("git:git@github.com:me/vault.git", "", app).unwrap() else { panic!() };
    assert_eq!((g.remote.as_str(), g.branch.as_str()), ("git@github.com:me/vault.git", DEFAULT_GIT_BRANCH));
    let Backend::Git(g) = Backend::parse("git:https://example.com/r.git#mine", "tok", app).unwrap() else { panic!() };
    assert_eq!(g.branch, "mine");
    assert!(g.work_dir.starts_with("/app/sync-git"));
    assert!(Backend::parse("git:http://example.com/r.git", "tok", app).is_err(), "a token in the clear");
    assert!(Backend::parse("git:x#--upload-pack=evil", "", app).is_err());
    assert!(Backend::parse("git:--upload-pack=evil", "", app).is_err(), "an option, not an address");
    assert!(Backend::parse("git:ext::sh -c evil", "", app).is_err(), "a helper that runs a command");
    assert!(Backend::parse("git:ssh://git@[::1]/r.git", "", app).is_ok(), "an IPv6 host is not a helper");

    assert!(Backend::parse("folder:relative/path", "", app).is_err());
    assert!(matches!(Backend::parse("libsql://db.turso.io", "t", app), Ok(Backend::Turso { .. })));
}

fn hosts(v: &serde_json::Value, field: &str) -> Vec<String> {
    v[field].as_array().map(|a| a.iter().map(|k| k["host"].as_str().unwrap().to_string()).collect()).unwrap_or_default()
}

/// Two devices each have the session; one saves a host key while the other
/// writes the session from a copy without it (not synced yet, or a build
/// that does not know host keys). The key must survive, on both, in both
/// formats, whichever write is newer.
async fn host_keys_survive(legacy: bool, other_newer: bool) {
    let w = world(legacy).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    a.put_json("s1", serde_json::json!({ "id": "s1", "name": "srv" })).await;
    a.sync(&w).await;
    b.sync(&w).await;

    let key = serde_json::json!([{ "host": "srv.example.com:22", "fingerprint": "SHA256:k", "saved_at": 100 }]);
    a.put_json("s1", serde_json::json!({ "id": "s1", "name": "srv", "host_keys": key })).await;
    b.put_json("s1", serde_json::json!({ "id": "s1", "name": "srv-renamed" })).await;
    if other_newer {
        b.later(5_000).await;
    } else {
        a.later(5_000).await;
    }
    if legacy {
        // Legacy rows carry seconds: make the newer one count there too.
        let (d, at) = if other_newer { (&b, 9) } else { (&a, 9) };
        d.local.execute("UPDATE secrets SET updated_at = ?", [at]).await.unwrap();
    }
    a.sync(&w).await;
    b.sync(&w).await;
    a.sync(&w).await;
    b.sync(&w).await;
    for d in [&a, &b] {
        let p = d.payload("s1").await;
        assert_eq!(hosts(&p, "host_keys"), vec!["srv.example.com:22"], "legacy {legacy}, other newer {other_newer}");
    }
    assert_eq!(a.payload("s1").await["name"], b.payload("s1").await["name"], "both end up the same");
    assert_eq!(sync_local::dirty_count(&a.local).await.unwrap(), 0);
    assert_eq!(sync_local::dirty_count(&b.local).await.unwrap(), 0);
}

#[tokio::test]
async fn a_saved_host_key_survives_an_edit_from_a_copy_without_it() {
    host_keys_survive(false, true).await;
    host_keys_survive(false, false).await;
    host_keys_survive(true, true).await;
    host_keys_survive(true, false).await;
}

#[tokio::test]
async fn a_removed_host_key_stays_removed() {
    let w = world(false).await;
    let a = device(&w, "a").await;
    let b = device(&w, "b").await;
    let key = serde_json::json!([{ "host": "srv.example.com:22", "fingerprint": "SHA256:k", "saved_at": 100 }]);
    a.put_json("s1", serde_json::json!({ "id": "s1", "name": "srv", "host_keys": key })).await;
    a.sync(&w).await;
    b.sync(&w).await;
    // Removed on a; b, not synced since, renames the session from its copy.
    a.put_json("s1", serde_json::json!({ "id": "s1", "name": "srv", "host_keys_removed": [{ "host": "srv.example.com:22", "removed_at": 200 }] })).await;
    a.sync(&w).await;
    b.put_json("s1", serde_json::json!({ "id": "s1", "name": "srv-renamed", "host_keys": key })).await;
    b.later(5_000).await;
    b.sync(&w).await;
    a.sync(&w).await;
    b.sync(&w).await;
    for d in [&a, &b] {
        let p = d.payload("s1").await;
        assert!(hosts(&p, "host_keys").is_empty());
        assert_eq!(hosts(&p, "host_keys_removed"), vec!["srv.example.com:22"]);
    }
}
