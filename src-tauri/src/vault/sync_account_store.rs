use reqwest::{header, StatusCode};
#[cfg(desktop)]
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use crate::ipc::sync_account_commands;
use crate::vault::VaultError;

pub(crate) const PREFIX: &str = "reachercm-sync:";
pub(crate) const ORG_PREFIX: &str = "reachercm-org:";
const MAX_BYTES: usize = 64 << 20;

#[derive(Clone)]
pub struct AccountTarget {
    server: Url,
    username: String,
    pub(crate) vault_id: Uuid,
    pub(crate) organization: Option<Uuid>,
    pub(crate) key_epoch: u64,
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

#[cfg(desktop)]
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
            organization: None,
            key_epoch: 0,
        })
    }

    pub(crate) fn parse(config: &str, username: &str) -> Result<Self, VaultError> {
        let shared = config.starts_with(ORG_PREFIX);
        let encoded = config
            .strip_prefix(if shared { ORG_PREFIX } else { PREFIX })
            .ok_or_else(|| error("Invalid vault sync server"))?;
        let (base, id) = encoded
            .rsplit_once('/')
            .ok_or_else(|| error("Missing sync vault ID"))?;
        let (origin, organization) = if shared {
            let (origin, org) = base
                .rsplit_once('/')
                .ok_or_else(|| error("Missing organization ID"))?;
            (
                origin,
                Some(Uuid::parse_str(org).map_err(|_| error("Invalid organization ID"))?),
            )
        } else {
            (base, None)
        };
        let server = sync_account_commands::origin(origin).map_err(connection_error)?;
        let vault_id = Uuid::parse_str(id).map_err(|_| error("Invalid sync vault ID"))?;
        if username.is_empty() {
            return Err(error("Missing vault sync account"));
        }
        Ok(Self {
            server,
            username: username.to_owned(),
            vault_id,
            organization,
            key_epoch: 0,
        })
    }

    pub(crate) fn config(&self) -> String {
        match self.organization {
            Some(org) => format!("{ORG_PREFIX}{}{org}/{}", self.server, self.vault_id),
            None => format!("{PREFIX}{}{}", self.server, self.vault_id),
        }
    }

    pub(crate) fn matches_account(&self, server: &str, username: &str) -> bool {
        self.server.as_str() == server && self.username == username
    }

    pub(crate) fn identity(&self) -> String {
        format!("{}#{}", self.config(), self.username)
    }

    #[cfg(desktop)]
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

    #[cfg(desktop)]
    pub(crate) fn shared(
        account: &sync_account_commands::AccountView,
        organization: Uuid,
        vault_id: Uuid,
    ) -> Result<Self, VaultError> {
        let mut target = Self::from_account(account, vault_id)?;
        target.organization = Some(organization);
        Ok(target)
    }

    pub(crate) async fn request(
        &self,
        request: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, VaultError> {
        sync_account_commands::vault_request(&self.server, &self.username, request)
            .await
            .map_err(connection_error)
    }

    pub(crate) fn api(&self, path: &str) -> Url {
        let mut url = self.server.clone();
        url.set_path(path);
        url
    }

    pub(crate) fn vault_path(&self) -> String {
        match self.organization {
            Some(org) => format!("/api/v1/organizations/{org}/vaults/{}", self.vault_id),
            None => format!("/api/v1/vaults/{}", self.vault_id),
        }
    }

    fn bundle_url(&self) -> Url {
        self.api(&format!("{}/bundle", self.vault_path()))
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

    pub(crate) async fn report_applied(&self, revision: &str) {
        let url = self.api(&format!("{}/applied", self.vault_path()));
        let result = self
            .request(|http| http.post(url.clone()).header(header::IF_MATCH, revision))
            .await;
        match result {
            Ok(response)
                if matches!(
                    response.status(),
                    StatusCode::NO_CONTENT
                        | StatusCode::NOT_FOUND
                        | StatusCode::PRECONDITION_FAILED
                ) => {}
            Ok(response) => {
                tracing::debug!(vault = %self.vault_id, status = %response.status(), "Vault receipt was not accepted")
            }
            Err(error) => {
                tracing::debug!(vault = %self.vault_id, %error, "Could not send vault receipt")
            }
        }
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
            let request = if self.organization.is_some() {
                request.header("X-Reacher-Vault-Key-Epoch", self.key_epoch)
            } else {
                request
            };
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
            StatusCode::CONFLICT if self.organization.is_some() => Err(error(
                "The vault owner must finish key rotation before changes can sync",
            )),
            StatusCode::CONFLICT => Err(error(
                "This sync account already has a different personal vault",
            )),
            StatusCode::FORBIDDEN => Err(error("This vault is read-only for this account")),
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
