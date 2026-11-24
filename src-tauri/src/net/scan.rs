//! A TCP connect scan: which of a host's ports accept a connection. Each
//! port gets a plain connect, closed again at once; nothing is sent.

use serde::Serialize;
use std::collections::HashMap;
use std::{io, net::IpAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};

/// At most this many ports in one scan (all of them).
pub const MAX_PORTS: usize = 65_535;
/// Connections open at once: the caller's choice, kept within this.
pub const MAX_CONCURRENCY: usize = 512;

/// What one port said.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PortState {
    /// Accepted the connection.
    Open,
    /// Refused it: the host is there, nothing listens.
    Closed,
    /// No answer in time: dropped by a firewall, or no host.
    Filtered,
}

/// What a scan reports while it runs.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ScanEvent {
    Open { port: u16, ms: f64 },
    Progress { done: usize, total: usize },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanSummary {
    pub open: Vec<u16>,
    pub closed: usize,
    pub filtered: usize,
    pub scanned: usize,
    pub cancelled: bool,
    pub ms: u64,
}

fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// Stop the scan `id`, if it runs.
pub fn cancel(id: &str) {
    if let Some(flag) = running().lock().unwrap_or_else(|e| e.into_inner()).get(id) {
        flag.store(true, Ordering::Relaxed);
    }
}

pub async fn probe(ip: IpAddr, port: u16, timeout: Duration) -> (PortState, f64) {
    let start = Instant::now();
    let state = port_state(super::tcp_connect(ip, port, timeout).await);
    (state, start.elapsed().as_secs_f64() * 1000.0)
}

pub(super) fn port_state(attempt: Option<io::Result<()>>) -> PortState {
    match attempt {
        Some(Ok(())) => PortState::Open,
        Some(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => PortState::Closed,
        Some(Err(_)) | None => PortState::Filtered,
    }
}

/// Scan `ports` of `ip`, `concurrency` at a time, telling `report` about
/// each open port and the progress (about a hundred times in all). Stops
/// early when `cancel(id)` is called.
pub async fn scan<F>(id: &str, ip: IpAddr, ports: Vec<u16>, timeout: Duration, concurrency: usize, report: F) -> ScanSummary
where
    F: Fn(ScanEvent) + Send + Sync,
{
    let flag = Arc::new(AtomicBool::new(false));
    running().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), flag.clone());
    let start = Instant::now();
    let total = ports.len();
    let step = (total / 100).max(1);
    let mut summary = ScanSummary::default();
    let mut results = stream::iter(ports)
        .map(|port| {
            let flag = flag.clone();
            async move {
                if flag.load(Ordering::Relaxed) {
                    return None;
                }
                let (state, ms) = probe(ip, port, timeout).await;
                Some((port, state, ms))
            }
        })
        .buffer_unordered(concurrency.clamp(1, MAX_CONCURRENCY));
    while let Some(r) = results.next().await {
        let Some((port, state, ms)) = r else { continue };
        summary.scanned += 1;
        match state {
            PortState::Open => {
                summary.open.push(port);
                report(ScanEvent::Open { port, ms });
            }
            PortState::Closed => summary.closed += 1,
            PortState::Filtered => summary.filtered += 1,
        }
        if summary.scanned % step == 0 || summary.scanned == total {
            report(ScanEvent::Progress { done: summary.scanned, total });
        }
    }
    summary.cancelled = flag.load(Ordering::Relaxed);
    summary.open.sort_unstable();
    summary.ms = start.elapsed().as_millis() as u64;
    running().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    summary
}
