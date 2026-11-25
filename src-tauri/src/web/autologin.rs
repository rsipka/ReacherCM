//! Filling a device's login form from the backend.
//!
//! When a page of the session's own origin (scheme, host and port) has
//! finished loading, a script is run in it that waits for a visible password
//! field, types the user name and password into it as a person would (so
//! React, Angular and ExtJS forms notice), and presses the login button once.
//!
//! The script is the only place the password goes, and it checks the
//! page's origin itself before touching it: `location.origin` cannot be
//! faked by the page, so a page that moved elsewhere between the load and
//! the script gets nothing. A login that fails does not loop: the script
//! presses the button only once per tab (a flag in the page's
//! `sessionStorage`), and fills only after that; the flag is cleared when a
//! page loads with no login form, that is once logged in.
//!
//! Presets are lists of selectors tried in order, then the generic search
//! (the first visible password field, the text field before it, the form's
//! submit button or a button reading "Log in"). They are best effort: device
//! firmware changes its pages, and `custom` lets the user name the fields.

/// Field selectors for one kind of login page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoginSpec {
    pub username: Vec<String>,
    pub password: Vec<String>,
    pub submit: Vec<String>,
}

fn list(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The device presets, by the name stored in `WebOptions::login`.
pub fn preset(name: &str) -> Option<LoginSpec> {
    Some(match name {
        // iDRAC 7/8 (login.html) and 9 (the Angular start page).
        "idrac" => LoginSpec {
            username: list(&["#user", "input[name=\"username\"]", "#username"]),
            password: list(&["#password", "input[name=\"password\"]"]),
            submit: list(&["#btnOK", "button[type=\"submit\"]"]),
        },
        // iLO 4 (#usernameInput…) and iLO 5/6.
        "ilo" => LoginSpec {
            username: list(&["#usernameInput", "#username", "input[name=\"username\"]"]),
            password: list(&["#passwordInput", "#password", "input[name=\"password\"]"]),
            submit: list(&["#ID_LOGON", "#login-form__submit", "button[type=\"submit\"]"]),
        },
        // Proxmox VE, Backup Server and Mail Gateway (ExtJS): the login
        // button is no <button>, so it is found by its text or Enter is
        // pressed in the password field.
        "proxmox" => LoginSpec {
            username: list(&["input[name=\"username\"]"]),
            password: list(&["input[name=\"password\"]"]),
            submit: Vec::new(),
        },
        // UniFi Network / UniFi OS.
        "unifi" => LoginSpec {
            username: list(&["#login-username", "input[name=\"username\"]"]),
            password: list(&["#login-password", "input[name=\"password\"]"]),
            submit: list(&["button[type=\"submit\"]"]),
        },
        _ => return None,
    })
}

pub fn is_preset(name: &str) -> bool {
    preset(name).is_some()
}

/// The selectors a session's `login` setting stands for: a preset, every
/// preset (`auto`), the user's own (`custom`) or none (`generic`, which uses
/// only the search every spec falls back to). `None`: do not fill (`off`).
pub fn spec_for(mode: &str, custom: [Option<&str>; 3]) -> Option<LoginSpec> {
    match mode {
        "off" => None,
        "generic" => Some(LoginSpec::default()),
        "custom" => {
            let one = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(|s| vec![s.to_string()]).unwrap_or_default();
            Some(LoginSpec { username: one(custom[0]), password: one(custom[1]), submit: one(custom[2]) })
        }
        "auto" => {
            let mut all = LoginSpec::default();
            for p in ["idrac", "ilo", "unifi", "proxmox"] {
                let s = preset(p).expect("a preset");
                for (into, from) in [(&mut all.username, s.username), (&mut all.password, s.password), (&mut all.submit, s.submit)] {
                    for sel in from {
                        if !into.contains(&sel) {
                            into.push(sel);
                        }
                    }
                }
            }
            Some(all)
        }
        other => preset(other),
    }
}

/// A Rust string as a JavaScript string literal. JSON is one, except that
/// U+2028/U+2029 must not appear raw in older engines, and `</script>` does
/// not matter here (the script is evaluated, not put in HTML).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).expect("a string serialises").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

fn js_list(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| js_str(s)).collect::<Vec<_>>().join(","))
}

/// The script that fills (and, if `submit`, sends) the login form, for a
/// page whose origin must be `origin` (as `Url::origin().ascii_serialization()`).
pub fn script(origin: &str, username: &str, password: &str, spec: &LoginSpec, submit: bool) -> String {
    format!(
        r#"(function () {{
  if (location.origin !== {origin}) return;
  var SPEC = {{ u: {su}, p: {sp}, s: {ss} }}, SUBMIT = {submit};
  var KEY = '__reachercm_login_sent';
  var store = null;
  try {{ store = window.sessionStorage; }} catch (e) {{}}
  var sent = false;
  try {{ sent = !!store && store.getItem(KEY) === '1'; }} catch (e) {{}}
  var setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  function usable(el) {{
    if (!el || el.disabled || el.readOnly) return false;
    var r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    var cs = getComputedStyle(el);
    return cs.visibility !== 'hidden' && cs.display !== 'none';
  }}
  function first(sels) {{
    for (var i = 0; i < sels.length; i++) {{
      var l;
      try {{ l = document.querySelectorAll(sels[i]); }} catch (e) {{ continue; }}
      for (var j = 0; j < l.length; j++) if (usable(l[j])) return l[j];
    }}
    return null;
  }}
  function userFieldBefore(pw) {{
    var all = (pw.form || document).querySelectorAll('input'), found = null;
    for (var i = 0; i < all.length; i++) {{
      var el = all[i];
      if (el === pw) break;
      var t = (el.getAttribute('type') || 'text').toLowerCase();
      if ((t === 'text' || t === 'email') && usable(el)) found = el;
    }}
    return found;
  }}
  function type(el, v) {{
    // An unfocused window may send no focus events of its own: a form that
    // waits for them (Angular's touched state) gets them here.
    el.focus();
    el.dispatchEvent(new FocusEvent('focus'));
    el.dispatchEvent(new FocusEvent('focusin', {{ bubbles: true }}));
    setValue.call(el, v);
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
    el.dispatchEvent(new FocusEvent('blur'));
    el.dispatchEvent(new FocusEvent('focusout', {{ bubbles: true }}));
  }}
  function pressEnter(el) {{
    el.focus();
    ['keydown', 'keypress', 'keyup'].forEach(function (t) {{
      el.dispatchEvent(new KeyboardEvent(t, {{ key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true }}));
    }});
  }}
  var WORDS = ['log in', 'login', 'log on', 'logon', 'sign in', 'submit', 'přihlásit', 'přihlásit se', 'anmelden'];
  function buttonByText() {{
    var l = document.querySelectorAll('button, a, input[type="button"], input[type="submit"], [role="button"], .x-btn');
    for (var i = 0; i < l.length; i++) {{
      var el = l[i], t = ((el.value || el.textContent || '') + '').trim().toLowerCase();
      if (WORDS.indexOf(t) >= 0 && usable(el)) return el;
    }}
    return null;
  }}
  function send(pw) {{
    var b = first(SPEC.s);
    if (b) {{ b.click(); return; }}
    var f = pw.form;
    if (f) {{
      var fb = f.querySelector('button[type="submit"], input[type="submit"], button:not([type])');
      if (fb && usable(fb)) {{ fb.click(); return; }}
    }}
    b = buttonByText();
    if (b) {{ b.click(); return; }}
    if (f && f.requestSubmit) {{ f.requestSubmit(); return; }}
    pressEnter(pw);
  }}
  // An iDRAC 9's console sign-in page (`/restgui/start.html?console`) takes
  // a few seconds to draw its form: say that the app is on it.
  var veil = null;
  if (!sent && /\/restgui\/start\.html$/.test(location.pathname) && /console/.test(location.search)) {{
    veil = document.createElement('div');
    veil.textContent = 'Reacher CM: signing in to open the console\u2026';
    veil.setAttribute('style', 'position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center;' +
      'background:rgba(20,22,28,.82);color:#fff;font:600 16px system-ui,sans-serif;pointer-events:none');
    (document.body || document.documentElement).appendChild(veil);
    setTimeout(function () {{ if (veil) veil.remove(); }}, 35000);
  }}
  // The form can come late (an iDRAC's start page draws it after a few
  // seconds) and a window that is not focused may hold its timers back, so
  // the page's own changes are watched as well as the clock.
  var started = Date.now(), filling = false, obs = null;
  function fill(pw) {{
    filling = true;
    if (obs) obs.disconnect();
    // A moment for the page to finish wiring the form up.
    setTimeout(function () {{
      if (location.origin !== {origin}) return;
      var user = first(SPEC.u) || userFieldBefore(pw);
      var u = {username};
      if (user && u) type(user, u);
      type(pw, {password});
      if (SUBMIT && !sent) {{
        try {{ if (store) store.setItem(KEY, '1'); }} catch (e) {{}}
        setTimeout(function () {{ send(pw); }}, 300);
      }}
      // Once sent, the page goes on; if it stays (a wrong password), the
      // form is left in view.
      setTimeout(function () {{ if (veil) veil.remove(); }}, 3000);
    }}, 400);
  }}
  // Some pages keep the password field read-only until it is clicked (to
  // stop browsers filling it): click such a field once, then look again.
  var poked = [];
  function pokeReadOnly() {{
    var l = document.querySelectorAll('input[type="password"]');
    for (var i = 0; i < l.length; i++) {{
      var el = l[i];
      if (!el.readOnly || el.disabled || poked.indexOf(el) >= 0) continue;
      var r = el.getBoundingClientRect(), cs = getComputedStyle(el);
      if (r.width <= 0 || r.height <= 0 || cs.visibility === 'hidden' || cs.display === 'none') continue;
      poked.push(el);
      try {{ el.focus(); el.click(); }} catch (e) {{}}
    }}
  }}
  function look() {{
    if (filling) return true;
    var pw = first(SPEC.p.concat(['input[type="password"]']));
    if (pw) {{ fill(pw); return true; }}
    pokeReadOnly();
    return false;
  }}
  try {{
    obs = new MutationObserver(function () {{ look(); }});
    obs.observe(document.documentElement, {{ childList: true, subtree: true, attributes: true }});
  }} catch (e) {{}}
  (function wait() {{
    if (look()) return;
    if (Date.now() - started < 30000) {{ setTimeout(wait, 250); return; }}
    if (obs) obs.disconnect();
    if (veil) veil.remove();
    try {{ if (store) store.removeItem(KEY); }} catch (e) {{}}
  }})();
}})();"#,
        origin = js_str(origin),
        su = js_list(&spec.username),
        sp = js_list(&spec.password),
        ss = js_list(&spec.submit),
        submit = if submit { "true" } else { "false" },
        username = js_str(username),
        password = js_str(password),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_names_a_password_field() {
        for p in ["idrac", "ilo", "proxmox", "unifi"] {
            assert!(!preset(p).unwrap().password.is_empty(), "{p}");
        }
        assert!(preset("auto").is_none());
    }

    #[test]
    fn auto_tries_every_preset_once() {
        let all = spec_for("auto", [None, None, None]).unwrap();
        assert!(all.username.contains(&"#user".to_string()));
        assert!(all.username.contains(&"#usernameInput".to_string()));
        let named = all.username.iter().filter(|s| *s == "input[name=\"username\"]").count();
        assert_eq!(named, 1);
    }

    #[test]
    fn custom_uses_only_what_was_given() {
        let s = spec_for("custom", [Some(" #login "), Some(""), None]).unwrap();
        assert_eq!(s.username, vec!["#login".to_string()]);
        assert!(s.password.is_empty() && s.submit.is_empty());
        assert!(spec_for("off", [None, None, None]).is_none());
        assert_eq!(spec_for("generic", [None, None, None]), Some(LoginSpec::default()));
    }

    #[test]
    fn script_quotes_what_it_types() {
        let s = script("https://10.0.0.5", "ro\"ot", "pa'ss\"</script>\u{2028}\\", &LoginSpec::default(), true);
        assert!(s.contains(r#"location.origin !== "https://10.0.0.5""#));
        assert!(s.contains(r#"var u = "ro\"ot";"#));
        assert!(s.contains(r#"type(pw, "pa'ss\"</script>\u2028\\");"#));
        assert!(!s.contains('\u{2028}'));
        assert!(s.contains("SUBMIT = true"));
    }

    #[test]
    fn script_checks_the_origin_before_anything_else() {
        let s = script("https://10.0.0.5", "u", "secret", &LoginSpec::default(), false);
        let check = s.find("location.origin !==").unwrap();
        assert!(check < s.find("secret").unwrap());
        assert!(s.contains("SUBMIT = false"));
    }
}
