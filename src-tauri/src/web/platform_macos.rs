//! WKWebView: certificates and HTTP sign-in for web windows.
//!
//! WKWebView asks its navigation delegate about every server certificate
//! and HTTP login (`webView:didReceiveAuthenticationChallenge:
//! completionHandler:`). wry's delegate does not answer that, so WebKit
//! applies the system's rules, which refuse a self-signed certificate. This
//! adds the method to wry's delegate class, and re-sets each web window's
//! delegate so WebKit notices it.
//!
//! The method answers only for web windows (it looks the web view up here);
//! for any other web view, the app's own window included, it does what
//! WebKit does without it (`PerformDefaultHandling`). A certificate the
//! system trusts is left to the system too; only one it refuses is checked
//! against the session's fingerprints, and accepted for this one challenge.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, OnceLock};

use block2::Block;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{class, msg_send, sel};
use objc2_foundation::NSString;
use tauri::WebviewWindow;

/// `SecTrustRef`, typed so that message encodings match.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct SecTrust(*mut c_void);

unsafe impl Encode for SecTrust {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("__SecTrust", &[]));
}
unsafe impl RefEncode for SecTrust {
    const ENCODING_REF: Encoding = Encoding::Pointer(&<Self as Encode>::ENCODING);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecTrustEvaluateWithError(trust: *mut c_void, error: *mut *mut c_void) -> bool;
    fn SecTrustGetCertificateAtIndex(trust: *mut c_void, ix: isize) -> *mut c_void;
    fn SecCertificateCopyData(certificate: *mut c_void) -> *mut c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(object: *const c_void);
}

// NSURLSessionAuthChallengeDisposition
const USE_CREDENTIAL: isize = 0;
const PERFORM_DEFAULT_HANDLING: isize = 1;
const CANCEL: isize = 2;
// NSURLCredentialPersistenceForSession
const FOR_SESSION: usize = 1;

/// Web windows' web views (by pointer), to their labels.
fn views() -> MutexGuard<'static, HashMap<usize, String>> {
    static VIEWS: OnceLock<Mutex<HashMap<usize, String>>> = OnceLock::new();
    VIEWS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether this macOS keeps separate data stores by identifier (14+).
pub(super) fn has_data_stores() -> bool {
    use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: 14,
        minorVersion: 0,
        patchVersion: 0,
    })
}

type Completion = Block<dyn Fn(isize, *mut AnyObject)>;

/// The leaf certificate of a trust, as DER.
unsafe fn leaf_der(trust: SecTrust) -> Option<Vec<u8>> {
    unsafe {
        let cert = SecTrustGetCertificateAtIndex(trust.0, 0);
        if cert.is_null() {
            return None;
        }
        let data = SecCertificateCopyData(cert);
        if data.is_null() {
            return None;
        }
        let len = CFDataGetLength(data);
        let ptr = CFDataGetBytePtr(data);
        let der = if ptr.is_null() || len <= 0 { None } else { Some(std::slice::from_raw_parts(ptr, len as usize).to_vec()) };
        CFRelease(data);
        der
    }
}

unsafe extern "C-unwind" fn did_receive_challenge(
    _this: *mut AnyObject,
    _cmd: Sel,
    webview: *mut AnyObject,
    challenge: *mut AnyObject,
    completion: *mut Completion,
) {
    unsafe {
        let Some(completion) = completion.as_ref() else { return };
        let default = || completion.call((PERFORM_DEFAULT_HANDLING, std::ptr::null_mut()));
        let label = views().get(&(webview as usize)).cloned();
        let Some(label) = label else { return default() };
        if challenge.is_null() {
            return default();
        }
        let space: *mut AnyObject = msg_send![challenge, protectionSpace];
        if space.is_null() {
            return default();
        }
        let method: *mut NSString = msg_send![space, authenticationMethod];
        let method = method.as_ref().map(|m| m.to_string()).unwrap_or_default();
        let host: *mut NSString = msg_send![space, host];
        let host = host.as_ref().map(|h| h.to_string()).unwrap_or_default();

        match method.as_str() {
            "NSURLAuthenticationMethodServerTrust" => {
                let trust: SecTrust = msg_send![space, serverTrust];
                if trust.0.is_null() || SecTrustEvaluateWithError(trust.0, std::ptr::null_mut()) {
                    return default();
                }
                let Some(der) = leaf_der(trust) else {
                    return completion.call((CANCEL, std::ptr::null_mut()));
                };
                if super::window::certificate_error(&label, &host, &super::fingerprint(&der)) {
                    let credential: *mut AnyObject = msg_send![class!(NSURLCredential), credentialForTrust: trust];
                    completion.call((USE_CREDENTIAL, credential));
                } else {
                    completion.call((CANCEL, std::ptr::null_mut()));
                }
            }
            "NSURLAuthenticationMethodHTTPBasic" | "NSURLAuthenticationMethodHTTPDigest" | "NSURLAuthenticationMethodDefault" => {
                let failures: isize = msg_send![challenge, previousFailureCount];
                if failures == 0 {
                    if let Some((user, password)) = super::window::http_login(&label, &host) {
                        let user = NSString::from_str(&user);
                        let password = NSString::from_str(&password);
                        let credential: *mut AnyObject = msg_send![
                            class!(NSURLCredential),
                            credentialWithUser: &*user,
                            password: &*password,
                            persistence: FOR_SESSION
                        ];
                        return completion.call((USE_CREDENTIAL, credential));
                    }
                }
                default()
            }
            _ => default(),
        }
    }
}

/// Add the challenge method to `cls` (wry's navigation delegate), once.
unsafe fn add_method(cls: &AnyClass) {
    static ADDED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let mut added = ADDED.lock().unwrap_or_else(|e| e.into_inner());
    let key = cls as *const AnyClass as usize;
    if added.contains(&key) {
        return;
    }
    added.push(key);
    type Method = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut Completion);
    let imp: Method = did_receive_challenge;
    unsafe {
        let ok = objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel!(webView:didReceiveAuthenticationChallenge:completionHandler:),
            std::mem::transmute::<Method, objc2::runtime::Imp>(imp),
            c"v@:@@@?".as_ptr(),
        );
        if !ok.as_bool() {
            tracing::warn!("Web session: the navigation delegate already answers certificate checks; leaving it as is");
        }
    }
}

pub(super) fn install(win: &WebviewWindow, label: String) {
    let popups = super::window::popups_allowed(&label);
    let result = win.with_webview(move |pv| unsafe {
        let view = pv.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        // WebKit blocks `window.open` unless it comes straight from a
        // click; a console button that asks the server first loses the
        // click. Windows the page opens are this session's own.
        let configuration: *mut AnyObject = msg_send![view, configuration];
        if let Some(c) = configuration.as_ref() {
            let preferences: *mut AnyObject = msg_send![c, preferences];
            if let Some(p) = preferences.as_ref() {
                let _: () = msg_send![p, setJavaScriptCanOpenWindowsAutomatically: Bool::new(popups)];
            }
        }
        views().insert(view as usize, label);
        let delegate: *mut AnyObject = msg_send![view, navigationDelegate];
        if let Some(d) = delegate.as_ref() {
            add_method(d.class());
            // WebKit reads which methods the delegate has when it is set.
            let _: () = msg_send![view, setNavigationDelegate: delegate];
        }
    });
    if let Err(e) = result {
        tracing::warn!("Web session: could not watch certificates: {e}");
    }
}

/// The user trusted the certificate: load the page again.
pub(super) fn retry(win: &WebviewWindow, _label: String) {
    let _ = win.with_webview(|pv| unsafe {
        let view = pv.inner() as *mut AnyObject;
        if !view.is_null() {
            let _: *mut AnyObject = msg_send![view, reload];
        }
    });
}

pub(super) fn forget(label: &str) {
    views().retain(|_, l| l != label);
}
