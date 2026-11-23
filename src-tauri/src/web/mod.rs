//! Web sessions: a device's management page (iDRAC, iLO, Proxmox VE, UniFi…)
//! shown in a window of its own.
//!
//! A web page is code from somewhere else, so the window it runs in is kept
//! away from everything the app can do:
//!
//! - **No app commands.** Web windows are labelled `web-*`. No capability in
//!   `capabilities/` names that label and none allows remote URLs, so Tauri
//!   refuses plugin commands, and app commands from a remote page, by its own
//!   ACL. On top of that, the app's invoke handler (lib.rs) turns away every
//!   message from a `web-*` window whatever its origin, so a page that gets
//!   the window onto a local URL (`tauri://`, `*.localhost`) still reaches
//!   nothing. The page cannot leave http(s) either (`navigation_allowed`).
//! - **Its own storage.** Each session keeps its cookies and storage in a
//!   folder of its own (a data store on macOS), never the app's: sessions
//!   only share a login when the user gave them the same profile name.
//! - **Certificates trusted per session.** A certificate the system does not
//!   trust (a device's self-signed one) is accepted only if this session has
//!   its SHA-256 fingerprint, which the user agreed to when first asked
//!   (`certs`). Nothing turns certificate checks off globally.
//! - **Sign-in from the backend.** The password goes from the vault (or a
//!   password manager) straight into the page, and only into a page whose
//!   origin is the session's own (`autologin`); the app's own page never
//!   sees it.

pub mod autologin;
pub mod certs;
#[cfg(desktop)]
pub mod window;

#[cfg(target_os = "linux")]
mod platform_linux;
#[cfg(target_os = "macos")]
mod platform_macos;
#[cfg(target_os = "windows")]
mod platform_windows;

use serde::{Deserialize, Serialize};

/// Labels of web session windows start with this. Nothing else may use it:
/// the invoke handler refuses every window whose label does.
pub const WEB_PREFIX: &str = "web-";

/// Whether a window label is a web session's (a page from elsewhere).
pub fn is_web_label(label: &str) -> bool {
    label.starts_with(WEB_PREFIX)
}

/// A web session's settings, kept on the session.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WebOptions {
    /// `https` (unset) or `http`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// Path and query after the address (`/restgui/start.html`); unset: `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Where it opens: unset in its own window, `browser` in the system's
    /// browser (no sign-in, no pinning: the browser decides).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_in: Option<String>,
    /// Cookies and storage: unset kept for this session alone, `profile`
    /// shared with every session of the same `profile` name, `temporary`
    /// forgotten when the window closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    /// The shared profile's name (`storage` = `profile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// How the login form is filled: unset `auto` (the known ones, then
    /// any password field), `off`, a device (`idrac`, `ilo`, `proxmox`,
    /// `unifi`), `generic` or `custom` (the selectors below).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    /// `custom`: CSS selector of the user name field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_selector: Option<String>,
    /// `custom`: CSS selector of the password field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_selector: Option<String>,
    /// `custom`: CSS selector of the button that logs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_selector: Option<String>,
    /// Press the login button once filled; unset: yes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<bool>,
    /// Let the page open windows of its own (an iDRAC or iLO virtual
    /// console), as more windows of this session; unset: yes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popups: Option<bool>,
    /// Certificates this session trusts although the system does not (a
    /// device's self-signed one). Changed only by answering the prompt and
    /// by `web_session_cert_remove`; an editor's save keeps the stored ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certs: Vec<WebCert>,
}

/// One certificate a web session trusts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebCert {
    /// SHA-256 of the certificate (DER), as `AB:CD:…`.
    pub sha256: String,
    /// The server it was first seen on, to show; trust is by fingerprint.
    #[serde(default)]
    pub host: String,
    /// When it was trusted (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<i64>,
}

/// What `storage` says, read strictly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    Session,
    Profile(String),
    Temporary,
}

impl WebOptions {
    /// Values nobody knows are dropped, so they read as the default.
    pub fn tidy(&mut self) {
        fn trimmed(v: &mut Option<String>) {
            if let Some(s) = v.as_mut() {
                *s = s.trim().to_string();
            }
            if v.as_deref().is_some_and(str::is_empty) {
                *v = None;
            }
        }
        for v in [
            &mut self.scheme,
            &mut self.path,
            &mut self.open_in,
            &mut self.storage,
            &mut self.profile,
            &mut self.login,
            &mut self.username_selector,
            &mut self.password_selector,
            &mut self.submit_selector,
        ] {
            trimmed(v);
        }
        if self.scheme.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("https")) || !matches!(self.scheme.as_deref(), None | Some("http")) {
            self.scheme = None;
        }
        if !matches!(self.open_in.as_deref(), None | Some("browser")) {
            self.open_in = None;
        }
        if !matches!(self.storage.as_deref(), None | Some("profile" | "temporary")) {
            self.storage = None;
        }
        if self.storage.as_deref() == Some("profile") && self.profile.is_none() {
            self.storage = None;
        }
        if self.storage.as_deref() != Some("profile") {
            self.profile = None;
        }
        if self.login.as_deref() == Some("auto") || !matches!(self.login.as_deref(), None | Some("off" | "generic" | "custom")) && !autologin::is_preset(self.login.as_deref().unwrap_or("")) {
            self.login = None;
        }
        if self.login.as_deref() != Some("custom") {
            self.username_selector = None;
            self.password_selector = None;
            self.submit_selector = None;
        }
        if self.submit == Some(true) {
            self.submit = None;
        }
        if self.popups == Some(true) {
            self.popups = None;
        }
        if let Some(p) = self.path.as_mut() {
            if !p.starts_with('/') {
                p.insert(0, '/');
            }
        }
    }

    pub fn storage(&self) -> Storage {
        match (self.storage.as_deref(), self.profile.as_deref()) {
            (Some("profile"), Some(p)) if !p.trim().is_empty() => Storage::Profile(p.trim().to_string()),
            (Some("temporary"), _) => Storage::Temporary,
            _ => Storage::Session,
        }
    }

    pub fn login_mode(&self) -> &str {
        self.login.as_deref().unwrap_or("auto")
    }
}

/// The address a web session opens: scheme, host (an IPv6 one in
/// brackets), the port unless it is the scheme's own, and the path.
pub fn session_url(host: &str, port: u16, opts: &WebOptions) -> Result<url::Url, String> {
    let scheme = opts.scheme.as_deref().unwrap_or("https");
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err("The session has no address".into());
    }
    if host.contains(['/', '?', '#', '@', ' ']) {
        return Err(format!("Not a host name or address: {host}"));
    }
    let host_part = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let default_port = if scheme == "http" { 80 } else { 443 };
    let port_part = if port == 0 || port == default_port { String::new() } else { format!(":{port}") };
    let path = opts.path.as_deref().unwrap_or("/");
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    url::Url::parse(&format!("{scheme}://{host_part}{port_part}{path}")).map_err(|e| format!("Not a valid address: {e}"))
}

/// Whether a web window may go to `url`. Only the web: never the app's own
/// pages (`tauri://`, `*.localhost` on Windows), files, or other schemes.
pub fn navigation_allowed(url: &url::Url) -> bool {
    match url.scheme() {
        "http" | "https" => !url.host_str().is_some_and(|h| {
            let h = h.to_ascii_lowercase();
            h.ends_with(".localhost") && h != "localhost"
        }),
        "about" => url.as_str() == "about:blank",
        // Pages the site makes itself (a generated file, an inline frame).
        "blob" | "data" => true,
        _ => false,
    }
}

/// Fingerprint text as browsers show it: upper-case hex pairs and colons.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(der);
    digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// A fingerprint as typed or pasted (any case, with or without colons or
/// spaces, with an `SHA256:` or `sha256/` in front), in the stored form.
pub fn parse_fingerprint(text: &str) -> Option<String> {
    let t = text.trim();
    let t = t
        .strip_prefix("SHA256:")
        .or_else(|| t.strip_prefix("sha256:"))
        .or_else(|| t.strip_prefix("SHA-256:"))
        .or_else(|| t.strip_prefix("sha256/"))
        .unwrap_or(t);
    let hex: String = t.chars().filter(|c| !matches!(c, ':' | ' ' | '-')).collect();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let up = hex.to_ascii_uppercase();
    Some(up.as_bytes().chunks(2).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> WebOptions {
        WebOptions::default()
    }

    #[test]
    fn url_leaves_out_the_default_port() {
        assert_eq!(session_url("idrac.example.com", 443, &opts()).unwrap().as_str(), "https://idrac.example.com/");
        assert_eq!(session_url("10.0.0.5", 8006, &opts()).unwrap().as_str(), "https://10.0.0.5:8006/");
        let http = WebOptions { scheme: Some("http".into()), path: Some("/login.html".into()), ..opts() };
        assert_eq!(session_url("10.0.0.5", 80, &http).unwrap().as_str(), "http://10.0.0.5/login.html");
        assert_eq!(session_url("10.0.0.5", 443, &http).unwrap().as_str(), "http://10.0.0.5:443/login.html");
    }

    #[test]
    fn url_brackets_ipv6() {
        assert_eq!(session_url("fd00::5", 443, &opts()).unwrap().as_str(), "https://[fd00::5]/");
        assert_eq!(session_url("[fd00::5]", 8443, &opts()).unwrap().as_str(), "https://[fd00::5]:8443/");
    }

    #[test]
    fn url_refuses_a_host_with_a_path_or_login() {
        assert!(session_url("evil.example.com/x", 443, &opts()).is_err());
        assert!(session_url("user@example.com", 443, &opts()).is_err());
        assert!(session_url("", 443, &opts()).is_err());
    }

    #[test]
    fn navigation_stays_on_the_web() {
        let ok = |u: &str| navigation_allowed(&url::Url::parse(u).unwrap());
        assert!(ok("https://idrac.example.com/login.html"));
        assert!(ok("http://10.0.0.5:8080/"));
        assert!(ok("http://localhost:8080/"));
        assert!(ok("about:blank"));
        assert!(!ok("tauri://localhost/"));
        assert!(!ok("http://tauri.localhost/"));
        assert!(!ok("https://ipc.localhost/plugin"));
        assert!(!ok("asset://localhost/etc/passwd"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("about:config"));
    }

    #[test]
    fn fingerprints_read_and_print_alike() {
        let fp = fingerprint(b"hello");
        assert_eq!(fp.len(), 32 * 3 - 1);
        assert_eq!(parse_fingerprint(&fp).as_deref(), Some(fp.as_str()));
        assert_eq!(parse_fingerprint(&fp.replace(':', "").to_lowercase()).as_deref(), Some(fp.as_str()));
        assert_eq!(parse_fingerprint(&format!("SHA256: {fp}")).as_deref(), Some(fp.as_str()));
        assert_eq!(parse_fingerprint("AB:CD"), None);
        assert_eq!(parse_fingerprint(&"zz".repeat(32)), None);
    }

    #[test]
    fn tidy_drops_what_nobody_knows() {
        let mut o = WebOptions {
            scheme: Some("ftp".into()),
            storage: Some("profile".into()),
            profile: Some("  ".into()),
            login: Some("nonsense".into()),
            username_selector: Some("#u".into()),
            path: Some("login.html".into()),
            submit: Some(true),
            popups: Some(true),
            ..opts()
        };
        o.tidy();
        assert_eq!(o.popups, None);
        assert_eq!(o.scheme, None);
        assert_eq!(o.storage, None);
        assert_eq!(o.profile, None);
        assert_eq!(o.login, None);
        assert_eq!(o.username_selector, None);
        assert_eq!(o.path.as_deref(), Some("/login.html"));
        assert_eq!(o.submit, None);

        let mut o = WebOptions { login: Some("idrac".into()), storage: Some("profile".into()), profile: Some(" PVE ".into()), ..opts() };
        o.tidy();
        assert_eq!(o.login.as_deref(), Some("idrac"));
        assert_eq!(o.storage(), Storage::Profile("PVE".into()));
    }
}
