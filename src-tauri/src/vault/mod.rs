#[cfg(test)]
mod compat_tests;
pub mod biometric;
pub mod apple_biometric;
pub mod crypto;
#[cfg(any(target_os = "linux", test))]
pub mod deadline;
pub mod error;
pub mod export;
pub mod fido2;
pub mod kdf;
#[cfg(target_os = "linux")]
pub mod linux_keychain;
pub mod manager;
pub mod private_fs;
pub mod schema;
pub mod shield;
pub mod sharing;
pub mod sync;
pub mod sync_bundle;
pub mod sync_engine;
pub mod sync_local;
pub mod sync_record;
pub mod sync_s3;
pub mod sync_store;
pub mod sync_tokens;
pub mod turso_api;
pub mod turso_tls;
pub mod types;

pub use error::VaultError;
pub use manager::{
    VaultManager, CREDENTIALS_VAULT, FOLDERS_VAULT, PLAYBOOKS_VAULT, SESSIONS_VAULT, SETTINGS_VAULT,
};
pub use types::{
    AppSettings, InviteInfo, MemberInfo, MemberRole, ReceivedShare, SecretCategory, SecretMetadata,
    ShareItemResult, SharedItemInfo, SyncStatus, TursoDbInfo, VaultInfo, VaultType,
};

pub(crate) mod sync_account_store;
