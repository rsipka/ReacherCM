//! The window a web session opens in, and what the backend keeps about it
//! while it is open: whose it is, the login to type, the certificates it
//! trusts. See the module above for why it is built as it is.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tauri::webview::{NewWindowFeatures, NewWindowResponse, PageLoadEvent, PageLoadPayload};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::autologin::{self, LoginSpec};
use super::certs::{self, CertAsk, Decision};
use super::{Storage, WebCert, WEB_PREFIX};
use crate::state::SessionConfig;

#[cfg(target_os = "linux")]
use super::platform_linux as platform;
#[cfg(target_os = "macos")]
use super::platform_macos as platform;
#[cfg(target_os = "windows")]
use super::platform_windows as platform;

/// One open web session; its popups (a virtual console) share it.
pub(crate) struct WebWin {
    pub session_id: String,
    pub name: String,
    /// The session's own origin, as `location.origin` prints it.
    origin: String,
    host: String,
    username: String,
    password: Option<String>,
    /// How to fill the form; `None`: do not (off, or no password).
    spec: Option<LoginSpec>,
    submit: bool,
    /// The page may open windows (see `WebOptions::popups`).
    pub(super) popups: bool,
    /// Fingerprints accepted: the session's, and those trusted since.
    trusted: Vec<String>,
    had_certs: bool,
    /// The certificate being asked about: fingerprint, window, server.
    pending: Option<(String, String, String)>,
    /// An HTTP login (Basic/Digest) was answered once already; a second
    /// request means it was refused, and is left to the user.
    basic_used: bool,
    /// A SOCKS5 proxy into the session's WireGuard tunnel; its popups use it too.
    proxy: Option<url::Url>,
    /// Keeps that tunnel on while the window is open, though a page may sit
    /// with no connection open for a while.
    _tunnel: Option<crate::wireguard::manager::Hold>,
}

impl Drop for WebWin {
    fn drop(&mut self) {
        if let Some(p) = self.password.as_mut() {
            // Best effort; the String's buffer is overwritten before it goes.
            let len = p.len();
            p.clear();
            p.extend(std::iter::repeat_n('\0', len));
        }
    }
}

type Shared = Arc<Mutex<WebWin>>;

#[derive(Default)]
struct Registry {
    /// Every web window (popups too), by label.
    windows: HashMap<String, Shared>,
    /// The main window of each open session, by session id.
    main: HashMap<String, String>,
    /// Folders to remove when the window (label) closes.
    temporary: HashMap<String, PathBuf>,
    next: u32,
}

fn registry() -> MutexGuard<'static, Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

static APP: OnceLock<AppHandle> = OnceLock::new();

pub(crate) fn entry(label: &str) -> Option<Shared> {
    registry().windows.get(label).cloned()
}

fn lock(e: &Shared) -> MutexGuard<'_, WebWin> {
    e.lock().unwrap_or_else(|e| e.into_inner())
}

fn next_label(reg: &mut Registry, app: &AppHandle) -> String {
    loop {
        reg.next += 1;
        let l = format!("{WEB_PREFIX}{}", reg.next);
        if app.get_webview_window(&l).is_none() && !reg.windows.contains_key(&l) {
            return l;
        }
    }
}

/// Where a session keeps cookies and storage.
enum Place {
    /// A folder (Linux, Windows): its own WebKit context or WebView2
    /// browser process.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Dir(PathBuf),
    /// A data store by identifier (macOS 14 and later).
    #[cfg(target_os = "macos")]
    Store([u8; 16]),
    /// Kept in memory only (macOS: temporary, or a macOS too old for
    /// separate stores, where a lasting one would be the app's own).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Memory,
}

fn web_root() -> PathBuf {
    crate::app_data_dir().join("web")
}

/// A name usable as a folder name, for a profile.
fn folder_name(s: &str) -> String {
    let clean: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    // Two names that clean up alike ("a b", "a_b") stay apart.
    let tag = &super::fingerprint(s.as_bytes()).replace(':', "")[..8];
    format!("{}-{}", clean.chars().take(40).collect::<String>(), tag.to_ascii_lowercase())
}

fn place_for(session_id: &str, storage: &Storage, label: &str) -> Result<(Place, Option<PathBuf>), String> {
    let key = match storage {
        Storage::Session => format!("sessions/{}", folder_name(session_id)),
        Storage::Profile(p) => format!("profiles/{}", folder_name(p)),
        Storage::Temporary => format!("tmp/{label}"),
    };
    #[cfg(target_os = "macos")]
    {
        let _ = label;
        if *storage == Storage::Temporary || !platform::has_data_stores() {
            return Ok((Place::Memory, None));
        }
        Ok((Place::Store(store_id(&key)), None))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let root = web_root();
        crate::vault::private_fs::private_dir(&root).map_err(|e| format!("Could not create {}: {e}", root.display()))?;
        let dir = root.join(key);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let temporary = (*storage == Storage::Temporary).then(|| dir.clone());
        Ok((Place::Dir(dir), temporary))
    }
}

/// The data store identifier for a storage key (macOS).
#[cfg(target_os = "macos")]
fn store_id(key: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

/// Forget a session's lasting data store (macOS; see `session_data_dir`).
#[cfg(target_os = "macos")]
pub async fn clear_store(app: &AppHandle, session_id: &str) -> Result<(), String> {
    if !platform::has_data_stores() {
        return Ok(());
    }
    let id = store_id(&format!("sessions/{}", folder_name(session_id)));
    let known = app.fetch_data_store_identifiers().await.map_err(|e| e.to_string())?;
    if !known.contains(&id) {
        return Ok(());
    }
    app.remove_data_store(id).await.map_err(|e| e.to_string())
}

/// The folder a session's lasting storage is in, for Clear browser data
/// (Linux, Windows; macOS keeps a data store, see `clear_store`).
pub fn session_data_dir(session_id: &str) -> PathBuf {
    web_root().join("sessions").join(folder_name(session_id))
}

/// Temporary folders left by a crash: nothing can be using them at start.
fn clear_leftovers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::fs::remove_dir_all(web_root().join("tmp"));
    });
}

/// Open a session's page, or bring its open window to the front.
/// `password`: already fetched (vault or password manager); `None` signs
/// nothing in. `socks`: the port of a local SOCKS5 proxy all of the
/// window's traffic must go through (a WireGuard tunnel), and the hold that
/// keeps the tunnel on while the window is open.
pub fn open(app: &AppHandle, s: &SessionConfig, password: Option<String>, socks: Option<(u16, crate::wireguard::manager::Hold)>) -> Result<String, String> {
    let (socks, tunnel) = socks.unzip();
    let _ = APP.set(app.clone());
    clear_leftovers();
    let opts = s.web.clone().unwrap_or_default();
    let url = super::session_url(&s.host, s.port, &opts)?;

    let existing = registry().main.get(&s.id).cloned();
    if let Some(label) = existing {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
            return Ok(label);
        }
    }

    let spec = autologin::spec_for(
        opts.login_mode(),
        [opts.username_selector.as_deref(), opts.password_selector.as_deref(), opts.submit_selector.as_deref()],
    )
    .filter(|_| password.is_some());
    let entry = Arc::new(Mutex::new(WebWin {
        session_id: s.id.clone(),
        name: s.name.clone(),
        origin: url.origin().ascii_serialization(),
        host: url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_string(),
        username: s.username.clone(),
        password,
        spec,
        submit: opts.submit.unwrap_or(true),
        popups: opts.popups.unwrap_or(true),
        trusted: opts.certs.iter().map(|c| c.sha256.clone()).collect(),
        had_certs: !opts.certs.is_empty(),
        pending: None,
        basic_used: false,
        proxy: socks.map(|port| format!("socks5://127.0.0.1:{port}").parse().expect("a URL")),
        _tunnel: tunnel,
    }));

    let label = {
        let mut reg = registry();
        let label = next_label(&mut reg, app);
        reg.windows.insert(label.clone(), entry);
        reg.main.insert(s.id.clone(), label.clone());
        label
    };
    let built = place_for(&s.id, &opts.storage(), &label).and_then(|(place, temporary)| {
        if let Some(t) = temporary {
            registry().temporary.insert(label.clone(), t);
        }
        build(app, &label, &title(&s.name, None), Some(place), None).map_err(|e| e.to_string())
    });
    let win = match built {
        Ok(w) => w,
        Err(e) => {
            forget(&label);
            return Err(e);
        }
    };
    platform::install(&win, label.clone());
    // Queued after the hooks above, so the first load already has them.
    win.navigate(url).map_err(|e| e.to_string())?;
    tracing::info!("Web session {}: opened in {label}", s.id);
    Ok(label)
}

fn title(name: &str, page: Option<&str>) -> String {
    match page.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!("{p} · {name}"),
        None => name.to_string(),
    }
}

/// Build a web window. `place`: a new session window's storage; `features`:
/// a popup's, which shares its opener's (and so its storage) instead.
fn build(
    app: &AppHandle,
    label: &str,
    title: &str,
    place: Option<Place>,
    features: Option<NewWindowFeatures>,
) -> tauri::Result<WebviewWindow> {
    let opener = label.to_string();
    let app_for_popups = app.clone();
    let mut b = WebviewWindowBuilder::new(app, label, WebviewUrl::External("about:blank".parse().expect("a URL")))
        .title(title)
        .inner_size(1280.0, 860.0)
        .min_inner_size(480.0, 320.0)
        // The window's own frame, title and buttons: the page draws none.
        .decorations(true)
        .on_navigation(super::navigation_allowed)
        .on_page_load(page_loaded)
        .on_new_window(move |url, f| popup(&app_for_popups, &opener, url, f))
        .on_document_title_changed(|w, t| {
            if let Some(e) = entry(w.label()) {
                let name = lock(&e).name.clone();
                let _ = w.set_title(&self::title(&name, Some(&t)));
            }
        });
    if let Some(f) = features {
        b = b.window_features(f);
    }
    if let Some(proxy) = entry(label).and_then(|e| lock(&e).proxy.clone()) {
        b = b.proxy_url(proxy);
    }
    match place {
        #[cfg(not(target_os = "macos"))]
        Some(Place::Dir(dir)) => b = b.data_directory(dir),
        #[cfg(target_os = "macos")]
        Some(Place::Dir(_)) => b = b.incognito(true),
        #[cfg(target_os = "macos")]
        Some(Place::Store(id)) => b = b.data_store_identifier(id),
        Some(Place::Memory) => b = b.incognito(true),
        None => {}
    }
    b.build()
}

/// The page opened a window (`window.open`, a link to `_blank`): another
/// web window of the same session, sharing its storage and trust.
fn popup(app: &AppHandle, opener: &str, url: url::Url, features: NewWindowFeatures) -> NewWindowResponse<tauri::Wry> {
    if !super::navigation_allowed(&url) {
        return NewWindowResponse::Deny;
    }
    let Some(e) = entry(opener) else { return NewWindowResponse::Deny };
    let (name, allowed) = {
        let w = lock(&e);
        (w.name.clone(), w.popups)
    };
    if !allowed {
        tracing::info!("Web session: a page's new window was refused (pop-up windows are off for the session)");
        return NewWindowResponse::Deny;
    }
    let label = {
        let mut reg = registry();
        let label = next_label(&mut reg, app);
        reg.windows.insert(label.clone(), e);
        label
    };
    match build(app, &label, &title(&name, None), None, Some(features)) {
        Ok(w) => {
            platform::install(&w, label);
            NewWindowResponse::Create { window: w }
        }
        Err(err) => {
            tracing::warn!("Web session popup: {err}");
            forget(&label);
            NewWindowResponse::Deny
        }
    }
}

/// A page finished loading: the login script, if it is the session's own
/// origin and there is a login to type.
/// An iDRAC 9's web pages (`/restgui/...`) keep their session's XSRF token
/// in a header; this keeps a copy in the window's session storage, so that
/// closing the window can log out (see [`close_requested`]).
const IDRAC_TOKEN_WATCH: &str = r#"(function () {
  if (window.__reachercmXsrf) return;
  window.__reachercmXsrf = 1;
  function keep(v) { if (v) { try { sessionStorage.setItem('__reachercm_xsrf', v); } catch (e) {} } }
  var X = XMLHttpRequest.prototype, setH = X.setRequestHeader, send = X.send;
  X.setRequestHeader = function (n, v) { if (String(n).toLowerCase() === 'xsrf-token') keep(v); return setH.apply(this, arguments); };
  X.send = function () {
    var x = this;
    x.addEventListener('load', function () { try { keep(x.getResponseHeader('XSRF-TOKEN')); } catch (e) {} });
    return send.apply(this, arguments);
  };
  if (window.fetch) {
    var f = window.fetch;
    window.fetch = function () {
      return f.apply(this, arguments).then(function (r) { try { keep(r.headers.get('XSRF-TOKEN')); } catch (e) {} return r; });
    };
  }
})();"#;

/// The iDRAC 9 web interface's own Log out: frees the session slot.
const IDRAC_LOGOUT: &str = r#"(function () {
  var t = null;
  try { t = sessionStorage.getItem('__reachercm_xsrf'); } catch (e) {}
  var h = {};
  if (t) h['XSRF-TOKEN'] = t;
  try { fetch('/sysmgmt/2015/bmc/session', { method: 'DELETE', headers: h, credentials: 'same-origin', keepalive: true }); } catch (e) {}
})();"#;

/// A web window is asked to close. The last open window of a session on an
/// iDRAC 9's web interface logs out first (the iDRAC allows only a few
/// sessions, and one left open holds a slot until it times out), then
/// closes a moment later. True: the close was taken over.
pub fn close_requested(app: &AppHandle, label: &str) -> bool {
    let Some(w) = app.get_webview_window(label) else { return false };
    let on_idrac = w.url().map(|u| u.path().starts_with("/restgui/")).unwrap_or(false);
    if !on_idrac {
        return false;
    }
    let last = {
        let reg = registry();
        let Some(mine) = reg.windows.get(label) else { return false };
        let id = lock(mine).session_id.clone();
        !reg.windows.iter().any(|(l, e)| l != label && lock(e).session_id == id)
    };
    if !last || w.eval(IDRAC_LOGOUT).is_err() {
        return false;
    }
    tracing::info!("Web session window {label}: logging out of the iDRAC before closing");
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let _ = w.destroy();
    });
    true
}

/// Leaves a one-time console login page that says Not Found for the
/// iDRAC's plain console address (`replace`, so the login is not kept in
/// the window's history either).
const CONSOLE_FALLBACK: &str = r#"(function () {
  var t = ((document.title || '') + ' ' + (document.body ? document.body.innerText.slice(0, 300) : '')).toLowerCase();
  if (t.indexOf('not found') < 0) return;
  document.body.innerHTML = '<div style="position:fixed;inset:0;display:flex;align-items:center;justify-content:center;' +
    'background:#14161c;color:#fff;font:600 16px system-ui,sans-serif">Reacher CM: signing in to open the console\u2026</div>';
  location.replace('/console');
})();"#;

fn page_loaded(w: WebviewWindow, p: PageLoadPayload<'_>) {
    if p.event() != PageLoadEvent::Finished || !matches!(p.url().scheme(), "http" | "https") {
        return;
    }
    // The path only, for a page that fails to open (a console that answers
    // Not Found): the query can hold one-time logins and the host stays out.
    tracing::info!("Web session window {}: loaded {}", w.label(), p.url().path());
    // An iDRAC 9's one-time console login (see `bmc::BmcConn::console`) that
    // the firmware refused (7.00.00.185 answers Not Found): plain `/console`
    // instead, which asks for the sign-in this window fills in.
    if p.url().path() == "/console" && p.url().query_pairs().any(|(k, _)| k == "tempUsername") {
        if let Err(err) = w.eval(CONSOLE_FALLBACK) {
            tracing::warn!("Web session console fallback: {err}");
        }
    }
    if p.url().path().starts_with("/restgui/") {
        if let Err(err) = w.eval(IDRAC_TOKEN_WATCH) {
            tracing::warn!("Web session iDRAC token watch: {err}");
        }
    }
    // An iDRAC 9's console sign-in page: the window comes forward, so that
    // the page runs at full speed and its pop-up console opens on top.
    if p.url().path() == "/restgui/start.html" {
        let _ = w.set_focus();
    }
    let Some(e) = entry(w.label()) else { return };
    let script = {
        let e = lock(&e);
        let (Some(spec), Some(password)) = (e.spec.as_ref(), e.password.as_deref()) else { return };
        if p.url().origin().ascii_serialization() != e.origin {
            return;
        }
        autologin::script(&e.origin, &e.username, password, spec, e.submit)
    };
    if let Err(err) = w.eval(script) {
        tracing::warn!("Web session sign-in: {err}");
    }
}

/// The engine met a certificate the system does not trust; true: accept
/// it (the session trusts it). Otherwise the main window is asked, once.
pub(super) fn certificate_error(label: &str, host: &str, fingerprint: &str) -> bool {
    let Some(e) = entry(label) else { return false };
    let ask = {
        let mut w = lock(&e);
        match certs::decide(&w.trusted, w.had_certs, fingerprint) {
            Decision::Allow => return true,
            Decision::Ask { changed } => {
                if w.pending.as_ref().is_some_and(|(f, l, _)| f == fingerprint && l == label) {
                    return false;
                }
                w.pending = Some((fingerprint.to_string(), label.to_string(), host.to_string()));
                CertAsk {
                    label: label.to_string(),
                    session_id: w.session_id.clone(),
                    session_name: w.name.clone(),
                    host: host.to_string(),
                    fingerprint: fingerprint.to_string(),
                    changed,
                }
            }
        }
    };
    tracing::info!("Web session {}: asking about the certificate of {host}", ask.session_id);
    if let Some(app) = APP.get() {
        let _ = app.emit_to("main", "web-cert-ask", &ask);
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
    }
    false
}

/// The user's answer to `web-cert-ask`. Trusted: the session's new
/// certificate, to keep on it, and the window loads again.
pub fn answer(label: &str, fingerprint: &str, trust: bool) -> Result<Option<(String, WebCert)>, String> {
    let e = entry(label).ok_or("That web window is closed")?;
    let saved = {
        let mut w = lock(&e);
        match w.pending.take() {
            Some((f, _, host)) if f == fingerprint => {
                if !trust {
                    return Ok(None);
                }
                if !w.trusted.contains(&f) {
                    w.trusted.push(f.clone());
                }
                w.had_certs = true;
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                (w.session_id.clone(), WebCert { sha256: f, host, saved_at: Some(now) })
            }
            other => {
                w.pending = other;
                return Err("This certificate is no longer being asked about".into());
            }
        }
    };
    if let Some(win) = APP.get().and_then(|a| a.get_webview_window(label)) {
        platform::retry(&win, label.to_string());
        let _ = win.set_focus();
    }
    Ok(Some(saved))
}

/// Whether the page in `label` may open windows of its own.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn popups_allowed(label: &str) -> bool {
    entry(label).is_some_and(|e| lock(&e).popups)
}

/// The login for an HTTP sign-in (Basic, Digest) from `host`: the session's,
/// once, and only for its own server.
pub(super) fn http_login(label: &str, host: &str) -> Option<(String, String)> {
    let e = entry(label)?;
    let mut w = lock(&e);
    if w.basic_used || w.spec.is_none() || !w.host.eq_ignore_ascii_case(host.trim_start_matches('[').trim_end_matches(']')) {
        return None;
    }
    w.basic_used = true;
    Some((w.username.clone(), w.password.clone()?))
}

fn forget(label: &str) {
    let mut reg = registry();
    reg.windows.remove(label);
    reg.main.retain(|_, l| l != label);
    if let Some(dir) = reg.temporary.remove(label) {
        // The engine may still be letting go of its files.
        std::thread::spawn(move || {
            for _ in 0..10 {
                if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    }
}

/// A web window was closed.
pub fn destroyed(label: &str) {
    platform::forget(label);
    forget(label);
}

/// Whether a session has a web window open (its storage is in use).
pub fn is_open(session_id: &str) -> bool {
    registry().main.contains_key(session_id)
}

/// The main window went: every web window goes too.
pub fn close_all(app: &AppHandle) {
    for (label, w) in app.webview_windows() {
        if super::is_web_label(&label) {
            let _ = w.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_folders_stay_apart() {
        assert_ne!(folder_name("a b"), folder_name("a_b"));
        assert!(folder_name("../../etc").chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(folder_name(&"x".repeat(200)).len() < 60);
    }
}
