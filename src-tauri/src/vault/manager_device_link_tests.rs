use super::*;

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reach-device-link-{}-{}", name, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A second device that set up an identity of its own (and so a vault of
/// its own) joins the first device's vault from a link code: it opens the
/// same personal vault, reads what the first device wrote, and its own
/// master password is now the first device's.
#[tokio::test]
async fn a_linked_device_opens_the_vault_the_first_one_syncs() {
    let dir = tmp_dir("join");
    let share = dir.join("share");
    std::fs::create_dir_all(&share).unwrap();
    let url = format!("folder:{}", share.display());
    let put = |text: &str| SecretBox::new(Box::new(text.as_bytes().to_vec()));

    let mut a = VaultManager::new(dir.join("a"));
    a.init_identity("desktop-pass").await.unwrap();
    assert!(a.export_device_link().await.is_err(), "nothing to link before sync is on");
    a.set_personal_sync_config(Some(url.clone()), Some(String::new())).await.unwrap();
    let vault = a.vault_names.get(SETTINGS_VAULT).cloned().unwrap();
    let one = a.create_secret(&vault, "Router", SecretCategory::Session, put("one")).await.unwrap();
    let code = a.export_device_link().await.unwrap();
    assert!(device_link::is_link_code(&code));
    let a = tokio::sync::Mutex::new(a);
    let (_, failed) = sync_vaults(&a, None, false).await;
    assert!(failed.is_empty());

    // The phone made its own identity first, as it did before linking existed.
    let mut b = VaultManager::new(dir.join("b"));
    b.init_identity("phone-pass").await.unwrap();
    let uuid = b.link_device(&format!("  {}\n", code)).await.unwrap();
    assert_eq!(Some(uuid), a.lock().await.user_uuid.clone());
    assert_eq!(b.vault_names.get(SETTINGS_VAULT), Some(&vault), "the same vault, not a new one");
    assert_ne!(b.device_id, a.lock().await.device_id, "another device of the same user");
    let b = tokio::sync::Mutex::new(b);
    let (_, failed) = sync_vaults(&b, None, false).await;
    assert!(failed.is_empty(), "{:?}", failed.iter().map(|(_, e)| e.to_string()).collect::<Vec<_>>());
    assert_eq!(b.lock().await.read_secret(&vault, &one).await.unwrap().expose_secret(), b"one");

    // A change on the phone reaches the desktop.
    let two = b.lock().await.create_secret(&vault, "Switch", SecretCategory::Session, put("two")).await.unwrap();
    sync_vaults(&b, None, false).await;
    sync_vaults(&a, None, false).await;
    assert_eq!(a.lock().await.read_secret(&vault, &two).await.unwrap().expose_secret(), b"two");

    // The token is sealed on disk, and the desktop's password opens the phone.
    let file = std::fs::read_to_string(dir.join("b/vault_identity.json")).unwrap();
    let stored: StoredIdentity = serde_json::from_str(&file).unwrap();
    assert!(stored.personal_sync_token.is_none());
    let mut b = VaultManager::new(dir.join("b"));
    assert!(!b.unlock("phone-pass").await.unwrap_or(false));
    assert!(b.unlock("desktop-pass").await.unwrap());
    assert_eq!(b.vault_names.get(SETTINGS_VAULT), Some(&vault));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_damaged_code_changes_nothing() {
    let dir = tmp_dir("damaged");
    let mut b = VaultManager::new(dir.clone());
    let uuid = b.init_identity("pass").await.unwrap();
    assert!(b.link_device("reachercm-link1:not-base64!").await.is_err());
    assert!(b.link_device("reachercm-link1:e30").await.is_err());
    assert_eq!(b.user_uuid, Some(uuid));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A code with a Turso URL and token still fits a QR code a phone reads.
#[tokio::test]
async fn a_link_code_fits_in_a_qr_code() {
    let dir = tmp_dir("qr");
    let mut a = VaultManager::new(dir.clone());
    a.init_identity("pass").await.unwrap();
    // A Turso database token is a JWT of about this size.
    a.personal_sync_url = Some("libsql://reach-personal-1a2b3c4d-someone.aws-eu-west-1.turso.io".into());
    a.personal_sync_token = Some("e".repeat(420));
    a.save_identity_current().await.unwrap();
    let code = a.export_device_link().await.unwrap();
    let qr = qrcode::QrCode::with_error_correction_level(code.as_bytes(), qrcode::EcLevel::L).unwrap();
    assert!(matches!(qr.version(), qrcode::Version::Normal(v) if v <= 30), "{:?} for {} bytes", qr.version(), code.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_phone_can_join_and_sync_the_encrypted_account_vault() {
    use crate::ipc::sync_account_commands::{
        clear_vault_test_account, seed_vault_test_account, VAULT_TEST_ACCOUNT,
    };
    use crate::vault::sync_account_store::AccountTarget;
    use axum::{
        body::Bytes,
        http::{header, HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{get, post},
        Router,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let _account_guard = VAULT_TEST_ACCOUNT.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("seed-vault");
    std::fs::create_dir(&folder).unwrap();

    // Seed an existing encrypted personal bundle without the desktop-only
    // wizard that moves a device's vault to an account server.
    let mut source = VaultManager::new(dir.path().join("source"));
    source.init_identity("source password").await.unwrap();
    source
        .set_personal_sync_config(
            Some(format!("folder:{}", folder.display())),
            Some(String::new()),
        )
        .await
        .unwrap();
    let vault = source.personal_vault_id().unwrap();
    let record = source
        .create_secret(
            &vault,
            "Already synced",
            SecretCategory::Session,
            SecretBox::new(Box::new(b"from desktop".to_vec())),
        )
        .await
        .unwrap();
    let seed = source.sync_targets(Some(&vault)).unwrap().pop().unwrap();
    sync_engine::sync(&seed).await.unwrap();

    let path = format!("/api/v1/vaults/{vault}");
    let bytes = Arc::new(Mutex::new(
        std::fs::read(folder.join(crate::vault::sync_store::FILE_NAME)).unwrap(),
    ));
    let receipt = Arc::new(Mutex::new(None::<String>));
    let etag = |data: &[u8]| format!("\"{}\"", crate::vault::sync_store::revision_of(data));
    let router = Router::new()
        .route(
            &format!("{path}/bundle"),
            get({
                let bytes = bytes.clone();
                move || async move {
                    let bytes = bytes.lock().await;
                    ([(header::ETAG, etag(&bytes))], bytes.clone())
                }
            })
            .put({
                let bytes = bytes.clone();
                move |headers: HeaderMap, next: Bytes| async move {
                    let mut bytes = bytes.lock().await;
                    if headers
                        .get(header::IF_MATCH)
                        .and_then(|tag| tag.to_str().ok())
                        != Some(etag(&bytes).as_str())
                    {
                        return StatusCode::PRECONDITION_FAILED;
                    }
                    *bytes = next.to_vec();
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .route(
            &format!("{path}/applied"),
            post({
                let bytes = bytes.clone();
                let receipt = receipt.clone();
                move |headers: HeaderMap| async move {
                    let revision = etag(&bytes.lock().await);
                    if headers
                        .get(header::IF_MATCH)
                        .and_then(|tag| tag.to_str().ok())
                        != Some(revision.as_str())
                    {
                        return StatusCode::PRECONDITION_FAILED.into_response();
                    }
                    *receipt.lock().await = Some(revision);
                    StatusCode::NO_CONTENT.into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let account = seed_vault_test_account(&server, "phone-vault-test").await;
    let target =
        AccountTarget::from_account(&account, uuid::Uuid::parse_str(&vault).unwrap()).unwrap();
    source.personal_sync_url = Some(target.config());
    source.personal_sync_token = Some(account.username);
    source.save_identity_current().await.unwrap();
    let code = source.export_device_link().await.unwrap();
    assert!(device_link::is_account_link_code(&code));
    assert!(
        qrcode::QrCode::with_error_correction_level(code.as_bytes(), qrcode::EcLevel::L).is_ok()
    );

    let mut phone = VaultManager::new(dir.path().join("phone"));
    phone.init_identity("phone password").await.unwrap();
    phone.link_device(&code).await.unwrap();
    assert_eq!(
        phone
            .read_secret(&vault, &record)
            .await
            .unwrap()
            .expose_secret(),
        b"from desktop"
    );
    assert_eq!(
        receipt.lock().await.as_deref(),
        Some(etag(&bytes.lock().await).as_str())
    );
    phone
        .update_secret(
            &vault,
            &record,
            SecretBox::new(Box::new(b"from phone".to_vec())),
        )
        .await
        .unwrap();
    let phone = Mutex::new(phone);
    let (_, failed) = sync_vaults(&phone, Some(&vault), false).await;
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(
        receipt.lock().await.as_deref(),
        Some(etag(&bytes.lock().await).as_str())
    );
    let mut source_target = seed;
    source_target.backend = sync_engine::Backend::Account(target);
    // The seed came from this same local database. Point its bookkeeping
    // at the fixture before reading back the phone's changes.
    let local = sync_engine::open_local(&source_target.local_path)
        .await
        .unwrap();
    sync_local::set_state(
        &local,
        sync_engine::STATE_BACKEND,
        &source_target.backend.identity(),
    )
    .await
    .unwrap();
    sync_engine::sync(&source_target).await.unwrap();
    assert_eq!(
        source
            .read_secret(&vault, &record)
            .await
            .unwrap()
            .expose_secret(),
        b"from phone"
    );

    clear_vault_test_account().await;
    service.abort();
}
