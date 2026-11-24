//! The Telnet surface the terminal calls; see [`crate::telnet`].

use crate::state::AppState;
use crate::telnet::TelnetConnectParams;

/// Connect, then stream output as `telnet-data-{id}` events until
/// `telnet-exit-{id}`. Returns once the connection is up, or why it is not.
#[tauri::command]
pub async fn telnet_connect(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    params: TelnetConnectParams,
    session_id: Option<String>,
) -> Result<(), String> {
    let mut params = params;
    // A password held by a password manager, fetched now. The entry is the
    // stored session's, approved by the connecting person for this device
    // (see `session_commands::require_trusted_refs`); the reference sent
    // here only says to use it.
    if params.password_ref.take().is_some() {
        let (stored, _) = crate::ipc::session_commands::stored_session_for_desktop(
            &state,
            session_id.as_deref(),
            crate::state::SessionKind::Telnet,
            &params.host,
            params.port,
            None,
            None,
        )
        .await?;
        let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
        let entry = crate::ipc::session_commands::desktop_ref(&stored)?;
        params.password = Some(
            crate::pwmanager::with_prompt(
                &app_handle,
                reason,
                crate::pwmanager::resolve_to_string(&state, entry),
            )
            .await
            .inspect_err(|e| tracing::warn!("Telnet {}: {e}", params.id))?,
        );
    }
    tracing::info!(
        "Telnet {}: connecting to {}:{}",
        params.id,
        params.host,
        params.port
    );
    // A WireGuard route: only the saved session's own for this address.
    let wireguard = params.wireguard.clone().filter(|w| !w.is_empty());
    if let Some(wg) = &wireguard {
        let sid = crate::ipc::session_commands::require_session_id(session_id.as_deref())?;
        let stored = crate::ipc::session_commands::stored_session(&state, sid).await?;
        if crate::ipc::ssh_commands::wireguard_route(&stored, &params.host, params.port) != Some(wg.as_str()) {
            return Err(format!("'{}' does not go through this WireGuard tunnel. Connect again.", stored.name));
        }
    }
    let stream = crate::telnet::open(&params.host, params.port, wireguard.as_deref()).await?;
    state
        .telnet_manager
        .lock()
        .await
        .start(app_handle, params, stream);
    Ok(())
}

#[tauri::command]
pub async fn telnet_send(
    state: tauri::State<'_, AppState>,
    id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    // A ZMODEM transfer holds the line.
    if crate::zmodem::intercept_input(&id, &data) {
        return Ok(());
    }
    crate::history::input(&id, &data);
    state.telnet_manager.lock().await.send(&id, data)
}

#[tauri::command]
pub async fn telnet_resize(
    state: tauri::State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.telnet_manager.lock().await.resize(&id, cols, rows)
}

#[tauri::command]
pub async fn telnet_disconnect(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.telnet_manager.lock().await.close(&id);
    Ok(())
}
