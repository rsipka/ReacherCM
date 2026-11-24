//! Floating session windows: more windows of the same app, each showing tabs
//! moved out of another one. A floating window is the app's own page opened
//! with `?float=<label>`, which draws a slim shell (no sidebar) around the
//! terminal area; see src/lib/state/windows.svelte.ts.
//!
//! Connections live here in the backend and outlive the window showing them,
//! so moving a tab only moves what draws it. What a new window should show is
//! kept here until its page has loaded and asks for it (`float_window_take`),
//! as the editor window's first file is.
//!
//! Desktop only: a phone has one window.

#![cfg(desktop)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Labels of floating windows start with this; capabilities match `float-*`.
pub const FLOAT_PREFIX: &str = "float-";

/// What each new window shows first, by label, until its page takes it.
fn pending() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    static PENDING: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Where a new window goes, in physical pixels. Without one the window is
/// centred at its default size.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct FloatRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub label: String,
    pub title: String,
}

/// The first free label: float-2, float-3… (the main window is the first).
fn next_label(app: &AppHandle) -> String {
    let taken = app.webview_windows();
    let reserved = pending().lock().map(|p| p.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    (2..)
        .map(|n| format!("{FLOAT_PREFIX}{n}"))
        .find(|l| !taken.contains_key(l) && !reserved.contains(l))
        .expect("an unused label")
}

/// Open a floating window that will show `payload` (the tabs moving into it).
#[tauri::command]
pub async fn float_window_open(
    app: AppHandle,
    payload: serde_json::Value,
    rect: Option<FloatRect>,
    title: Option<String>,
) -> Result<String, String> {
    let label = next_label(&app);
    pending()
        .lock()
        .map_err(|e| e.to_string())?
        .insert(label.clone(), payload);

    let url = WebviewUrl::App(format!("/?float={label}").into());
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(title.unwrap_or_else(|| "Reacher CM".into()))
        .inner_size(1000.0, 680.0)
        .min_inner_size(480.0, 320.0)
        .background_color(tauri::window::Color(10, 10, 10, 255))
        // Placed and sized while hidden, so it never flashes at the wrong spot.
        .visible(false);
    // As the main window (tauri.macos.conf.json): the Mac's own traffic
    // lights over the app's title bar; elsewhere the app draws its own.
    // y = 22 centres the buttons on the 38px title bar: tao sizes the button
    // strip to button height (16) + y and centres them in it.
    #[cfg(target_os = "macos")]
    let builder = builder
        .decorations(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 22.0));
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);

    let win = match builder.build() {
        Ok(w) => w,
        Err(e) => {
            if let Ok(mut p) = pending().lock() {
                p.remove(&label);
            }
            return Err(e.to_string());
        }
    };
    match rect {
        Some(r) => {
            // Wayland ignores a position (the compositor places windows);
            // the size still applies.
            let _ = win.set_size(tauri::PhysicalSize::new(r.width, r.height));
            let _ = win.set_position(tauri::PhysicalPosition::new(r.x, r.y));
            let _ = win.show();
            if r.maximized {
                let _ = win.maximize();
            }
        }
        None => {
            let _ = win.center();
            let _ = win.show();
        }
    }
    let _ = win.set_focus();
    Ok(label)
}

/// What this floating window was opened to show; asked once by its page.
#[tauri::command]
pub async fn float_window_take(window: tauri::WebviewWindow) -> Result<Option<serde_json::Value>, String> {
    Ok(pending().lock().map_err(|e| e.to_string())?.remove(window.label()))
}

/// The main window and every floating one, for "Move to window".
#[tauri::command]
pub async fn float_window_list(app: AppHandle) -> Result<Vec<WindowInfo>, String> {
    let mut out: Vec<WindowInfo> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label == "main" || label.starts_with(FLOAT_PREFIX))
        .map(|(label, w)| WindowInfo { title: w.title().unwrap_or_default(), label })
        .collect();
    // Main first, then by number, so the menu keeps its order.
    out.sort_by_key(|w| (w.label != "main", w.label.trim_start_matches(FLOAT_PREFIX).parse::<u32>().unwrap_or(0)));
    Ok(out)
}

/// Bring a window to the front (from a floating window: the main one, to unlock it).
#[tauri::command]
pub async fn float_window_focus(app: AppHandle, label: String) -> Result<(), String> {
    if label != "main" && !label.starts_with(FLOAT_PREFIX) {
        return Err("not a session window".into());
    }
    let win = app.get_webview_window(&label).ok_or("no such window")?;
    let _ = win.show();
    let _ = win.unminimize();
    win.set_focus().map_err(|e| e.to_string())
}

/// Labels of RemoteApp windows: `rail-<tab>-<remote window id>`.
pub const RAIL_PREFIX: &str = "rail-";

/// One RemoteApp window as it should be here: where (physical pixels on this
/// screen), how big, what it is called.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RailPlace {
    pub id: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// A menu, tooltip or other window that is part of another: kept above
    /// the others, off the taskbar, and never takes focus by itself.
    pub popup: bool,
}

/// Make this computer's RemoteApp windows for the tab `tab` match `windows`:
/// open the missing ones, move and resize the rest, close those gone.
/// Each shows `/?railview=<tab>&win=<id>`, its part of the session's picture.
#[tauri::command]
pub async fn rail_windows_sync(app: AppHandle, tab: String, windows: Vec<RailPlace>) -> Result<(), String> {
    if tab.is_empty() || !tab.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("bad tab id".into());
    }
    let prefix = format!("{RAIL_PREFIX}{tab}-");
    let wanted: HashMap<String, &RailPlace> = windows.iter().map(|w| (format!("{prefix}{}", w.id), w)).collect();
    for (label, win) in app.webview_windows() {
        if label.starts_with(&prefix) && !wanted.contains_key(&label) {
            let _ = win.destroy();
        }
    }
    for (label, place) in wanted {
        let (size, at) = (
            tauri::PhysicalSize::new(place.width.max(1), place.height.max(1)),
            tauri::PhysicalPosition::new(place.x, place.y),
        );
        if let Some(win) = app.get_webview_window(&label) {
            if win.outer_size().ok() != Some(size) {
                let _ = win.set_size(size);
            }
            if win.outer_position().ok() != Some(at) {
                let _ = win.set_position(at);
            }
            if win.title().ok().as_deref() != Some(place.title.as_str()) {
                let _ = win.set_title(&place.title);
            }
            continue;
        }
        let url = WebviewUrl::App(format!("/?railview={tab}&win={}", place.id).into());
        let win = WebviewWindowBuilder::new(&app, &label, url)
            .title(if place.title.is_empty() { "RemoteApp" } else { &place.title })
            .decorations(false)
            .resizable(false)
            .shadow(false)
            .skip_taskbar(place.popup)
            .always_on_top(place.popup)
            .focused(!place.popup)
            .background_color(tauri::window::Color(0, 0, 0, 255))
            .visible(false)
            .build()
            .map_err(|e| e.to_string())?;
        // Wayland ignores a position (the compositor places windows).
        let _ = win.set_size(size);
        let _ = win.set_position(at);
        let _ = win.show();
    }
    Ok(())
}

/// The main window went: its floating windows go with it, as the app ends.
pub fn close_floating(app: &AppHandle) {
    for (label, win) in app.webview_windows() {
        if label.starts_with(FLOAT_PREFIX) || label.starts_with(RAIL_PREFIX) {
            let _ = win.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_reads_without_maximized() {
        let r: FloatRect = serde_json::from_str(r#"{"x":-1920,"y":40,"width":1200,"height":800}"#).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height, r.maximized), (-1920, 40, 1200, 800, false));
    }
}
