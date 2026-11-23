//! WebKitGTK: certificates and HTTP sign-in for web windows.
//!
//! WebKitGTK refuses a certificate it does not trust and reports it with
//! `load-failed-with-tls-errors`, giving the certificate. If the session
//! trusts its fingerprint, it is allowed for that host in the window's own
//! web context (each session has one, from its data folder, so this reaches
//! no other session and not the app) and the page loads again. Otherwise
//! the certificate is kept here until the user answers.

use std::cell::RefCell;
use std::collections::HashMap;

use tauri::WebviewWindow;
use webkit2gtk::gio;
use webkit2gtk::gio::prelude::TlsCertificateExt;
use webkit2gtk::glib::translate::ToGlibPtr;
use webkit2gtk::{AuthenticationRequestExt, AuthenticationScheme, SettingsExt, WebContextExt, WebViewExt};

/// What `retry` needs once the user trusts a certificate: it, its host and
/// the page that failed. GTK objects stay on the main thread.
struct Pending {
    certificate: gio::TlsCertificate,
    host: String,
    uri: String,
}

thread_local! {
    static PENDING: RefCell<HashMap<String, Pending>> = RefCell::new(HashMap::new());
}

pub(super) fn install(win: &WebviewWindow, label: String) {
    let popups = super::window::popups_allowed(&label);
    let result = win.with_webview(move |pv| {
        let view = pv.inner();
        // WebKit blocks `window.open` unless it comes straight from a
        // click. A device's console button usually asks the server for a
        // session first and opens the window after the answer, which no
        // longer counts as a click, so the console never opened. Windows
        // the page opens are this session's own (see `window::popup`).
        if let Some(settings) = WebViewExt::settings(&view) {
            settings.set_javascript_can_open_windows_automatically(popups);
        }
        let l = label.clone();
        view.connect_load_failed_with_tls_errors(move |view, uri, certificate, _errors| {
            let Some(der) = certificate.certificate() else { return false };
            let fp = super::fingerprint(&der);
            let host = url::Url::parse(uri).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
            if super::window::certificate_error(&l, &host, &fp) {
                if let Some(ctx) = view.context() {
                    ctx.allow_tls_certificate_for_host(certificate, &host);
                    view.load_uri(uri);
                    return true;
                }
                return false;
            }
            PENDING.with(|p| {
                p.borrow_mut().insert(l.clone(), Pending { certificate: certificate.clone(), host, uri: uri.to_string() });
            });
            // WebKit's own error page shows meanwhile.
            false
        });
        let l = label;
        view.connect_authenticate(move |_view, request| {
            if !matches!(request.scheme(), AuthenticationScheme::HttpBasic | AuthenticationScheme::HttpDigest | AuthenticationScheme::Default) {
                return false;
            }
            if request.is_retry() {
                return false;
            }
            let host = request.host().map(|h| h.to_string()).unwrap_or_default();
            let Some((user, password)) = super::window::http_login(&l, &host) else { return false };
            let credential = webkit2gtk::Credential::new(&user, &password, webkit2gtk::CredentialPersistence::ForSession);
            // SAFETY: both are live GObjects for the length of the call;
            // WebKit takes its own reference to the credential.
            unsafe {
                webkit2gtk::ffi::webkit_authentication_request_authenticate(
                    request.to_glib_none().0,
                    credential.to_glib_none().0,
                );
            }
            true
        });
    });
    if let Err(e) = result {
        tracing::warn!("Web session: could not watch certificates: {e}");
    }
}

/// The user trusted the certificate `label` is waiting on: allow it and
/// load the page again.
pub(super) fn retry(win: &WebviewWindow, label: String) {
    let _ = win.with_webview(move |pv| {
        let Some(p) = PENDING.with(|p| p.borrow_mut().remove(&label)) else { return };
        let view = pv.inner();
        if let Some(ctx) = view.context() {
            ctx.allow_tls_certificate_for_host(&p.certificate, &p.host);
        }
        view.load_uri(&p.uri);
    });
}

pub(super) fn forget(label: &str) {
    let label = label.to_string();
    let forget = move || {
        PENDING.with(|p| {
            p.borrow_mut().remove(&label);
        })
    };
    // Window events arrive on the main thread, where the map lives.
    forget();
}
