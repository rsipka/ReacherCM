use super::*;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

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
