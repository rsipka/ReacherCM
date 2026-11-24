use reqwest::{header, StatusCode};
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use crate::ipc::sync_account_commands;
use crate::vault::VaultError;

pub(crate) const PREFIX: &str = "reachercm-sync:";
const MAX_BYTES: usize = 64 << 20;

#[derive(Clone)]
pub struct AccountTarget {
    server: Url,
    username: String,
    pub(crate) vault_id: Uuid,
}

fn error(message: &str) -> VaultError {
    VaultError::SyncError(message.to_owned())
}

fn connection_error(key: &str) -> VaultError {
    error(match key {
        "sync_account.error_revoked" => "Sign in to the Reacher CM Sync account for this vault",
        "sync_account.busy" => "The sync account is busy. Try again shortly",
        _ => "Could not reach the Reacher CM Sync server",
    })
}

#[derive(Deserialize)]
struct VaultList {
    vault_ids: Vec<Uuid>,
}

impl AccountTarget {
    pub(crate) fn from_account(
        account: &sync_account_commands::AccountView,
        vault_id: Uuid,
    ) -> Result<Self, VaultError> {
        let server = sync_account_commands::origin(&account.server).map_err(connection_error)?;
        Ok(Self {
            server,
            username: account.username.clone(),
            vault_id,
        })
    }

    pub(crate) fn parse(config: &str, username: &str) -> Result<Self, VaultError> {
        let encoded = config
            .strip_prefix(PREFIX)
            .ok_or_else(|| error("Invalid vault sync server"))?;
        let (origin, id) = encoded
            .rsplit_once('/')
            .ok_or_else(|| error("Missing sync vault ID"))?;
        let server = sync_account_commands::origin(origin).map_err(connection_error)?;
        let vault_id = Uuid::parse_str(id).map_err(|_| error("Invalid sync vault ID"))?;
        if username.is_empty() {
            return Err(error("Missing vault sync account"));
        }
        Ok(Self {
            server,
            username: username.to_owned(),
            vault_id,
        })
    }

    pub(crate) fn config(&self) -> String {
        format!("{PREFIX}{}{}", self.server, self.vault_id)
    }

    pub(crate) fn identity(&self) -> String {
        format!("{}#{}", self.config(), self.username)
    }

    pub(crate) async fn listed(&self) -> Result<Vec<Uuid>, VaultError> {
        let url = self
            .server
            .join("/api/v1/vaults")
            .map_err(|_| error("Invalid vault sync server"))?;
        let result = sync_account_commands::vault_request(&self.server, &self.username, |http| {
            http.get(url.clone())
        })
        .await
        .map_err(connection_error)?;
        if result.status() != StatusCode::OK {
            return Err(error("The sync server could not list personal vaults"));
        }
        result
            .json::<VaultList>()
            .await
            .map(|list| list.vault_ids)
            .map_err(|_| error("The sync server returned an invalid vault list"))
    }

    fn bundle_url(&self) -> Url {
        let mut url = self.server.clone();
        url.set_path(&format!("/api/v1/vaults/{}/bundle", self.vault_id));
        url
    }

    pub(crate) async fn get(&self) -> Result<Option<(Vec<u8>, String)>, VaultError> {
        let url = self.bundle_url();
        let mut response =
            sync_account_commands::vault_request(&self.server, &self.username, |http| {
                http.get(url.clone())
            })
            .await
            .map_err(connection_error)?;
        match response.status() {
            StatusCode::NOT_FOUND => return Ok(None),
            StatusCode::OK => {}
            _ => return Err(error("The sync server could not download the vault bundle")),
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_revision(value))
            .ok_or_else(|| error("The sync server returned an invalid vault revision"))?
            .to_owned();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(error("The vault bundle exceeds the maximum size"));
        }
        let mut data = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error("The vault download was interrupted"))?
        {
            if chunk.len() > MAX_BYTES - data.len() {
                return Err(error("The vault bundle exceeds the maximum size"));
            }
            data.extend_from_slice(&chunk);
        }
        Ok(Some((data, etag)))
    }

    pub(crate) async fn put(
        &self,
        data: &[u8],
        expected: Option<&str>,
    ) -> Result<bool, VaultError> {
        if data.len() > MAX_BYTES || data.is_empty() {
            return Err(error("The vault bundle size is invalid"));
        }
        if expected.is_some_and(|etag| !valid_revision(etag)) {
            return Err(error("Invalid vault revision"));
        }
        let url = self.bundle_url();
        let response = sync_account_commands::vault_request(&self.server, &self.username, |http| {
            let request = http
                .put(url.clone())
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(data.to_vec());
            match expected {
                Some(etag) => request.header(header::IF_MATCH, etag),
                None => request.header(header::IF_NONE_MATCH, "*"),
            }
        })
        .await
        .map_err(connection_error)?;
        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::PRECONDITION_FAILED => Ok(false),
            StatusCode::CONFLICT => Err(error(
                "This sync account already has a different personal vault",
            )),
            StatusCode::PAYLOAD_TOO_LARGE => Err(error(
                "The vault bundle exceeds the sync server's size limit",
            )),
            _ => Err(error("The sync server could not write the vault bundle")),
        }
    }
}

fn valid_revision(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    bytes.len() == 66
        && bytes[0] == b'"'
        && bytes[65] == b'"'
        && bytes[1..65]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}
