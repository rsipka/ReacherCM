use super::*;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::ipc::sync_account_commands::VAULT_TEST_ACCOUNT;

#[derive(Default)]
struct Remote {
    id: Option<String>,
    bytes: Vec<u8>,
    revision: u64,
    writes_allowed: bool,
    downloads_before_failure: Option<usize>,
}

impl Remote {
    fn tag(&self) -> String {
        format!("\"{:064x}\"", self.revision)
    }
}

type Shared = Arc<Mutex<Remote>>;

#[cfg(desktop)]
async fn list(State(remote): State<Shared>) -> impl IntoResponse {
    Json(json!({"vault_ids": remote.lock().await.id.iter().collect::<Vec<_>>()}))
}

async fn download(State(remote): State<Shared>, Path(id): Path<String>) -> Response {
    let mut remote = remote.lock().await;
    if let Some(remaining) = remote.downloads_before_failure.as_mut() {
        if *remaining == 0 {
            remote.downloads_before_failure = None;
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        *remaining -= 1;
    }
    if remote.id.as_deref() != Some(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    ([(header::ETAG, remote.tag())], remote.bytes.clone()).into_response()
}

async fn upload(
    State(remote): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let mut remote = remote.lock().await;
    if !remote.writes_allowed {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let current = remote.tag();
    let matches = if remote.id.is_none() {
        headers
            .get(header::IF_NONE_MATCH)
            .is_some_and(|tag| tag == "*")
    } else {
        remote.id.as_deref() == Some(&id)
            && headers
                .get(header::IF_MATCH)
                .is_some_and(|tag| tag == current.as_str())
    };
    if !matches {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    remote.id = Some(id);
    remote.bytes = bytes.to_vec();
    remote.revision += 1;
    (StatusCode::NO_CONTENT, [(header::ETAG, remote.tag())]).into_response()
}

fn value(text: &str) -> SecretBox<Vec<u8>> {
    SecretBox::new(Box::new(text.as_bytes().to_vec()))
}

async fn flush(manager: &VaultManager, id: &str) -> sync_engine::Report {
    let target = manager.sync_targets(Some(id)).unwrap().pop().unwrap();
    sync_engine::sync(&target).await.unwrap()
}

#[cfg(desktop)]
#[tokio::test]
async fn an_account_vault_migrates_and_links_without_exposing_records() {
    use crate::ipc::sync_account_commands::{clear_vault_test_account, seed_vault_test_account};

    let _account_guard = VAULT_TEST_ACCOUNT.lock().await;
    let remote: Shared = Default::default();
    let router = Router::new()
        .route("/api/v1/vaults", get(list))
        .route("/api/v1/vaults/{id}/bundle", get(download).put(upload))
        .with_state(remote.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let account = seed_vault_test_account(&url, "vault-test").await;

    let dir = std::env::temp_dir().join(format!("reacher-account-vault-{}", uuid::Uuid::new_v4()));
    let mut first = VaultManager::new(dir.join("first"));
    first.init_identity("desktop-pass").await.unwrap();
    let original_vault = first.get_vault_id_by_name(SESSIONS_VAULT).unwrap();
    let original = "private-session-description-for-this-test";
    let record = first
        .create_secret(
            &original_vault,
            "Example",
            SecretCategory::Session,
            value(original),
        )
        .await
        .unwrap();

    assert!(
        first.enable_account_sync(&account).await.is_err(),
        "simulated failed upload"
    );
    assert_eq!(
        first.get_vault_id_by_name(SESSIONS_VAULT).as_deref(),
        Some(original_vault.as_str())
    );
    assert!(first.get_personal_sync_config().0.is_none());
    remote.lock().await.writes_allowed = true;

    let personal = first.enable_account_sync(&account).await.unwrap();
    assert_eq!(
        first.get_vault_id_by_name(SESSIONS_VAULT).as_deref(),
        Some(personal.as_str())
    );
    assert_ne!(personal, original_vault);
    assert!(dir
        .join("first/vaults")
        .join(format!("{original_vault}.db"))
        .exists());
    let code = first.export_device_link().await.unwrap();
    assert!(code.starts_with("reachercm-account-link1:"));
    assert!(!remote
        .lock()
        .await
        .bytes
        .windows(original.len())
        .any(|s| s == original.as_bytes()));

    let mut second = VaultManager::new(dir.join("second"));
    let prior_id = second.init_identity("second-password").await.unwrap();
    let local_vault = second.get_vault_id_by_name(SESSIONS_VAULT).unwrap();
    let local_record = second
        .create_secret(
            &local_vault,
            "Before linking",
            SecretCategory::Session,
            value("local device session"),
        )
        .await
        .unwrap();
    let identity_path = dir.join("second/vault_identity.json");
    let prior_file = std::fs::read(&identity_path).unwrap();
    seed_vault_test_account(&url, "other-account").await;
    assert!(second.link_device(&code).await.is_err());
    assert_eq!(second.user_uuid.as_deref(), Some(prior_id.as_str()));
    seed_vault_test_account(&url, "vault-test").await;
    // The first GET proves the link. Fail the second, which would load
    // the new vault, to check rollback after staging has started.
    remote.lock().await.downloads_before_failure = Some(1);
    assert!(second.link_device(&code).await.is_err());
    assert_eq!(second.user_uuid.as_deref(), Some(prior_id.as_str()));
    assert_eq!(std::fs::read(&identity_path).unwrap(), prior_file);
    assert_eq!(
        second
            .read_secret(&local_vault, &local_record)
            .await
            .unwrap()
            .expose_secret(),
        b"local device session"
    );

    second.link_device(&code).await.unwrap();
    assert!(second.local_vault_recovery_available());
    assert_eq!(
        second
            .read_secret(&personal, &record)
            .await
            .unwrap()
            .expose_secret(),
        original.as_bytes()
    );
    assert!(std::fs::read_dir(dir.join("second")).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("vault_identity.before-sync-link-")
    }));

    first
        .update_secret(&personal, &record, value("first desktop change"))
        .await
        .unwrap();
    second
        .update_secret(&personal, &record, value("second desktop change"))
        .await
        .unwrap();
    first
        .vaults
        .get(&personal)
        .unwrap()
        .conn()
        .unwrap()
        .execute(
            "UPDATE sync_dirty SET changed_at = changed_at + 5000 WHERE id = ?",
            [record.as_str()],
        )
        .await
        .unwrap();
    flush(&first, &personal).await;
    assert_eq!(flush(&second, &personal).await.conflicts, 1);
    flush(&first, &personal).await;
    assert_eq!(first.list_secrets(&personal).await.unwrap().len(), 2);

    drop(second);
    clear_vault_test_account().await;
    let mut reopened = VaultManager::new(dir.join("second"));
    assert!(reopened.unlock("desktop-pass").await.unwrap());
    assert_eq!(reopened.list_secrets(&personal).await.unwrap().len(), 2);
    assert!(reopened.local_vault_recovery_available());
    reopened.restore_local_vault_before_link().await.unwrap();
    assert_eq!(std::fs::read(&identity_path).unwrap(), prior_file);
    assert!(reopened.unlock("second-password").await.unwrap());
    assert!(!reopened.local_vault_recovery_available());
    assert_eq!(
        reopened
            .read_secret(&local_vault, &local_record)
            .await
            .unwrap()
            .expose_secret(),
        b"local device session"
    );
    assert!(dir
        .join("second/vaults")
        .join(format!("{personal}.db"))
        .exists());

    *remote.lock().await = Remote {
        writes_allowed: true,
        ..Default::default()
    };
    let folder = dir.join("previous-backend");
    std::fs::create_dir_all(&folder).unwrap();
    let mut migrated = VaultManager::new(dir.join("migrated"));
    migrated.init_identity("migrate-password").await.unwrap();
    let folder_url = format!("folder:{}", folder.display());
    migrated
        .set_personal_sync_config(Some(folder_url), Some(String::new()))
        .await
        .unwrap();
    let old_id = migrated.get_vault_id_by_name(SESSIONS_VAULT).unwrap();
    let old_record = migrated
        .create_secret(
            &old_id,
            "Old location",
            SecretCategory::Session,
            value(original),
        )
        .await
        .unwrap();
    flush(&migrated, &old_id).await;
    let migrate_account = seed_vault_test_account(&url, "migration-test").await;
    assert_eq!(
        migrated
            .enable_account_sync(&migrate_account)
            .await
            .unwrap(),
        old_id
    );
    assert_eq!(
        migrated
            .read_secret(&old_id, &old_record)
            .await
            .unwrap()
            .expose_secret(),
        original.as_bytes()
    );
    assert!(folder.join(crate::vault::sync_store::FILE_NAME).exists());

    clear_vault_test_account().await;
    service.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[derive(Default)]
struct OrganizationAccessFixture {
    signed_in: &'static str,
    grants: std::collections::BTreeMap<String, Value>,
    owner_key: Option<String>,
    epoch: u64,
}

#[tokio::test]
async fn an_organization_vault_rotates_without_losing_retained_offline_edits() {
    use crate::ipc::sync_account_commands::{clear_vault_test_account, seed_vault_test_account};
    use axum::routing::put;
    use uuid::Uuid;

    let _account_guard = VAULT_TEST_ACCOUNT.lock().await;
    let remote = Arc::new(Mutex::new(Remote {
        writes_allowed: true,
        ..Default::default()
    }));
    let org = Uuid::new_v4();
    let vaults = format!("/api/v1/organizations/{org}/vaults");
    let acl = Arc::new(Mutex::new(OrganizationAccessFixture {
        signed_in: "owner",
        ..Default::default()
    }));
    let organization = json!({"id": org, "name": "Team", "role": "owner"});
    let listed = organization.clone();
    let router = Router::new()
        .route("/api/v1/organizations", get(move || async move {
            Json(vec![listed])
        }))
        .route(&vaults, get({
            let acl = acl.clone();
            let remote = remote.clone();
            move || async move {
                let acl = acl.lock().await;
                let remote = remote.lock().await;
                let entries: Vec<_> = remote.id.iter().filter_map(|id| {
                    if acl.signed_in == "owner" {
                        return Some(json!({"id": id, "role": "owner", "wrapped_key": acl.owner_key, "key_epoch": acl.epoch}));
                    }
                    let grant = acl.grants.get(acl.signed_in)?;
                    Some(json!({"id": id, "role": grant["role"], "wrapped_key": grant["wrapped_key"], "key_epoch": acl.epoch}))
                }).collect();
                Json(entries)
            }
        }))
        .route(&format!("{vaults}/{{id}}/members/{{username}}"), put({
            let acl = acl.clone();
            move |Path((_id, username)): Path<(String, String)>, Json(value): Json<Value>| async move {
                acl.lock().await.grants.insert(username, value);
                StatusCode::NO_CONTENT
            }
        }))
        .route(&format!("{vaults}/{{id}}/rotation"), get({
            let acl = acl.clone();
            let remote = remote.clone();
            move || async move {
                let acl = acl.lock().await;
                let remote = remote.lock().await;
                let grants: Vec<_> = acl.grants.iter().map(|(username, grant)| json!({
                    "username": username, "recipient_proof": grant["recipient_proof"]
                })).collect();
                Json(json!({
                    "key_epoch": acl.epoch, "acl_version": 0, "revision": remote.tag(), "grants": grants
                }))
            }
        }).put({
            let acl = acl.clone();
            let remote = remote.clone();
            move |headers: HeaderMap, bytes: Bytes| async move {
                let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
                let metadata: Value = serde_json::from_slice(&bytes[4..4 + size]).unwrap();
                let mut acl = acl.lock().await;
                let mut remote = remote.lock().await;
                assert_eq!(headers[header::IF_MATCH], remote.tag());
                assert_eq!(metadata["key_epoch"], acl.epoch);
                acl.grants.remove(metadata["revoke"].as_str().unwrap()).unwrap();
                for (username, next) in metadata["grants"].as_object().unwrap() {
                    let grant = acl.grants.get_mut(username).unwrap();
                    grant["wrapped_key"] = next["wrapped_key"].clone();
                    grant["recipient_proof"] = next["recipient_proof"].clone();
                }
                acl.owner_key = metadata["owner_key"].as_str().map(str::to_owned);
                acl.epoch += 1;
                remote.bytes = bytes[4 + size..].to_vec();
                remote.revision += 1;
                StatusCode::NO_CONTENT
            }
        }))
        .route(&format!("{vaults}/{{id}}/bundle"), get(download).put(upload))
        .with_state(remote.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let mut owner = VaultManager::new(dir.path().join("owner"));
    let mut recipient = VaultManager::new(dir.path().join("recipient"));
    owner.init_identity("owner-password").await.unwrap();
    recipient.init_identity("recipient-password").await.unwrap();
    let account = seed_vault_test_account(&url, "owner").await;
    let organizations = VaultManager::organizations(&account).await.unwrap();
    assert_eq!(serde_json::to_value(organizations).unwrap(), json!([organization]));
    let info = owner
        .create_organization_vault(&account, org, "Team vault")
        .await
        .unwrap();
    let secret = "private-session-record-in-team-vault";
    let record = owner
        .create_secret(
            &info.id,
            "Team server",
            SecretCategory::Session,
            value(secret),
        )
        .await
        .unwrap();
    flush(&owner, &info.id).await;
    assert!(!remote
        .lock()
        .await
        .bytes
        .windows(secret.len())
        .any(|s| s == secret.as_bytes()));
    let public_key = BASE64.encode(recipient.identity.as_ref().unwrap().public_key.as_bytes());
    owner
        .change_organization_grant(&info.id, "recipient", Some("viewer"), &public_key)
        .await
        .unwrap();

    let removed_secret = x25519_dalek::StaticSecret::from(rand::random::<[u8; 32]>());
    let removed_key = BASE64.encode(PublicKey::from(&removed_secret).as_bytes());
    owner
        .change_organization_grant(&info.id, "removed", Some("viewer"), &removed_key)
        .await
        .unwrap();
    let previous_master = owner.vaults[&info.id]
        .master_dek
        .as_ref()
        .unwrap()
        .with_key(|key| Dek::from_fn(|copy| copy.copy_from_slice(key)));
    let previous_key = sync_record::RecordKey::derive(&previous_master, &info.id).unwrap();

    acl.lock().await.signed_in = "recipient";
    let account = seed_vault_test_account(&url, "recipient").await;
    let id = Uuid::parse_str(&info.id).unwrap();
    let owner_key = BASE64.encode(owner.identity.as_ref().unwrap().public_key.as_bytes());
    assert!(
        recipient
            .open_organization_vault(&account, org, id, None)
            .await
            .is_err()
    );
    assert!(
        recipient
            .open_organization_vault(&account, org, id, Some(&public_key))
            .await
            .is_err()
    );
    recipient
        .open_organization_vault(&account, org, id, Some(&owner_key))
        .await
        .unwrap();
    assert_eq!(
        recipient
            .read_secret(&info.id, &record)
            .await
            .unwrap()
            .expose_secret(),
        secret.as_bytes()
    );
    assert!(matches!(
        recipient
            .update_secret(&info.id, &record, value("viewer edit"))
            .await,
        Err(VaultError::AccessDenied(_))
    ));

    acl.lock().await.grants.get_mut("recipient").unwrap()["role"] = json!("editor");
    recipient
        .open_organization_vault(&account, org, id, None)
        .await
        .unwrap();
    recipient
        .update_secret(&info.id, &record, value("editor edit"))
        .await
        .unwrap();
    // A higher reported epoch without an owner-authorized key must leave
    // the pending edit intact and stop sync before the upload.
    let revision = remote.lock().await.revision;
    acl.lock().await.epoch = 1;
    let recipient_manager = Mutex::new(recipient);
    let (_, failures) = sync_vaults(&recipient_manager, Some(&info.id), false).await;
    assert_eq!(failures.len(), 1);
    assert_eq!(remote.lock().await.revision, revision);
    let recipient = recipient_manager.into_inner();
    assert_eq!(
        recipient.vaults[&info.id]
            .organization_key_epoch()
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        recipient
            .read_secret(&info.id, &record)
            .await
            .unwrap()
            .expose_secret(),
        b"editor edit"
    );
    {
        let mut acl = acl.lock().await;
        acl.epoch = 0;
        acl.signed_in = "owner";
    }
    seed_vault_test_account(&url, "owner").await;
    owner
        .change_organization_grant(&info.id, "removed", None, "")
        .await
        .unwrap();
    assert!(crate::vault::sync_bundle::verify(&previous_key, &remote.lock().await.bytes).is_err());

    acl.lock().await.signed_in = "recipient";
    seed_vault_test_account(&url, "recipient").await;
    let recipient = Mutex::new(recipient);
    let (_, failures) = sync_vaults(&recipient, Some(&info.id), false).await;
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(
        recipient.lock().await.vaults[&info.id]
            .organization_key_epoch()
            .await
            .unwrap(),
        1
    );
    drop(recipient);

    acl.lock().await.signed_in = "owner";
    seed_vault_test_account(&url, "owner").await;
    assert_eq!(flush(&owner, &info.id).await.pulled, 1);
    assert_eq!(
        owner
            .read_secret(&info.id, &record)
            .await
            .unwrap()
            .expose_secret(),
        b"editor edit"
    );
    assert!(crate::vault::sync_bundle::verify(&previous_key, &remote.lock().await.bytes).is_err());

    clear_vault_test_account().await;
    service.abort();
    let mut restarted = VaultManager::new(dir.path().join("recipient"));
    assert!(restarted.unlock("recipient-password").await.unwrap());
    assert_eq!(
        restarted
            .read_secret(&info.id, &record)
            .await
            .unwrap()
            .expose_secret(),
        b"editor edit"
    );
}

#[derive(Deserialize)]
struct NativeSyncFixture {
    server: String,
    organization: uuid::Uuid,
    devices: [NativeSyncDevice; 3],
}

#[derive(Deserialize)]
struct NativeSyncDevice {
    username: String,
    access_token: String,
    refresh_token: String,
    device_id: String,
}

impl NativeSyncDevice {
    async fn activate(&self, server: &str) -> crate::ipc::sync_account_commands::AccountView {
        crate::ipc::sync_account_commands::seed_vault_test_tokens(
            server,
            &self.username,
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.device_id.clone(),
        )
        .await
    }
}

async fn native_sync(
    manager: &Mutex<VaultManager>,
    device: &NativeSyncDevice,
    server: &str,
    vault: &str,
) {
    device.activate(server).await;
    let (_, failed) = sync_vaults(manager, Some(vault), false).await;
    assert!(failed.is_empty(), "{}: {failed:?}", device.username);
}

async fn native_snapshot(manager: &Mutex<VaultManager>, vault: &str) -> Vec<(String, Vec<u8>)> {
    let manager = manager.lock().await;
    let mut records = Vec::new();
    for item in manager.list_secrets(vault).await.unwrap() {
        let value = manager.read_secret(vault, &item.id).await.unwrap();
        records.push((item.id, value.expose_secret().to_vec()));
    }
    records.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    records
}

#[tokio::test]
#[ignore = "requires the Go loopback portal and disposable PostgreSQL fixture on stdin"]
async fn three_native_clients_sync_through_postgres() {
    use crate::ipc::sync_account_commands::clear_vault_test_account;
    use crate::vault::sync_account_store::AccountTarget;
    use uuid::Uuid;

    let _account_guard = VAULT_TEST_ACCOUNT.lock().await;
    let fixture: NativeSyncFixture = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let [owner_device, editor_device, third_device] = &fixture.devices;
    let server = &fixture.server;
    let org = fixture.organization;
    let dir = tempfile::tempdir().unwrap();
    let owner = Mutex::new(VaultManager::new(dir.path().join("owner")));
    let editor = Mutex::new(VaultManager::new(dir.path().join("editor")));
    let third = Mutex::new(VaultManager::new(dir.path().join("third")));
    for (manager, password) in [
        (&owner, "synthetic owner password"),
        (&editor, "synthetic editor password"),
        (&third, "synthetic third password"),
    ] {
        manager.lock().await.init_identity(password).await.unwrap();
    }

    let account = owner_device.activate(server).await;
    let (vault, common, to_delete) = {
        let mut manager = owner.lock().await;
        let vault = manager
            .create_organization_vault(&account, org, "Synthetic shared vault")
            .await
            .unwrap()
            .id;
        let common = manager
            .create_secret(
                &vault,
                "Common session",
                SecretCategory::Session,
                value("initial"),
            )
            .await
            .unwrap();
        let to_delete = manager
            .create_secret(
                &vault,
                "Unused session",
                SecretCategory::Session,
                value("delete me"),
            )
            .await
            .unwrap();
        (vault, common, to_delete)
    };
    let id = Uuid::parse_str(&vault).unwrap();
    native_sync(&owner, owner_device, server, &vault).await;
    let account = third_device.activate(server).await;
    assert!(third
        .lock()
        .await
        .organization_vaults(&account, org)
        .await
        .unwrap()
        .is_empty());

    owner_device.activate(server).await;
    let editor_key = BASE64.encode(
        editor
            .lock()
            .await
            .identity
            .as_ref()
            .unwrap()
            .public_key
            .as_bytes(),
    );
    let third_key = BASE64.encode(
        third
            .lock()
            .await
            .identity
            .as_ref()
            .unwrap()
            .public_key
            .as_bytes(),
    );
    owner
        .lock()
        .await
        .change_organization_grant(&vault, &editor_device.username, Some("editor"), &editor_key)
        .await
        .unwrap();
    owner
        .lock()
        .await
        .change_organization_grant(&vault, &third_device.username, Some("viewer"), &third_key)
        .await
        .unwrap();

    let owner_key = BASE64.encode(
        owner
            .lock()
            .await
            .identity
            .as_ref()
            .unwrap()
            .public_key
            .as_bytes(),
    );
    for (manager, device) in [(&editor, editor_device), (&third, third_device)] {
        let account = device.activate(server).await;
        manager
            .lock()
            .await
            .open_organization_vault(&account, org, id, Some(&owner_key))
            .await
            .unwrap();
    }
    let baseline = native_snapshot(&owner, &vault).await;
    assert_eq!(baseline, native_snapshot(&editor, &vault).await);
    assert_eq!(baseline, native_snapshot(&third, &vault).await);
    assert!(matches!(
        third
            .lock()
            .await
            .update_secret(&vault, &common, value("viewer denied"))
            .await,
        Err(VaultError::AccessDenied(_))
    ));

    owner_device.activate(server).await;
    owner
        .lock()
        .await
        .change_organization_grant(&vault, &third_device.username, Some("editor"), &third_key)
        .await
        .unwrap();
    native_sync(&third, third_device, server, &vault).await;
    for (manager, text) in [
        (&owner, "owner collision edit"),
        (&editor, "editor collision edit"),
        (&third, "third collision edit"),
    ] {
        manager
            .lock()
            .await
            .update_secret(&vault, &common, value(text))
            .await
            .unwrap();
    }
    {
        // Make the conflict winner deterministic without sleeping. Both
        // losing session values should survive in conflict copies.
        let manager = owner.lock().await;
        manager.vaults[&vault]
            .conn()
            .unwrap()
            .execute(
                "UPDATE sync_dirty SET changed_at = changed_at + 5000 WHERE id = ?",
                [common.as_str()],
            )
            .await
            .unwrap();
    }
    editor
        .lock()
        .await
        .create_secret(
            &vault,
            "Editor addition",
            SecretCategory::Session,
            value("distinct editor addition"),
        )
        .await
        .unwrap();
    third
        .lock()
        .await
        .delete_secret(&vault, &to_delete)
        .await
        .unwrap();

    for (manager, device) in [
        (&owner, owner_device),
        (&editor, editor_device),
        (&third, third_device),
        (&editor, editor_device),
        (&owner, owner_device),
    ] {
        native_sync(manager, device, server, &vault).await;
    }
    let converged = native_snapshot(&owner, &vault).await;
    assert_eq!(converged, native_snapshot(&editor, &vault).await);
    assert_eq!(converged, native_snapshot(&third, &vault).await);
    assert_eq!(converged.len(), 4);
    for expected in [
        "owner collision edit",
        "editor collision edit",
        "third collision edit",
        "distinct editor addition",
    ] {
        assert!(
            converged
                .iter()
                .any(|(_, bytes)| bytes == expected.as_bytes()),
            "missing {expected}"
        );
    }
    assert!(!converged.iter().any(|(id, _)| id == &to_delete));

    // Keep an editor offline across ownership transfer and key rotation.
    let transfer_pending = third
        .lock()
        .await
        .create_secret(
            &vault,
            "Edit queued before ownership transfer",
            SecretCategory::Session,
            value("publish after verifying the new owner"),
        )
        .await
        .unwrap();
    owner_device.activate(server).await;
    owner
        .lock()
        .await
        .change_organization_grant(&vault, &editor_device.username, Some("owner"), &editor_key)
        .await
        .unwrap();
    editor_device.activate(server).await;
    editor
        .lock()
        .await
        .finish_organization_rotation(&vault)
        .await
        .unwrap();

    let third_account = third_device.activate(server).await;
    let target = AccountTarget::shared(&third_account, org, id).unwrap();
    let (_, epoch_revision) = target.get().await.unwrap().unwrap();
    let (_, failed) = sync_vaults(&third, Some(&vault), false).await;
    assert!(matches!(
        failed.as_slice(),
        [(_, VaultError::OrganizationOwnerVerificationRequired)]
    ));
    assert_eq!(target.get().await.unwrap().unwrap().1, epoch_revision);
    {
        let mut manager = third.lock().await;
        assert_eq!(
            manager.vaults[&vault].organization_key_epoch().await.unwrap(),
            0
        );
        assert_eq!(
            manager
                .read_secret(&vault, &transfer_pending)
                .await
                .unwrap()
                .expose_secret(),
            b"publish after verifying the new owner"
        );
        assert!(matches!(
            manager
                .open_organization_vault(&third_account, org, id, Some(&owner_key))
                .await,
            Err(VaultError::OrganizationOwnerVerificationRequired)
        ));
        manager
            .open_organization_vault(&third_account, org, id, Some(&editor_key))
            .await
            .unwrap();
    }
    native_sync(&editor, editor_device, server, &vault).await;
    assert!(
        native_snapshot(&editor, &vault)
            .await
            .iter()
            .any(|(id, _)| id == &transfer_pending)
    );

    // The former owner has to receive a fresh grant and verify the new
    // owner too. An epoch-zero owner has no cached wrapped-key envelope.
    editor
        .lock()
        .await
        .change_organization_grant(&vault, &owner_device.username, Some("editor"), &owner_key)
        .await
        .unwrap();
    let owner_account = owner_device.activate(server).await;
    owner
        .lock()
        .await
        .open_organization_vault(&owner_account, org, id, Some(&editor_key))
        .await
        .unwrap();

    let old_key = {
        let manager = third.lock().await;
        sync_record::RecordKey::derive(manager.vaults[&vault].master_dek.as_ref().unwrap(), &vault)
            .unwrap()
    };
    let retained_pending = owner
        .lock()
        .await
        .create_secret(
            &vault,
            "Retained offline edit",
            SecretCategory::Session,
            value("publish after rotation"),
        )
        .await
        .unwrap();
    let revoked_pending = third
        .lock()
        .await
        .create_secret(
            &vault,
            "Revoked offline edit",
            SecretCategory::Session,
            value("must not upload"),
        )
        .await
        .unwrap();
    let editor_account = editor_device.activate(server).await;
    editor
        .lock()
        .await
        .change_organization_grant(&vault, &third_device.username, None, "")
        .await
        .unwrap();
    let target = AccountTarget::shared(&editor_account, org, id).unwrap();
    let (bundle, _) = target.get().await.unwrap().unwrap();
    assert!(crate::vault::sync_bundle::verify(&old_key, &bundle).is_err());
    let third_account = third_device.activate(server).await;
    assert!(AccountTarget::shared(&third_account, org, id)
        .unwrap()
        .get()
        .await
        .unwrap()
        .is_none());
    let (_, failed) = sync_vaults(&third, Some(&vault), false).await;
    assert_eq!(failed.len(), 1);

    native_sync(&editor, editor_device, server, &vault).await;
    native_sync(&owner, owner_device, server, &vault).await;
    native_sync(&editor, editor_device, server, &vault).await;
    let final_records = native_snapshot(&owner, &vault).await;
    assert_eq!(final_records, native_snapshot(&editor, &vault).await);
    assert_eq!(final_records.len(), 6);
    assert!(final_records.iter().any(|(id, _)| id == &transfer_pending));
    assert!(final_records.iter().any(|(id, _)| id == &retained_pending));
    assert!(!final_records.iter().any(|(id, _)| id == &revoked_pending));
    clear_vault_test_account().await;
    println!("Three clients converged; both collision copies and the deletion synced. Owner verification and revocation preserved the retained offline edits.");
}
