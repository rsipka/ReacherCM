//! The local TFTP, FTP, SFTP and HTTP servers (Servers in the sidebar).
//! Desktop only; see `crate::localserver`.

use std::net::IpAddr;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::localserver::net::{self, Interface, PortCheck};
use crate::localserver::{LocalServerManager, LogEntry, ServerConfig, ServerKind, ServerStatus};

/// The running servers, managed beside `AppState` (desktop only).
#[derive(Default)]
pub struct LocalServers(pub Mutex<LocalServerManager>);

fn parse_ip(bind: &str) -> Result<IpAddr, String> {
    bind.trim().parse().map_err(|_| format!("Not an IP address: {bind}"))
}

#[tauri::command]
pub fn local_server_interfaces() -> Vec<Interface> {
    net::interfaces()
}

#[tauri::command]
pub async fn local_server_check_port(
    servers: State<'_, LocalServers>,
    kind: ServerKind,
    bind: String,
    port: u16,
) -> Result<PortCheck, String> {
    let ip = parse_ip(&bind)?;
    if port == 0 {
        return Err("Choose a port between 1 and 65535".into());
    }
    // Our own server already on it is not a conflict.
    if servers.0.lock().await.holds(kind, port) {
        return Ok(PortCheck { free: true, problem: None, suggestion: None });
    }
    Ok(net::check_port(kind, ip, port))
}

#[tauri::command]
pub async fn local_server_start(
    app: AppHandle,
    servers: State<'_, LocalServers>,
    config: ServerConfig,
) -> Result<ServerStatus, String> {
    let emit = Arc::new(move |entry: &LogEntry| {
        let _ = app.emit("local-server-log", entry);
    });
    servers.0.lock().await.start(config, emit).await
}

#[tauri::command]
pub async fn local_server_stop(servers: State<'_, LocalServers>, kind: ServerKind) -> Result<ServerStatus, String> {
    let mut manager = servers.0.lock().await;
    manager.stop(kind);
    Ok(manager.status(kind))
}

#[tauri::command]
pub async fn local_server_status(servers: State<'_, LocalServers>) -> Result<Vec<ServerStatus>, String> {
    Ok(servers.0.lock().await.statuses())
}

#[tauri::command]
pub async fn local_server_clear_log(servers: State<'_, LocalServers>, kind: ServerKind) -> Result<(), String> {
    servers.0.lock().await.clear_log(kind);
    Ok(())
}
