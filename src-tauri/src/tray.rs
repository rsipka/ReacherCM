//! The system tray icon: its tooltip and its menu.
//!
//! The interface knows what the menu should hold (open tabs, favorites, the
//! sessions used lately, whether the vault is open, the language), so it sends
//! that over with `tray_update` whenever it changes, and the menu is rebuilt
//! from it here. Clicks on items that need the interface go back to the main
//! window as a `tray-action` event; showing, hiding and quitting are done here.
//!
//! Until the interface has reported, the menu is the plain Show/Quit pair.
//!
//! Linux tray hosts (AppIndicator) show no tooltip, so the first menu line
//! carries the same status there and everywhere else.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Deserialize;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::APP_NAME;

/// The tray icon's id, for `app.tray_by_id`.
pub const TRAY_ID: &str = "main";

/// How many sessions a Favorites or Recent submenu lists at most.
const MAX_SESSIONS: usize = 15;
/// Longer session names are cut, so the menu stays a sensible width.
const MAX_NAME_CHARS: usize = 48;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TraySession {
    pub id: String,
    pub name: String,
}

/// The menu's wording, in the interface's language.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrayLabels {
    pub show: String,
    pub hide: String,
    pub favorites: String,
    pub recent: String,
    pub local_terminal: String,
    pub quick_connect: String,
    /// "Mute sounds" or "Unmute sounds", whichever applies now.
    pub mute: String,
    pub lock: String,
    pub settings: String,
    pub quit: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrayState {
    /// One line on what is open, such as "3 tabs open, 2 connected".
    pub status: String,
    /// The vault is locked: nothing to connect to, nothing to lock.
    pub locked: bool,
    /// A vault exists and can be locked (with no identity there is none).
    pub can_lock: bool,
    /// A local shell can be opened here.
    pub local_terminal: bool,
    pub favorites: Vec<TraySession>,
    pub recent: Vec<TraySession>,
    pub labels: TrayLabels,
}

static STATE: Mutex<Option<TrayState>> = Mutex::new(None);
/// Whether the main window was visible when the menu was last built, so the
/// Show/Hide item is rebuilt only when that changes.
static BUILT_VISIBLE: AtomicBool = AtomicBool::new(true);

#[derive(Debug, PartialEq)]
enum Entry {
    Item { id: String, text: String, enabled: bool },
    Submenu { id: String, text: String, items: Vec<(String, String)> },
    Separator,
}

fn item(id: &str, text: &str) -> Entry {
    Entry::Item { id: id.into(), text: menu_text(text), enabled: true }
}

/// A label as menus take it: `&` marks a mnemonic, so a literal one is
/// doubled; a long name is cut with an ellipsis.
fn menu_text(text: &str) -> String {
    let text: String = if text.chars().count() > MAX_NAME_CHARS {
        let mut cut: String = text.chars().take(MAX_NAME_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        text.to_string()
    };
    text.replace('&', "&&")
}

/// The tooltip: the app's name, then the status line.
fn tooltip(state: Option<&TrayState>) -> String {
    match state {
        Some(s) if !s.status.is_empty() => format!("{APP_NAME}\n{}", s.status),
        _ => APP_NAME.to_string(),
    }
}

fn sessions_submenu(id: &str, text: &str, sessions: &[TraySession]) -> Option<Entry> {
    if sessions.is_empty() {
        return None;
    }
    let items = sessions
        .iter()
        .take(MAX_SESSIONS)
        .map(|s| (format!("tray:connect:{id}:{}", s.id), menu_text(&s.name)))
        .collect();
    Some(Entry::Submenu { id: format!("tray:{id}"), text: menu_text(text), items })
}

/// What the menu holds, apart from building it (so it can be tested).
fn plan(state: Option<&TrayState>, visible: bool) -> Vec<Entry> {
    let Some(s) = state else {
        return vec![item("show", "Show"), Entry::Separator, item("quit", "Quit")];
    };
    let l = &s.labels;
    let mut out = Vec::new();
    let status = if s.status.is_empty() { APP_NAME.to_string() } else { format!("{APP_NAME} · {}", s.status) };
    out.push(Entry::Item { id: "tray:status".into(), text: menu_text(&status), enabled: false });
    out.push(item("tray:toggle", if visible { &l.hide } else { &l.show }));
    out.push(item("tray:mute", &l.mute));

    if !s.locked {
        let mut connect = Vec::new();
        connect.extend(sessions_submenu("favorites", &l.favorites, &s.favorites));
        connect.extend(sessions_submenu("recent", &l.recent, &s.recent));
        if s.local_terminal {
            connect.push(item("tray:local", &l.local_terminal));
        }
        connect.push(item("tray:quick-connect", &l.quick_connect));
        out.push(Entry::Separator);
        out.extend(connect);

        out.push(Entry::Separator);
        if s.can_lock {
            out.push(item("tray:lock", &l.lock));
        }
        out.push(item("tray:settings", &l.settings));
    }

    out.push(Entry::Separator);
    out.push(item("quit", &l.quit));
    out
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, entries: &[Entry]) -> tauri::Result<Menu<R>> {
    let mut menu = MenuBuilder::new(app);
    for entry in entries {
        menu = match entry {
            Entry::Item { id, text, enabled } => {
                menu.item(&MenuItemBuilder::with_id(id.as_str(), text).enabled(*enabled).build(app)?)
            }
            Entry::Separator => menu.separator(),
            Entry::Submenu { id, text, items } => {
                let mut sub = SubmenuBuilder::with_id(app, id.as_str(), text);
                for (item_id, item_text) in items {
                    sub = sub.item(&MenuItemBuilder::with_id(item_id.as_str(), item_text).build(app)?);
                }
                menu.item(&sub.build()?)
            }
        };
    }
    menu.build()
}

fn main_visible<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(true)
}

/// Rebuild the menu and the tooltip from the last state the interface sent.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let visible = main_visible(app);
    BUILT_VISIBLE.store(visible, Ordering::Relaxed);
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match build_menu(app, &plan(state.as_ref(), visible)) {
        Ok(menu) => {
            if let Err(e) = tray.set_menu(Some(menu)) {
                tracing::warn!("Could not set the tray menu: {e}");
            }
        }
        Err(e) => tracing::warn!("Could not build the tray menu: {e}"),
    }
    let _ = tray.set_tooltip(Some(tooltip(state.as_ref())));
}

/// Rebuild only if the main window was shown or hidden since the last build,
/// so Show/Hide says the right thing. Cheap enough for every focus change.
pub fn sync_visibility<R: Runtime>(app: &AppHandle<R>) {
    if main_visible(app) != BUILT_VISIBLE.load(Ordering::Relaxed) {
        refresh(app);
    }
}

/// The interface's view of what the tray should show.
#[tauri::command]
pub fn tray_update(app: AppHandle, state: TrayState) {
    {
        let mut current = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref() == Some(&state) && main_visible(&app) == BUILT_VISIBLE.load(Ordering::Relaxed) {
            return;
        }
        *current = Some(state);
    }
    refresh(&app);
}

/// The interface hid or showed the window itself (close to tray).
#[tauri::command]
pub fn tray_sync(app: AppHandle) {
    sync_visibility(&app);
}

/// Bring the main window up, in front.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    sync_visibility(app);
}

fn quit<R: Runtime>(app: &AppHandle<R>) {
    // Route through the frontend so Quit warns about active SSH sessions too
    // (same guard as the window close). Fall back to a hard exit if the
    // window or the frontend can't be reached.
    match app.get_webview_window("main") {
        Some(window) => {
            show_main(app);
            if window.emit("app-quit-requested", ()).is_err() {
                app.exit(0);
            }
        }
        None => app.exit(0),
    }
}

/// What a tray item asks the interface to do, as the `tray-action` payload.
fn action_for(id: &str) -> Option<serde_json::Value> {
    if let Some(rest) = id.strip_prefix("tray:connect:") {
        // "favorites:<id>" or "recent:<id>": the list it came from only
        // keeps the ids apart when a session is in both.
        let session = rest.split_once(':').map(|(_, s)| s).unwrap_or(rest);
        return Some(serde_json::json!({ "action": "connect", "sessionId": session }));
    }
    let action = match id {
        "tray:local" => "local-terminal",
        "tray:quick-connect" => "quick-connect",
        "tray:lock" => "lock",
        "tray:mute" => "mute",
        "tray:settings" => "settings",
        _ => return None,
    };
    Some(serde_json::json!({ "action": action }))
}

/// A click on a menu item. Every menu's events come here (the app's own menu
/// bar too), so anything that is not the tray's is left alone.
pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "show" => show_main(app),
        "quit" => quit(app),
        "tray:toggle" => {
            let window = app.get_webview_window("main");
            if main_visible(app) {
                if let Some(w) = window {
                    let _ = w.hide();
                }
                sync_visibility(app);
            } else {
                show_main(app);
            }
        }
        // Muting changes nothing on screen, so the window stays where it is.
        "tray:mute" => {
            if let (Some(w), Some(payload)) = (app.get_webview_window("main"), action_for(id)) {
                let _ = w.emit("tray-action", payload);
            }
        }
        _ => {
            if let Some(payload) = action_for(id) {
                // Each of these opens or changes something in the window,
                // so it comes to the front first.
                show_main(app);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("tray-action", payload);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> TrayLabels {
        TrayLabels {
            show: "Show window".into(),
            hide: "Hide window".into(),
            favorites: "Favorites".into(),
            recent: "Recent".into(),
            local_terminal: "New local terminal".into(),
            quick_connect: "Quick connect…".into(),
            mute: "Mute sounds".into(),
            lock: "Lock".into(),
            settings: "Settings…".into(),
            quit: "Quit".into(),
        }
    }

    fn state() -> TrayState {
        TrayState {
            status: "2 tabs open, 1 connected".into(),
            locked: false,
            can_lock: true,
            local_terminal: true,
            favorites: vec![TraySession { id: "a".into(), name: "Router & switch".into() }],
            recent: vec![],
            labels: labels(),
        }
    }

    fn ids(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item { id, .. } | Entry::Submenu { id, .. } => id.clone(),
                Entry::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn plain_menu_before_the_interface_reports() {
        assert_eq!(ids(&plan(None, true)), ["show", "-", "quit"]);
        assert_eq!(tooltip(None), APP_NAME);
    }

    #[test]
    fn full_menu_when_unlocked() {
        let s = state();
        assert_eq!(
            ids(&plan(Some(&s), true)),
            [
                "tray:status",
                "tray:toggle",
                "tray:mute",
                "-",
                "tray:favorites",
                "tray:local",
                "tray:quick-connect",
                "-",
                "tray:lock",
                "tray:settings",
                "-",
                "quit"
            ]
        );
        assert_eq!(tooltip(Some(&s)), format!("{APP_NAME}\n2 tabs open, 1 connected"));
    }

    #[test]
    fn locked_menu_offers_only_show_and_quit() {
        let mut s = state();
        s.locked = true;
        assert_eq!(ids(&plan(Some(&s), false)), ["tray:status", "tray:toggle", "tray:mute", "-", "quit"]);
    }

    #[test]
    fn toggle_follows_visibility() {
        let s = state();
        let text = |visible| match &plan(Some(&s), visible)[1] {
            Entry::Item { text, .. } => text.clone(),
            _ => unreachable!(),
        };
        assert_eq!(text(true), "Hide window");
        assert_eq!(text(false), "Show window");
    }

    #[test]
    fn status_line_is_disabled() {
        let s = state();
        assert!(matches!(&plan(Some(&s), true)[0], Entry::Item { enabled: false, .. }));
    }

    #[test]
    fn names_are_escaped_and_cut() {
        assert_eq!(menu_text("Router & switch"), "Router && switch");
        let long = "x".repeat(80);
        let cut = menu_text(&long);
        assert_eq!(cut.chars().count(), MAX_NAME_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn session_items_keep_their_list_apart() {
        let entry = sessions_submenu("recent", "Recent", &[TraySession { id: "x1".into(), name: "x".into() }]).unwrap();
        let Entry::Submenu { items, .. } = entry else { unreachable!() };
        assert_eq!(items[0].0, "tray:connect:recent:x1");
        assert_eq!(
            action_for(&items[0].0),
            Some(serde_json::json!({ "action": "connect", "sessionId": "x1" }))
        );
    }

    #[test]
    fn list_is_capped() {
        let many: Vec<_> = (0..40).map(|i| TraySession { id: i.to_string(), name: i.to_string() }).collect();
        let Some(Entry::Submenu { items, .. }) = sessions_submenu("favorites", "Favorites", &many) else { unreachable!() };
        assert_eq!(items.len(), MAX_SESSIONS);
    }

    #[test]
    fn other_menus_events_are_ignored() {
        assert_eq!(action_for("copy"), None);
        assert_eq!(action_for("paste"), None);
        assert_eq!(action_for("tray:status"), None);
    }
}
