//! S3 sessions: buckets and objects of Amazon S3 or any S3-compatible store
//! (MinIO, Cloudflare R2, Backblaze B2, Wasabi, Ceph, Garage…), shown in
//! the full-pane file manager the way an FTP session's files are. The
//! `sftp_*` commands send a connection id that belongs to one of these here
//! (see `ipc::sftp_commands`), so the file manager needs no S3 of its own.
//!
//! Paths are `/bucket/key`: the root lists the buckets, and a key's `/`
//! separated prefixes are its folders. A folder made here is an empty
//! object named `prefix/`, as the AWS console makes them.
//!
//! Requests are signed with AWS Signature Version 4 (`sign`), so no SDK is
//! needed, over our own TLS with certificate pinning for self-signed
//! stores.

pub mod ops;
pub mod sign;
pub mod xml;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long reaching the store may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a store may stay silent within one answer.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// An S3 session's own settings. The access key is the session's user
/// name, the secret key its password.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct S3Options {
    /// The region requests are signed for; from the host name unless set
    /// (`s3.<region>.amazonaws.com`, …), else `us-east-1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// `host/bucket` addresses (true) or `bucket.host` (false); unset:
    /// `bucket.host` on Amazon, `host/bucket` elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_style: Option<bool>,
    /// The bucket the file manager opens in; keys that may not list all
    /// buckets need one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    /// Plain HTTP, for a store on this computer or network (MinIO on 9000).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub http: bool,
    /// SHA-256 fingerprints of certificates this session trusts although
    /// the system does not. Changed only by answering the prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certs: Vec<String>,
}

impl S3Options {
    pub fn is_empty(&self) -> bool {
        *self == S3Options::default()
    }
}

#[derive(Debug)]
pub enum S3Error {
    /// The store's certificate is trusted by neither the system nor the
    /// session: its fingerprint.
    UntrustedCert(String),
    /// The store refused the keys.
    Login(String),
    /// Not there (404).
    NotFound(String),
    Cancelled,
    Other(String),
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            S3Error::UntrustedCert(fp) => write!(f, "The store's certificate is not trusted ({fp})"),
            S3Error::Cancelled => f.write_str("Transfer cancelled"),
            S3Error::Login(m) | S3Error::NotFound(m) | S3Error::Other(m) => f.write_str(m),
        }
    }
}

/// What a connect needs.
pub struct Params {
    pub host: String,
    pub port: u16,
    pub http: bool,
    pub region: Option<String>,
    pub path_style: Option<bool>,
    pub bucket: Option<String>,
    pub access_key: String,
    pub secret_key: String,
    pub trusted: Vec<String>,
    /// A local SOCKS port into the session's WireGuard tunnel.
    pub socks: Option<u16>,
}

/// One open S3 session.
pub struct S3Conn {
    http: reqwest::Client,
    scheme: &'static str,
    /// `host[:port]`, as requests name it.
    authority: String,
    host_lower: String,
    region: Mutex<String>,
    path_style: Option<bool>,
    pub bucket: Option<String>,
    access_key: String,
    secret_key: String,
    refused: crate::ftp::tls::RefusedSlot,
}

/// A request's body: bytes in memory, signed by their hash.
pub enum Body {
    Empty,
    Bytes(bytes::Bytes),
}

/// The `<Code>` and `<Message>` of an S3 error answer.
fn error_parts(body: &str) -> (String, String) {
    (xml::text(body, "Code").unwrap_or_default(), xml::text(body, "Message").unwrap_or_default())
}

/// The `/`-encoded path of a key, with `.` and `..` segments escaped so no
/// URL handling folds them away.
pub fn encode_key(key: &str) -> String {
    key.split('/')
        .map(|seg| if seg == "." || seg == ".." { seg.replace('.', "%2E") } else { sign::encode(seg, false) })
        .collect::<Vec<_>>()
        .join("/")
}

/// `/bucket/a/b` as its bucket and key; the root has neither.
pub fn split(path: &str) -> (Option<String>, String) {
    let p = path.trim_matches('/');
    if p.is_empty() {
        return (None, String::new());
    }
    match p.split_once('/') {
        Some((b, k)) => (Some(b.to_string()), k.trim_end_matches('/').to_string()),
        None => (Some(p.to_string()), String::new()),
    }
}

pub fn valid_bucket(b: &str) -> bool {
    (3..=63).contains(&b.len()) && b.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') && !b.starts_with(['-', '.'])
}

impl S3Conn {
    pub fn new(p: &Params) -> Result<Self, S3Error> {
        if p.access_key.trim().is_empty() || p.secret_key.is_empty() {
            return Err(S3Error::Login("The access key and secret key are both needed".into()));
        }
        let (tls, refused) = crate::ftp::tls::pinning_config(&p.trusted).map_err(S3Error::Other)?;
        let http = crate::http::client_builder().tls_backend_preconfigured(tls).connect_timeout(CONNECT_TIMEOUT).read_timeout(READ_TIMEOUT);
        let http = match p.socks {
            Some(port) => http.proxy(reqwest::Proxy::all(format!("socks5h://127.0.0.1:{port}")).map_err(|e| S3Error::Other(e.to_string()))?),
            None => http.no_proxy(),
        };
        // A redirect would need signing again for another host.
        let http = http.redirect(reqwest::redirect::Policy::none()).build().map_err(|e| S3Error::Other(format!("HTTPS setup failed: {e}")))?;
        let host = p.host.trim().trim_end_matches('/');
        let host = host.strip_prefix("https://").or_else(|| host.strip_prefix("http://")).unwrap_or(host).to_ascii_lowercase();
        let scheme = if p.http { "http" } else { "https" };
        let default_port = if p.http { 80 } else { 443 };
        let bracketed = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.clone() };
        let authority = if p.port == 0 || p.port == default_port { bracketed } else { format!("{bracketed}:{}", p.port) };
        let region = p.region.clone().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).unwrap_or_else(|| crate::vault::sync_s3::region_of(&host));
        Ok(S3Conn {
            http,
            scheme,
            authority,
            host_lower: host,
            region: Mutex::new(region),
            path_style: p.path_style,
            bucket: p.bucket.clone().map(|b| b.trim().to_string()).filter(|b| !b.is_empty()),
            access_key: p.access_key.trim().to_string(),
            secret_key: p.secret_key.clone(),
            refused,
        })
    }

    fn region(&self) -> String {
        self.region.lock().map(|r| r.clone()).unwrap_or_else(|_| "us-east-1".into())
    }

    fn virtual_host(&self, bucket: &str) -> bool {
        match self.path_style {
            Some(path) => !path,
            None => self.host_lower.ends_with(".amazonaws.com") && !bucket.contains('.'),
        }
    }

    /// The Host a request is signed for and its encoded path.
    fn target(&self, bucket: Option<&str>, key: &str) -> (String, String) {
        match bucket {
            None => (self.authority.clone(), "/".into()),
            Some(b) if self.virtual_host(b) => (format!("{b}.{}", self.authority), format!("/{}", encode_key(key))),
            Some(b) if key.is_empty() => (self.authority.clone(), format!("/{}", sign::encode(b, false))),
            Some(b) => (self.authority.clone(), format!("/{}/{}", sign::encode(b, false), encode_key(key))),
        }
    }

    fn net_error(&self, e: reqwest::Error) -> S3Error {
        if let Some(fp) = self.refused.lock().ok().and_then(|mut r| r.take()) {
            return S3Error::UntrustedCert(fp);
        }
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg = format!("{msg}: {s}");
            src = s.source();
        }
        if e.is_timeout() {
            return S3Error::Other(format!("The store did not answer in time ({msg})"));
        }
        S3Error::Other(format!("Could not reach the store: {msg}"))
    }

    /// Send a signed request; a store that names another region for the
    /// bucket gets it once more, signed for that one. Fails on an error
    /// answer, with its code and message.
    pub async fn send(
        &self,
        method: reqwest::Method,
        bucket: Option<&str>,
        key: &str,
        query: &[(String, String)],
        body: Body,
        headers: &[(&str, String)],
    ) -> Result<reqwest::Response, S3Error> {
        // Web addresses fold `.` and `..` (even written %2E), so such a key
        // would reach another object: refused.
        if key.split('/').any(|s| s == "." || s == "..") {
            return Err(S3Error::Other(format!("Names \".\" and \"..\" cannot be used in S3 here: {key}")));
        }
        let bytes = match body {
            Body::Empty => None,
            Body::Bytes(b) => Some(b),
        };
        let mut retried = false;
        loop {
            let response = self.send_once(method.clone(), bucket, key, query, bytes.clone(), headers).await?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let text = response.text().await.unwrap_or_default();
            let (code, message) = error_parts(&text);
            if !retried && matches!(code.as_str(), "AuthorizationHeaderMalformed" | "PermanentRedirect" | "IllegalLocationConstraintException") {
                if let Some(region) = xml::text(&text, "Region").filter(|r| !r.is_empty() && *r != self.region()) {
                    if let Ok(mut r) = self.region.lock() {
                        *r = region;
                    }
                    retried = true;
                    continue;
                }
            }
            return Err(self.explain(status, &code, &message, bucket, key));
        }
    }

    async fn send_once(
        &self,
        method: reqwest::Method,
        bucket: Option<&str>,
        key: &str,
        query: &[(String, String)],
        body: Option<bytes::Bytes>,
        headers: &[(&str, String)],
    ) -> Result<reqwest::Response, S3Error> {
        let (host, path) = self.target(bucket, key);
        let q = sign::canonical_query(query);
        let payload = match &body {
            Some(b) => sign::sha256_hex(b),
            None => sign::EMPTY_SHA256.to_string(),
        };
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let mut signed: Vec<(String, String)> = vec![("host".into(), host.clone()), ("x-amz-content-sha256".into(), payload.clone()), ("x-amz-date".into(), amz_date.clone())];
        for (k, v) in headers {
            signed.push((k.to_ascii_lowercase(), v.clone()));
        }
        let pairs: Vec<(&str, &str)> = signed.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let region = self.region();
        let creds = sign::Credentials { access_key: &self.access_key, secret_key: &self.secret_key, region: &region };
        let auth = sign::authorization(&creds, method.as_str(), &path, &q, &pairs, &payload, &amz_date);
        let url = if q.is_empty() { format!("{}://{host}{path}", self.scheme) } else { format!("{}://{host}{path}?{q}", self.scheme) };
        let mut request = self.http.request(method, &url).header("authorization", auth);
        for (k, v) in &signed {
            if k != "host" {
                request = request.header(k.as_str(), v.as_str());
            }
        }
        if let Some(b) = body {
            request = request.body(b);
        }
        request.send().await.map_err(|e| self.net_error(e))
    }

    fn explain(&self, status: reqwest::StatusCode, code: &str, message: &str, bucket: Option<&str>, key: &str) -> S3Error {
        let what = match bucket {
            Some(b) if key.is_empty() => b.to_string(),
            Some(b) => format!("{b}/{key}"),
            None => self.authority.clone(),
        };
        match code {
            "InvalidAccessKeyId" => S3Error::Login("The store does not know that access key".into()),
            "SignatureDoesNotMatch" => S3Error::Login("The secret key is wrong".into()),
            "RequestTimeTooSkewed" => S3Error::Other("This computer's clock is too far off for the store".into()),
            "NoSuchBucket" => S3Error::NotFound(format!("The bucket {} does not exist", bucket.unwrap_or_default())),
            "NoSuchKey" => S3Error::NotFound(format!("{what} does not exist")),
            "AccessDenied" | "AllAccessDisabled" => S3Error::Other(format!("Access denied to {what}{}", if message.is_empty() { String::new() } else { format!(": {message}") })),
            "BucketNotEmpty" => S3Error::Other(format!("The bucket {what} is not empty")),
            "AuthorizationHeaderMalformed" | "PermanentRedirect" => S3Error::Other(format!("The bucket is in another region than {}: set its region", self.region())),
            _ if status == reqwest::StatusCode::NOT_FOUND => S3Error::NotFound(format!("{what} does not exist")),
            _ if status == reqwest::StatusCode::FORBIDDEN && code.is_empty() => S3Error::Other(format!("Access denied to {what}")),
            "" => S3Error::Other(format!("The store answered {status} for {what}")),
            other => S3Error::Other(format!("The store answered {status} ({other}{}) for {what}", if message.is_empty() { String::new() } else { format!(": {message}") })),
        }
    }

    /// A link anyone can download `bucket/key` with for `expires` seconds.
    pub fn presign(&self, bucket: &str, key: &str, expires: u32) -> String {
        let (host, path) = self.target(Some(bucket), key);
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let region = self.region();
        let creds = sign::Credentials { access_key: &self.access_key, secret_key: &self.secret_key, region: &region };
        let q = sign::presigned_query(&creds, &host, &path, expires, &amz_date);
        format!("{}://{host}{path}?{q}", self.scheme)
    }

    /// Check the keys: list the buckets, or the session's bucket.
    pub async fn check(&self) -> Result<(), S3Error> {
        match &self.bucket {
            Some(b) => {
                let q = vec![("list-type".to_string(), "2".to_string()), ("max-keys".to_string(), "1".to_string())];
                self.send(reqwest::Method::GET, Some(b), "", &q, Body::Empty, &[]).await.map(|_| ())
            }
            None => self.send(reqwest::Method::GET, None, "", &[], Body::Empty, &[]).await.map(|_| ()),
        }
    }

    /// Where the file manager opens.
    pub fn home(&self) -> String {
        match &self.bucket {
            Some(b) => format!("/{b}"),
            None => "/".into(),
        }
    }
}

/// The open S3 sessions, by connection id.
#[derive(Default)]
pub struct S3Manager {
    conns: std::sync::Mutex<HashMap<String, Arc<S3Conn>>>,
}

impl S3Manager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<S3Conn>> {
        self.conns.lock().ok()?.get(id).cloned()
    }

    pub fn insert(&self, id: String, conn: S3Conn) {
        if let Ok(mut m) = self.conns.lock() {
            m.insert(id, Arc::new(conn));
        }
    }

    pub fn remove(&self, id: &str) -> Option<Arc<S3Conn>> {
        self.conns.lock().ok()?.remove(id)
    }
}

#[cfg(test)]
#[path = "mock_tests.rs"]
mod mock_tests;
