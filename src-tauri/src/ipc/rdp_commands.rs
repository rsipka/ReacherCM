//! The RDP surface the webview calls.
//!
//! Thin on purpose: every command is a lock, a lookup and one call into
//! [`crate::rdp::RdpManager`], which is where the protocol mapping lives.
//! Frames do not pass through here at all — `rdp_connect` hands the manager a
//! channel and the pixels go straight down it for the life of the session.

use tauri::ipc::Channel;

use crate::rdp::{mouse_pdu, RdpConnectParams};
use crate::state::AppState;

/// Open a desktop. Frames arrive on `on_frame` as raw bytes; lifecycle
/// arrives as `rdp-status-{id}` events.
#[tauri::command]
pub async fn rdp_connect(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    params: RdpConnectParams,
    session_id: Option<String>,
    on_frame: Channel,
) -> Result<(), String> {
    // Boxed: Tauri builds a command's future on the main thread and passes it
    // by value before spawning it, and the main thread's stack is small
    // (1 MB on Windows). Only a pointer is left there this way.
    Box::pin(async move {
        let id = params.id.clone();
        let mut params = params;
        // A password held by a password manager, fetched now; not for a panel
        // that only re-attaches to a running desktop. The entry is the stored
        // session's, approved by the connecting person for this server (see
        // `session_commands::require_trusted_refs`); the reference sent here only
        // says to use it.
        if params.password_ref.is_some() && !state.rdp_manager.lock().await.running(&id) {
            let (stored, _) = crate::ipc::session_commands::stored_session_for_desktop(
                &state,
                session_id.as_deref(),
                crate::state::SessionKind::Rdp,
                &params.host,
                params.port,
                Some(&params.username),
                None,
            )
            .await?;
            let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
            let entry = crate::ipc::session_commands::desktop_ref(&stored)?;
            params.password = crate::pwmanager::with_prompt(&app_handle, reason, crate::pwmanager::resolve_to_string(&state, entry))
                .await
                .inspect_err(|e| tracing::warn!("RDP {id}: {e}"))?;
        }
        state
            .rdp_manager
            .lock()
            .await
            .connect(app_handle, params, on_frame)
            .map_err(|e| {
                // The panel shows this; the log keeps it.
                tracing::warn!("RDP {id}: connect rejected: {e}");
                e
            })
    })
    .await
}

#[tauri::command]
pub async fn rdp_disconnect(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.rdp_manager.lock().await.disconnect(&id)
}

/// Every open desktop, on the way out of the app.
#[tauri::command]
pub async fn rdp_disconnect_all(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.rdp_manager.lock().await.disconnect_all();
    Ok(())
}

/// `action` is one of `move`, `down`, `up`, `wheel`; `button` is 0 left,
/// 1 middle, 2 right; `delta` is the wheel step, negative for away.
#[tauri::command]
pub async fn rdp_mouse(
    state: tauri::State<'_, AppState>,
    id: String,
    x: u16,
    y: u16,
    action: String,
    button: u8,
    delta: i16,
) -> Result<(), String> {
    let pdu = mouse_pdu(x, y, &action, button, delta)?;
    state.rdp_manager.lock().await.send_mouse(&id, pdu)
}

/// A PC/AT set-1 scancode, with `extended` for the keys that carry the E0
/// prefix (right Ctrl and Alt, the arrow cluster, the navigation block).
#[tauri::command]
pub async fn rdp_key(
    state: tauri::State<'_, AppState>,
    id: String,
    scancode: u8,
    extended: bool,
    release: bool,
) -> Result<(), String> {
    state.rdp_manager.lock().await.send_key(&id, scancode, extended, release)
}

/// Set the remote's lock keys to the local ones: on connect, on focus, and
/// after a lock key is pressed.
#[tauri::command]
pub async fn rdp_sync_locks(
    state: tauri::State<'_, AppState>,
    id: String,
    caps: bool,
    num: bool,
    scroll: bool,
) -> Result<(), String> {
    state.rdp_manager.lock().await.sync_locks(&id, caps, num, scroll)
}

/// The local NumLock, where the webview cannot tell it: WebKit (Linux and
/// macOS) reports only CapsLock to `getModifierState`. On Linux the keyboard
/// LEDs are readable in sysfs whatever the display server; `None` elsewhere
/// or when no keyboard LED is found.
#[tauri::command]
pub fn rdp_local_numlock() -> Option<bool> {
    local_numlock()
}

#[cfg(target_os = "linux")]
fn local_numlock() -> Option<bool> {
    let mut found = None;
    for entry in std::fs::read_dir("/sys/class/leds").ok()?.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with("::numlock") {
            continue;
        }
        let Ok(value) = std::fs::read_to_string(entry.path().join("brightness")) else {
            continue;
        };
        // Every keyboard has its own LED; any lit one means NumLock is on.
        let lit = value.trim().parse::<u32>().map(|v| v > 0).unwrap_or(false);
        found = Some(found.unwrap_or(false) || lit);
    }
    found
}

#[cfg(not(target_os = "linux"))]
fn local_numlock() -> Option<bool> {
    None
}

/// A character rather than a key, for text with no scancode on this keyboard.
#[tauri::command]
pub async fn rdp_unicode(
    state: tauri::State<'_, AppState>,
    id: String,
    code: u16,
    release: bool,
) -> Result<(), String> {
    state.rdp_manager.lock().await.send_unicode(&id, code, release)
}

/// The desktop gained focus: the local clipboard may have changed since it
/// was last offered to the remote.
#[tauri::command]
pub async fn rdp_clipboard_sync(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.rdp_manager.lock().await.clipboard_sync(&id)
}

/// Files dropped on a desktop: offer them on its clipboard, for a paste
/// there. Only paths the user dropped reach this; each must exist.
#[tauri::command]
pub async fn rdp_clipboard_send_files(
    state: tauri::State<'_, AppState>,
    id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let paths = paths.into_iter().map(std::path::PathBuf::from).collect();
    state.rdp_manager.lock().await.clipboard_send_files(&id, paths)
}

/// Download the files the remote copied that were too large to take unasked.
#[tauri::command]
pub async fn rdp_clipboard_fetch(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.rdp_manager.lock().await.clipboard_fetch(&id)
}

/// The most a pick of files to send may hold: each file passes through
/// memory once on its way into the session's folder (Android's picker gives
/// links, read whole through the ContentResolver).
const MAX_PICK_TOTAL: u64 = 512 * 1024 * 1024;

/// Pick files with the system's picker and put them on the desktop's
/// clipboard, for a paste there. For phones, which have no file clipboard
/// and no drag and drop; works on desktops as well. Returns how many were
/// offered (0: nothing picked).
#[tauri::command]
pub async fn rdp_clipboard_pick_files(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<usize, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(dir) = state.rdp_manager.lock().await.clipboard_staging_dir(&id)? else {
        return Err("the clipboard is off for this desktop".into());
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_files(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx.await.map_err(|e| e.to_string())?.unwrap_or_default();
    if picked.is_empty() {
        let _ = std::fs::remove_dir(&dir);
        return Ok(0);
    }
    let mut staged = Vec::new();
    let mut total = 0u64;
    for file in picked {
        let (name, bytes) = crate::ipc::vault_commands::picked_file::read_named(&app, &picked_path(file)).await?;
        total += bytes.len() as u64;
        if total > MAX_PICK_TOTAL {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("more than {} MB picked; send fewer files at a time", MAX_PICK_TOTAL / (1024 * 1024)));
        }
        let path = dir.join(unique_name(&dir, &safe_name(&name)));
        tokio::fs::write(&path, bytes).await.map_err(|e| e.to_string())?;
        staged.push(path);
    }
    let count = staged.len();
    state.rdp_manager.lock().await.clipboard_send_staged(&id, staged)?;
    Ok(count)
}

/// Save what the last download from the desktop brought, one Save dialog
/// per file. Folders are not saved this way (a phone's Save dialog takes
/// one file); they are counted in `skipped`. For phones, where nothing can
/// be pasted from a file clipboard.
#[tauri::command]
pub async fn rdp_clipboard_save_received(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<SavedFiles, String> {
    use tauri_plugin_dialog::DialogExt;
    let received = state.rdp_manager.lock().await.clipboard_received(&id)?;
    let mut saved = SavedFiles::default();
    for path in received {
        if !path.is_file() {
            saved.skipped += 1;
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.dialog().file().set_file_name(name).save_file(move |picked| {
            let _ = tx.send(picked);
        });
        // Cancelled: the rest are not asked about either.
        let Some(target) = rx.await.map_err(|e| e.to_string())? else { break };
        let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
        crate::ipc::vault_commands::picked_file::write(&app, &picked_path(target), &bytes).await?;
        saved.saved += 1;
    }
    Ok(saved)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct SavedFiles {
    saved: usize,
    skipped: usize,
}

/// What the dialog picked, as the picked_file helpers take it: a path for
/// a file (iOS hands back `file://` links), else the link itself (Android's
/// `content://`).
fn picked_path(file: tauri_plugin_dialog::FilePath) -> String {
    match file.clone().into_path() {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(_) => file.to_string(),
    }
}

/// A picked file's name as a single safe path component.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "file".to_string() } else { cleaned }
}

/// `name`, or `name (2)` and so on, whichever is not yet in `dir`.
fn unique_name(dir: &std::path::Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|n| format!("{stem} ({n}){ext}")).find(|n| !dir.join(n).exists()).unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod clipboard_name_tests {
    use super::*;

    #[test]
    fn a_picked_name_is_one_safe_component() {
        assert_eq!(safe_name("report.pdf"), "report.pdf");
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name("a\\b:c"), "a_b_c");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(safe_name(""), "file");
    }

    #[test]
    fn a_link_or_path_gives_its_last_part_decoded() {
        use crate::ipc::vault_commands::picked_file::name_of;
        assert_eq!(name_of("/home/me/My%20Report.pdf"), "My Report.pdf");
        assert_eq!(name_of("content://com.android.externalstorage.documents/document/primary%3ADownload%2Fx.pdf"), "x.pdf");
        assert_eq!(name_of("C:\\Users\\me\\a.txt"), "a.txt");
        assert_eq!(name_of("content://x/"), "x");
        assert_eq!(name_of("%zz"), "%zz");
        assert_eq!(name_of(""), "file");
    }

    #[test]
    fn a_second_file_of_the_same_name_is_numbered() {
        let dir = std::env::temp_dir().join(format!("reach-pick-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_name(&dir, "a.txt"), "a.txt");
        std::fs::write(dir.join("a.txt"), b"1").unwrap();
        assert_eq!(unique_name(&dir, "a.txt"), "a (2).txt");
        std::fs::write(dir.join("noext"), b"1").unwrap();
        assert_eq!(unique_name(&dir, "noext"), "noext (2)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Stop the clipboard transfer in progress, either way.
#[tauri::command]
pub async fn rdp_clipboard_cancel(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.rdp_manager.lock().await.clipboard_cancel(&id)
}

/// The window itself goes full screen (or comes back) for a desktop. Done
/// here rather than through the web-side window API so it needs no
/// capability grant and cannot be refused quietly.
///
/// A maximized window told to go full screen keeps the work-area size on
/// Windows — the screen less the taskbar — so it is un-maximized first, and
/// maximized again on the way back if that is how it was found.
#[tauri::command]
pub async fn rdp_window_fullscreen(app_handle: tauri::AppHandle, on: bool) -> Result<(), String> {
    // A mobile window has no maximized or full-screen state to speak of;
    // the panel over everything is the whole of full screen there.
    #[cfg(not(desktop))]
    {
        let _ = (app_handle, on);
        Ok(())
    }
    #[cfg(desktop)]
    desktop_fullscreen(app_handle, on)
}

#[cfg(desktop)]
fn desktop_fullscreen(app_handle: tauri::AppHandle, on: bool) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::Manager as _;
    static WAS_MAXIMIZED: AtomicBool = AtomicBool::new(false);

    let window = app_handle.get_webview_window("main").ok_or("no main window")?;
    let err = |e: tauri::Error| e.to_string();
    if on {
        let maximized = window.is_maximized().map_err(err)?;
        WAS_MAXIMIZED.store(maximized, Ordering::Relaxed);
        if maximized {
            window.unmaximize().map_err(err)?;
        }
        window.set_fullscreen(true).map_err(err)
    } else {
        window.set_fullscreen(false).map_err(err)?;
        if WAS_MAXIMIZED.swap(false, Ordering::Relaxed) {
            window.maximize().map_err(err)?;
        }
        Ok(())
    }
}

/// The webview painted one frame message. The pump sends the next only once
/// it hears this, so a busy screen never gets ahead of the canvas.
#[tauri::command]
pub async fn rdp_ack(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.rdp_manager.lock().await.ack(&id)
}

#[tauri::command]
pub async fn rdp_resize(
    state: tauri::State<'_, AppState>,
    id: String,
    width: u16,
    height: u16,
) -> Result<(), String> {
    state.rdp_manager.lock().await.resize(&id, width, height)
}

/// A RemoteApp's windows, for a panel that (re)attaches; later changes come
/// as `rdp-windows-{id}` events.
#[tauri::command]
pub async fn rdp_rail_windows(state: tauri::State<'_, AppState>, id: String) -> Result<Vec<crate::rdp::remote_app::RailWindow>, String> {
    Ok(state.rdp_manager.lock().await.rail_windows(&id))
}

/// `activate`, `close`, `minimize`, `maximize` or `restore` a RemoteApp window.
#[tauri::command]
pub async fn rdp_rail_command(
    state: tauri::State<'_, AppState>,
    id: String,
    window_id: u32,
    command: String,
) -> Result<(), String> {
    state.rdp_manager.lock().await.rail_command(&id, window_id, &command)
}
