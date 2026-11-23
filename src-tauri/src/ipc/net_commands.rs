//! Network tools for the window: resolve, ping, port scan, Wake-on-LAN.
//! See `crate::net`.

use crate::net::{self, scan, wol, PingReply};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// The addresses `host` resolves to, IPv4 first.
#[tauri::command]
pub async fn net_resolve(host: String) -> Result<Vec<String>, String> {
    Ok(net::resolve(&host).await?.into_iter().map(|ip| ip.to_string()).collect())
}

fn ip_of(address: &str) -> Result<IpAddr, String> {
    address.trim().trim_start_matches('[').trim_end_matches(']').parse().map_err(|_| format!("Not an IP address: {address}"))
}

/// One ping of `address` (an IP, from `net_resolve`). `method`: `auto`
/// (ICMP, TCP to `port` where this device allows no ICMP), `icmp` or `tcp`.
#[tauri::command]
pub async fn net_ping(address: String, method: Option<String>, port: Option<u16>, timeout_ms: Option<u64>, seq: Option<u16>) -> Result<PingReply, String> {
    let ip = ip_of(&address)?;
    let method = method.unwrap_or_else(|| "auto".into());
    if !matches!(method.as_str(), "auto" | "icmp" | "tcp") {
        return Err(format!("Unknown ping method: {method}"));
    }
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(2000).clamp(100, 30_000));
    Ok(net::ping(ip, &method, port.filter(|p| *p != 0).unwrap_or(443), timeout, seq.unwrap_or(0)).await)
}

/// Scan `ports` of `address`, reporting open ports and progress on
/// `on_event` as it goes. `id` names the scan for `net_scan_cancel`.
#[tauri::command]
pub async fn net_scan_ports(
    id: String,
    address: String,
    ports: Vec<u16>,
    timeout_ms: Option<u64>,
    concurrency: Option<usize>,
    on_event: tauri::ipc::Channel<scan::ScanEvent>,
) -> Result<scan::ScanSummary, String> {
    let ip = ip_of(&address)?;
    let mut ports: Vec<u16> = ports.into_iter().filter(|p| *p != 0).collect();
    ports.sort_unstable();
    ports.dedup();
    if ports.is_empty() {
        return Err("No ports to scan".into());
    }
    if ports.len() > scan::MAX_PORTS {
        ports.truncate(scan::MAX_PORTS);
    }
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(1000).clamp(50, 10_000));
    let default = if cfg!(mobile) { 32 } else { 128 };
    let concurrency = concurrency.unwrap_or(default);
    tracing::info!("port scan of {} ports at {} at a time", ports.len(), concurrency);
    Ok(scan::scan(&id, ip, ports, timeout, concurrency, move |e| {
        let _ = on_event.send(e);
    })
    .await)
}

#[tauri::command]
pub fn net_scan_cancel(id: String) {
    scan::cancel(&id);
}

/// Send a Wake-on-LAN magic packet for `mac` to `broadcast` (default
/// 255.255.255.255; a name or a directed broadcast address such as
/// 10.0.0.255 also works) on `port` (default 9).
#[tauri::command]
pub async fn net_wake(mac: String, broadcast: Option<String>, port: Option<u16>) -> Result<(), String> {
    let mac = wol::parse_mac(&mac)?;
    let port = port.filter(|p| *p != 0).unwrap_or(wol::DEFAULT_PORT);
    let target = broadcast.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    let ip = match target {
        None => IpAddr::V4(wol::DEFAULT_BROADCAST),
        Some(b) => *net::resolve(&b).await?.first().ok_or("No address")?,
    };
    tokio::task::spawn_blocking(move || wol::send(mac, SocketAddr::new(ip, port)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Wake-on-LAN to {ip}:{port}: {e}"))
}
