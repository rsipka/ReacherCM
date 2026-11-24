//! The SPICE surface the webview calls. Shaped like the RDP and VNC ones, so
//! the same desktop panel drives it; see [`crate::spice`].

use tauri::ipc::Channel;
use tauri::State;

use crate::db::types::Route;
use crate::spice::vv::VvFile;
use crate::spice::{Input, SpiceConnectParams};
use crate::state::AppState;
use crate::vnc::MouseAction;

/// Open a desktop. Frames arrive on `on_frame`; lifecycle arrives as
/// `spice-status-{id}` events. Through an SSH session, that login happens
/// here, so a failed one is this command's error.
#[tauri::command]
pub async fn spice_connect(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    params: SpiceConnectParams,
    session_id: Option<String>,
    on_frame: Channel,
) -> Result<(), String> {
    // Boxed for the main thread's small stack, as `vnc_connect`.
    Box::pin(async move {
        let id = params.id.clone();
        let running = state.spice_manager.lock().await.running(&id);
        let mut params = params;
        // The password manager entry and the SSH leg are checked and built
        // as for VNC: from the stored session, against the way in the entry
        // was approved for.
        let reason = crate::pwmanager::Reason::Connect(params.host.clone());
        let forward = crate::pwmanager::with_prompt(&app_handle, reason, async {
            let mut pin = None;
            if params.password_ref.is_some() && !running {
                let (stored, via) = crate::ipc::session_commands::stored_session_for_desktop(
                    &state,
                    session_id.as_deref(),
                    crate::state::SessionKind::Spice,
                    &params.host,
                    params.port,
                    None,
                    Some(params.via_session_id.as_deref()),
                )
                .await?;
                crate::pwmanager::name_operation(&stored.name);
                pin = via.as_ref().map(crate::ipc::session_commands::approval_context);
                params.password = crate::pwmanager::resolve_to_string(&state, crate::ipc::session_commands::desktop_ref(&stored)?)
                    .await
                    .inspect_err(|e| tracing::warn!("SPICE {id}: {e}"))?;
            }
            Ok::<_, String>(match params.via_session_id.clone().filter(|s| !s.is_empty() && !running) {
                Some(session_id) => {
                    let route = Route::Session { session_id };
                    let port = params.target_port();
                    crate::ipc::db_commands::forward_route_pinned(&app_handle, &state, &route, params.host.clone(), port, pin.as_deref())
                        .await
                        .inspect_err(|e| tracing::warn!("SPICE {id}: SSH leg failed: {e}"))?
                }
                // Or a WireGuard tunnel the saved session goes through.
                None if !running => crate::ipc::wireguard_commands::session_forward_to(&state, session_id.as_deref(), &params.host, params.port, params.target_port())
                    .await?
                    .map(crate::db::conn::Forward::borrowed),
                None => None,
            })
        })
        .await?;
        state.spice_manager.lock().await.connect(app_handle, params, forward, on_frame);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn spice_disconnect(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.spice_manager.lock().await.disconnect(&id)
}

#[tauri::command]
pub async fn spice_disconnect_all(state: State<'_, AppState>) -> Result<(), String> {
    state.spice_manager.lock().await.disconnect_all();
    Ok(())
}

/// The webview painted one frame message; see [`crate::rdp::Flow`].
#[tauri::command]
pub async fn spice_ack(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.spice_manager.lock().await.ack(&id)
}

/// As `rdp_mouse`: `action` is `move`, `down`, `up` or `wheel`; `button` is
/// 0 left, 1 middle, 2 right; `delta` is positive for a notch away.
#[tauri::command]
pub async fn spice_mouse(
    state: State<'_, AppState>,
    id: String,
    x: u16,
    y: u16,
    action: String,
    button: u8,
    delta: i16,
) -> Result<(), String> {
    let action = MouseAction::parse(&action)?;
    if button > 2 {
        return Err(format!("unknown mouse button {button}"));
    }
    state.spice_manager.lock().await.send(&id, Input::Mouse { x, y, action, button, delta })
}

/// A key by its PC/AT set-1 scancode, as `rdp_key`.
#[tauri::command]
pub async fn spice_key(state: State<'_, AppState>, id: String, scancode: u8, extended: bool, release: bool) -> Result<(), String> {
    state.spice_manager.lock().await.send(&id, Input::Key { scancode, extended, down: !release })
}

/// Text with no key behind it, typed as US-layout keystrokes.
#[tauri::command]
pub async fn spice_type(state: State<'_, AppState>, id: String, text: String) -> Result<(), String> {
    state.spice_manager.lock().await.send(&id, Input::Type(text))
}

#[tauri::command]
pub async fn spice_resize(state: State<'_, AppState>, id: String, width: u16, height: u16) -> Result<(), String> {
    state.spice_manager.lock().await.send(&id, Input::Resize { width, height })
}

/// The desktop gained focus: offer the local clipboard's text to the guest.
#[tauri::command]
pub async fn spice_clipboard_sync(app_handle: tauri::AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let Ok(text) = app_handle.clipboard().read_text() else { return Ok(()) };
    if text.is_empty() {
        return Ok(());
    }
    state.spice_manager.lock().await.offer_clipboard(&id, text)
}

/// Read a virt-viewer file (`.vv`) the user picked. A file that asks to be
/// deleted once read (Proxmox's: its ticket is spent) is, as virt-viewer
/// does, where it is a plain path.
#[tauri::command]
pub async fn spice_read_vv(app_handle: tauri::AppHandle, path: String) -> Result<VvFile, String> {
    let bytes = crate::ipc::vault_commands::picked_file::read(&app_handle, &path).await?;
    if bytes.len() as u64 > crate::spice::vv::MAX_BYTES {
        return Err("This file is too large for a virt-viewer file".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let file = crate::spice::vv::parse(&text)?;
    if file.delete_after && !path.contains("://") && path.to_ascii_lowercase().ends_with(".vv") {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::debug!("SPICE: could not delete the spent .vv file: {e}");
        }
    }
    Ok(file)
}
