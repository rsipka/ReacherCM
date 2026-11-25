//! A small Redfish client: HTTPS with the session's pinned certificates,
//! a Redfish login session (`X-Auth-Token`) or HTTP Basic where the BMC has
//! no session service, and one new login when a token has run out.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use serde_json::Value;

/// How long a BMC may take to answer one request: an iDRAC can take many
/// seconds for a big collection.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum BmcError {
    /// TLS refused a certificate nothing trusts; its SHA-256 fingerprint.
    UntrustedCert(String),
    /// The user name or password was refused.
    Login(String),
    /// The BMC needs a licence (iDRAC Enterprise, iLO Advanced, XCC
    /// Advanced, Supermicro's OOB key...) for this; its own message.
    Licence(String),
    /// The BMC answered with an error status.
    Status { code: u16, message: String },
    /// No answer, or one that is not Redfish.
    Other(String),
}

impl std::fmt::Display for BmcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BmcError::UntrustedCert(fp) => write!(f, "Untrusted certificate {fp}"),
            BmcError::Login(m) => write!(f, "Login refused: {m}"),
            BmcError::Licence(m) => write!(f, "The controller needs a licence for this: {m}"),
            BmcError::Status { code, message } if message.is_empty() => write!(f, "The BMC answered {code}"),
            BmcError::Status { code, message } => write!(f, "{message} ({code})"),
            BmcError::Other(m) => f.write_str(m),
        }
    }
}

impl BmcError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, BmcError::Status { code: 404 | 405 | 501, .. })
    }
}

/// What a login gave.
#[derive(Debug, Clone)]
enum Auth {
    /// A Redfish session: its token, and where to delete it at the end.
    Token { token: String, location: Option<String> },
    /// HTTP Basic on every request.
    Basic,
}

pub struct Params {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Fingerprints (`AB:CD:..`) of certificates trusted for this session.
    pub trusted: Vec<String>,
    /// A local SOCKS5 proxy into the session's WireGuard tunnel, and the
    /// hold that keeps the tunnel on while the session is open.
    pub socks: Option<u16>,
    pub tunnel: Option<crate::wireguard::manager::Hold>,
}

pub struct Redfish {
    http: reqwest::Client,
    /// `https://host:port`, no trailing slash.
    base: String,
    username: String,
    password: String,
    auth: tokio::sync::Mutex<Auth>,
    refused: Arc<Mutex<Option<String>>>,
}

/// A Redfish resource path as the BMC wrote it (`/redfish/v1/...`), checked
/// so that nothing outside the service is ever requested.
pub fn valid_path(path: &str) -> bool {
    path.starts_with("/redfish/v1")
        && !path.contains("..")
        && !path.contains('\\')
        && !path.chars().any(|c| c.is_control() || c == ' ')
        && !path.contains("://")
}

/// A host for a URL: an IPv6 address in brackets.
pub(crate) fn url_host(host: &str) -> String {
    let h = host.trim().trim_matches(['[', ']']);
    if h.contains(':') { format!("[{h}]") } else { h.to_string() }
}

/// Whether a Redfish error says a licence is missing: Dell's LIC messages,
/// HPE's LicenseKeyRequired, Lenovo's and Supermicro's wording.
pub fn needs_licence(body: &Value) -> bool {
    let err = &body["error"];
    let ids = err["@Message.ExtendedInfo"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["MessageId"].as_str()).map(str::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    let text = format!("{} {}", error_message(body), err["code"].as_str().unwrap_or_default()).to_ascii_lowercase();
    ids.iter().any(|id| id.contains("License") || id.contains("Licence") || id.split('.').next_back().is_some_and(|l| l.starts_with("LIC")))
        || text.contains("license")
        || text.contains("licence")
}

/// The message a Redfish error body carries, or nothing.
pub fn error_message(body: &Value) -> String {
    let err = &body["error"];
    let ext: Vec<String> = err["@Message.ExtendedInfo"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["Message"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    if !ext.is_empty() {
        return ext.join(" ");
    }
    err["message"].as_str().unwrap_or_default().to_string()
}

impl Redfish {
    pub fn new(p: &Params) -> Result<Self, BmcError> {
        let (tls, refused) = crate::ftp::tls::pinning_config(&p.trusted).map_err(BmcError::Other)?;
        let http = crate::http::client_builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // A BMC's links are its own; never follow one elsewhere.
            .redirect(reqwest::redirect::Policy::none());
        // Never the system's proxy; the tunnel's own when the session goes
        // through WireGuard (names resolved inside it: socks5h).
        let http = match p.socks {
            Some(port) => http.proxy(reqwest::Proxy::all(format!("socks5h://127.0.0.1:{port}")).map_err(|e| BmcError::Other(e.to_string()))?),
            None => http.no_proxy(),
        };
        let http = http
            .build()
            .map_err(|e| BmcError::Other(format!("HTTPS setup failed: {e}")))?;
        let port = if p.port == 0 { 443 } else { p.port };
        Ok(Self {
            http,
            base: format!("https://{}:{port}", url_host(&p.host)),
            username: p.username.clone(),
            password: p.password.clone(),
            auth: tokio::sync::Mutex::new(Auth::Basic),
            refused,
        })
    }

    fn net_error(&self, e: reqwest::Error) -> BmcError {
        if let Some(fp) = self.refused.lock().ok().and_then(|mut r| r.take()) {
            return BmcError::UntrustedCert(fp);
        }
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg = format!("{msg}: {s}");
            src = s.source();
        }
        if e.is_timeout() {
            return BmcError::Other(format!("The BMC did not answer in time ({msg})"));
        }
        BmcError::Other(msg)
    }

    /// Read the service root (no login needed), then log in: a Redfish
    /// session where there is one, else Basic, checked on the systems.
    pub async fn login(&self) -> Result<Value, BmcError> {
        let root = self.send(Method::GET, "/redfish/v1/", None, None, false).await?.0;
        if root.get("RedfishVersion").is_none() && root.get("@odata.id").is_none() {
            return Err(BmcError::Other("This address does not answer as a Redfish service".into()));
        }
        let sessions = root["Links"]["Sessions"]["@odata.id"]
            .as_str()
            .filter(|p| valid_path(p))
            .unwrap_or("/redfish/v1/SessionService/Sessions")
            .to_string();
        let body = serde_json::json!({ "UserName": self.username, "Password": self.password });
        let res = self
            .http
            .post(format!("{}{sessions}", self.base))
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| self.net_error(e))?;
        let status = res.status();
        if status.is_success() {
            let token = res.headers().get("X-Auth-Token").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let location = res
                .headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .map(|l| self.local_path(l));
            let body: Value = res.json().await.unwrap_or(Value::Null);
            let location = location.or_else(|| body["@odata.id"].as_str().map(str::to_owned)).filter(|l| valid_path(l));
            if let Some(token) = token {
                *self.auth.lock().await = Auth::Token { token, location };
                return Ok(root);
            }
        } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            let body: Value = res.json().await.unwrap_or(Value::Null);
            return Err(BmcError::Login(error_message(&body)));
        }
        // No session service: Basic, checked on something that needs a login.
        *self.auth.lock().await = Auth::Basic;
        match self.send(Method::GET, "/redfish/v1/Systems", None, None, false).await {
            Ok(_) => Ok(root),
            Err(BmcError::Status { code: 401 | 403, message }) => Err(BmcError::Login(message)),
            Err(e) => Err(e),
        }
    }

    /// A `Location` the BMC sent, as a path on this service.
    fn local_path(&self, location: &str) -> String {
        match location.strip_prefix(&self.base) {
            Some(p) => p.to_string(),
            None => match url::Url::parse(location) {
                Ok(u) => u.path().to_string(),
                Err(_) => location.to_string(),
            },
        }
    }

    /// Delete the login session, so it does not hold one of the BMC's few
    /// session slots until it times out.
    pub async fn logout(&self) {
        let auth = self.auth.lock().await.clone();
        if let Auth::Token { location: Some(loc), .. } = auth {
            let _ = self.send(Method::DELETE, &loc, None, None, false).await;
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<&str>,
        retry: bool,
    ) -> Result<(Value, HeaderMap), BmcError> {
        if !valid_path(path) {
            return Err(BmcError::Other(format!("Not a Redfish path: {path}")));
        }
        let auth = self.auth.lock().await.clone();
        let mut req = self.http.request(method.clone(), format!("{}{path}", self.base)).header("Accept", "application/json");
        req = match &auth {
            Auth::Token { token, .. } => req.header("X-Auth-Token", HeaderValue::from_str(token).map_err(|e| BmcError::Other(e.to_string()))?),
            Auth::Basic => req.basic_auth(&self.username, Some(&self.password)),
        };
        if let Some(tag) = if_match {
            req = req.header("If-Match", tag);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req.send().await.map_err(|e| self.net_error(e))?;
        let status = res.status();
        let headers = res.headers().clone();
        let text = res.text().await.map_err(|e| self.net_error(e))?;
        let value: Value = if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::Null) };
        if status == StatusCode::UNAUTHORIZED && retry && matches!(auth, Auth::Token { .. }) {
            // The token ran out (idle timeout): log in once more.
            Box::pin(self.login()).await?;
            return Box::pin(self.send(method, path, body, if_match, false)).await;
        }
        if !status.is_success() {
            if needs_licence(&value) {
                return Err(BmcError::Licence(error_message(&value)));
            }
            return Err(BmcError::Status { code: status.as_u16(), message: error_message(&value) });
        }
        Ok((value, headers))
    }

    pub async fn get(&self, path: &str) -> Result<Value, BmcError> {
        self.send(Method::GET, path, None, None, true).await.map(|(v, _)| v)
    }

    /// A resource and its ETag, for a PATCH that must name it.
    pub async fn get_tagged(&self, path: &str) -> Result<(Value, Option<String>), BmcError> {
        let (v, h) = self.send(Method::GET, path, None, None, true).await?;
        let tag = h
            .get("ETag")
            .and_then(|t| t.to_str().ok())
            .map(str::to_owned)
            .or_else(|| v["@odata.etag"].as_str().map(str::to_owned));
        Ok((v, tag))
    }

    /// POST an action; the BMC's message, if it sent one.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Option<String>, BmcError> {
        let (v, _) = self.send(Method::POST, path, Some(body), None, true).await?;
        Ok(Some(error_message(&v)).filter(|m| !m.is_empty()))
    }

    /// POST an action and keep what it answered (an action that returns data).
    pub async fn post_for(&self, path: &str, body: &Value) -> Result<Value, BmcError> {
        self.send(Method::POST, path, Some(body), None, true).await.map(|(v, _)| v)
    }

    /// `https://host:port`, as the connection was opened.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// The password this connection logged in with, for the console page's
    /// sign-in (`crate::web::autologin`), which only types it into a page of
    /// the controller's own origin.
    pub(crate) fn password(&self) -> &str {
        &self.password
    }

    pub async fn patch(&self, path: &str, body: &Value, etag: Option<&str>) -> Result<(), BmcError> {
        self.send(Method::PATCH, path, Some(body), etag, true).await.map(|_| ())
    }

    pub async fn delete(&self, path: &str) -> Result<(), BmcError> {
        self.send(Method::DELETE, path, None, None, true).await.map(|_| ())
    }

    /// The member paths of a collection, following `nextLink` pages, at most `max`.
    pub async fn members(&self, path: &str, max: usize) -> Result<Vec<String>, BmcError> {
        Ok(self.collection(path, max).await?.into_iter().filter_map(|m| m["@odata.id"].as_str().map(str::to_owned)).collect())
    }

    /// The members of a collection as the BMC listed them (log entries come
    /// whole; most others only as links), following pages, at most `max`.
    pub async fn collection(&self, path: &str, max: usize) -> Result<Vec<Value>, BmcError> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        let mut pages = 0;
        while let Some(p) = next.take() {
            let page = self.get(&p).await?;
            if let Some(m) = page["Members"].as_array() {
                out.extend(m.iter().cloned());
            }
            pages += 1;
            if out.len() >= max || pages >= 50 {
                break;
            }
            next = page["Members@odata.nextLink"].as_str().map(|l| self.local_path(l)).filter(|l| valid_path(l));
        }
        out.truncate(max);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_service() {
        assert!(valid_path("/redfish/v1/Systems/1"));
        assert!(valid_path("/redfish/v1/Managers/iDRAC.Embedded.1/LogServices/Sel/Entries?$skip=50"));
        assert!(!valid_path("/redfish/v1/../../etc/passwd"));
        assert!(!valid_path("/other"));
        assert!(!valid_path("/redfish/v1/x http://evil"));
        assert!(!valid_path("/redfish/v1/x\nHost: evil"));
    }

    #[test]
    fn ipv6_hosts_get_brackets() {
        assert_eq!(url_host("fe80::1"), "[fe80::1]");
        assert_eq!(url_host("[fe80::1]"), "[fe80::1]");
        assert_eq!(url_host("10.0.0.5"), "10.0.0.5");
    }

    #[test]
    fn redfish_error_messages() {
        let body = serde_json::json!({"error": {"code": "Base.1.0.GeneralError", "message": "A general error.",
            "@Message.ExtendedInfo": [{"Message": "The request failed due to an internal service error."}]}});
        assert_eq!(error_message(&body), "The request failed due to an internal service error.");
        let lic = serde_json::json!({"error": {"@Message.ExtendedInfo": [{"MessageId": "iLO.2.14.LicenseKeyRequired", "Message": "An iLO Advanced license is required."}]}});
        assert!(needs_licence(&lic));
        let dell = serde_json::json!({"error": {"@Message.ExtendedInfo": [{"MessageId": "IDRAC.2.8.LIC501", "Message": "Unable to complete the operation."}]}});
        assert!(needs_licence(&dell));
        let smc = serde_json::json!({"error": {"message": "Not licensed to perform this request. The following licenses SFT-DCMS-SINGLE were needed"}});
        assert!(needs_licence(&smc));
        assert!(!needs_licence(&serde_json::json!({"error": {"message": "Bad thing"}})));
        let plain = serde_json::json!({"error": {"message": "Bad thing"}});
        assert_eq!(error_message(&plain), "Bad thing");
        assert_eq!(error_message(&Value::Null), "");
    }
}
