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

static TEST_ACCOUNT: Mutex<()> = Mutex::const_new(());

#[derive(Default)]
struct Remote {
    id: Option<String>,
    bytes: Vec<u8>,
    revision: u64,
    writes_allowed: bool,
}

impl Remote {
    fn tag(&self) -> String {
        format!("\"{:064x}\"", self.revision)
    }
}

type Shared = Arc<Mutex<Remote>>;

async fn list(State(remote): State<Shared>) -> impl IntoResponse {
    Json(json!({"vault_ids": remote.lock().await.id.iter().collect::<Vec<_>>()}))
}

async fn download(State(remote): State<Shared>, Path(id): Path<String>) -> Response {
    let remote = remote.lock().await;
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

#[tokio::test]
async fn an_account_vault_migrates_and_links_without_exposing_records() {
    use crate::ipc::sync_account_commands::{clear_vault_test_account, seed_vault_test_account};

    let _account_guard = TEST_ACCOUNT.lock().await;
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
    seed_vault_test_account(&url, "other-account").await;
    assert!(second.link_device(&code).await.is_err());
    assert_eq!(second.user_uuid.as_deref(), Some(prior_id.as_str()));
    seed_vault_test_account(&url, "vault-test").await;
    second.link_device(&code).await.unwrap();
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

    let _account_guard = TEST_ACCOUNT.lock().await;
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
    let router = Router::new()
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
    recipient
        .open_organization_vault(&account, org, id)
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
        .open_organization_vault(&account, org, id)
        .await
        .unwrap();
    recipient
        .update_secret(&info.id, &record, value("editor edit"))
        .await
        .unwrap();
    acl.lock().await.signed_in = "owner";
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
