//! A Proxmox Backup Server's pages: the server and its datastores, the
//! backups in them (by namespace, group and snapshot), and what can be done
//! with them: garbage collection, verifying, protecting and forgetting.
//!
//! The API is PBS's own (`/admin/datastore`, `/status/datastore-usage`);
//! the node is always `localhost`, as the PBS web UI calls it.

use serde::Serialize;
use serde_json::{json, Value};

use super::client::{Product, ProxmoxError};
use super::model::{node_status, NodeStatus};
use super::ProxmoxConn;

/// How a PBS calls itself in node paths.
pub const PBS_NODE: &str = "localhost";

fn s(v: &Value, key: &str) -> Option<String> {
    match &v[key] {
        Value::String(x) if !x.trim().is_empty() => Some(x.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn u(v: &Value, key: &str) -> Option<u64> {
    match &v[key] {
        Value::Number(n) => n.as_f64().filter(|x| *x >= 0.0).map(|x| x as u64),
        Value::String(x) => x.trim().parse().ok(),
        _ => None,
    }
}

fn flag(v: &Value, key: &str) -> bool {
    match &v[key] {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(x) => x == "1" || x.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A PBS id (datastore, backup id): a letter, digit or `_`, then those,
/// `.` and `-`.
fn safe_id(id: &str, max: usize) -> bool {
    let mut c = id.chars();
    c.next().is_some_and(|f| f.is_ascii_alphanumeric() || f == '_')
        && id.len() <= max
        && c.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

pub fn valid_store(store: &str) -> bool {
    safe_id(store, 32)
}

pub fn valid_backup_type(kind: &str) -> bool {
    matches!(kind, "vm" | "ct" | "host")
}

pub fn valid_backup_id(id: &str) -> bool {
    safe_id(id, 128)
}

/// A namespace: none (the root), or up to 7 ids joined by `/`.
pub fn valid_ns(ns: &str) -> bool {
    ns.is_empty() || (ns.split('/').count() <= 7 && ns.split('/').all(|p| safe_id(p, 32)))
}

/// A datastore, with how full it is.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Datastore {
    pub store: String,
    pub comment: Option<String>,
    /// Maintenance mode, as PBS writes it (`read-only`, `offline`...).
    pub maintenance: Option<String>,
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub avail: Option<u64>,
    /// When PBS expects it to be full, from its growth.
    pub full_at: Option<u64>,
    /// What the last garbage collection removed and left pending.
    pub gc_removed: Option<u64>,
    pub gc_pending: Option<u64>,
    /// Why its usage could not be read (an unmounted removable disk...).
    pub error: Option<String>,
}

/// `/admin/datastore` (names, comments, maintenance) joined with
/// `/status/datastore-usage` (sizes), sorted by name.
pub fn datastores(config: &Value, usage: &Value) -> Vec<Datastore> {
    let mut list: Vec<Datastore> = Vec::new();
    for c in config.as_array().into_iter().flatten() {
        let Some(store) = s(c, "store").or_else(|| s(c, "name")).filter(|n| valid_store(n)) else { continue };
        let maintenance = s(c, "maintenance-mode").or_else(|| s(c, "maintenance")).map(|m| {
            m.split(',').find_map(|p| p.strip_prefix("type=")).map(str::to_owned).unwrap_or(m)
        });
        list.push(Datastore { store, comment: s(c, "comment"), maintenance, ..Default::default() });
    }
    for e in usage.as_array().into_iter().flatten() {
        let Some(store) = s(e, "store").filter(|n| valid_store(n)) else { continue };
        let i = match list.iter().position(|d| d.store == store) {
            Some(i) => i,
            None => {
                list.push(Datastore { store, ..Default::default() });
                list.len() - 1
            }
        };
        let d = &mut list[i];
        d.total = u(e, "total");
        d.used = u(e, "used");
        d.avail = u(e, "avail");
        d.full_at = u(e, "estimated-full-date").filter(|t| *t > 0);
        d.gc_removed = u(&e["gc-status"], "removed-bytes");
        d.gc_pending = u(&e["gc-status"], "pending-bytes");
        d.error = s(e, "error");
    }
    list.sort_by(|a, b| a.store.cmp(&b.store));
    list
}

/// The server and its datastores.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PbsOverview {
    pub node: NodeStatus,
    pub datastores: Vec<Datastore>,
}

/// `/nodes/localhost/status`: as a PVE node's, but its root disk is `root`.
pub fn pbs_node(v: &Value) -> NodeStatus {
    let mut n = node_status("", true, None, v);
    if n.root_total.is_none() {
        n.root_used = u(&v["root"], "used");
        n.root_total = u(&v["root"], "total");
    }
    n.name = s(&v["info"], "hostname").unwrap_or_else(|| PBS_NODE.to_string());
    n
}

/// The namespaces of a datastore, the root (`""`) first.
pub fn namespaces(v: &Value) -> Vec<String> {
    let mut list: Vec<String> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| s(e, "ns").unwrap_or_default())
        .filter(|n| valid_ns(n))
        .collect();
    if !list.iter().any(String::is_empty) {
        list.push(String::new());
    }
    list.sort();
    list.dedup();
    list
}

/// A backup group: every backup of one VM, container or host.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackupGroup {
    /// `vm`, `ct` or `host`.
    pub kind: String,
    pub id: String,
    pub last: Option<u64>,
    pub count: u64,
    pub owner: Option<String>,
    pub comment: Option<String>,
}

/// Groups by type, then by id (numbers as numbers).
pub fn groups(v: &Value) -> Vec<BackupGroup> {
    let mut list: Vec<BackupGroup> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let kind = s(e, "backup-type").filter(|k| valid_backup_type(k))?;
            let id = s(e, "backup-id").filter(|i| valid_backup_id(i))?;
            Some(BackupGroup { kind, id, last: u(e, "last-backup"), count: u(e, "backup-count").unwrap_or(0), owner: s(e, "owner"), comment: s(e, "comment") })
        })
        .collect();
    let order = |k: &str| match k {
        "vm" => 0,
        "ct" => 1,
        _ => 2,
    };
    list.sort_by(|a, b| {
        order(&a.kind).cmp(&order(&b.kind)).then_with(|| match (a.id.parse::<u64>(), b.id.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.id.cmp(&b.id),
        })
    });
    list
}

/// One backup.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackupSnapshot {
    pub kind: String,
    pub id: String,
    pub time: u64,
    pub size: Option<u64>,
    pub protected: bool,
    /// The last verification: `ok` or `failed`; none: never verified.
    pub verified: Option<String>,
    pub encrypted: bool,
    pub comment: Option<String>,
    pub owner: Option<String>,
}

/// Snapshots newest first.
pub fn snapshots(v: &Value) -> Vec<BackupSnapshot> {
    let mut list: Vec<BackupSnapshot> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let kind = s(e, "backup-type").filter(|k| valid_backup_type(k))?;
            let id = s(e, "backup-id").filter(|i| valid_backup_id(i))?;
            let time = u(e, "backup-time")?;
            let encrypted = e["files"].as_array().is_some_and(|f| f.iter().any(|x| x["crypt-mode"].as_str() == Some("encrypt")));
            Some(BackupSnapshot {
                kind,
                id,
                time,
                size: u(e, "size"),
                protected: flag(e, "protected"),
                verified: s(&e["verification"], "state"),
                encrypted,
                comment: s(e, "comment"),
                owner: s(e, "owner"),
            })
        })
        .collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.time));
    list
}

/// The query naming one backup (and its namespace, when not the root).
fn snapshot_query(ns: &str, kind: &str, id: &str, time: u64) -> Vec<(&'static str, String)> {
    let mut q = vec![("backup-type", kind.to_string()), ("backup-id", id.to_string()), ("backup-time", time.to_string())];
    if !ns.is_empty() {
        q.push(("ns", ns.to_string()));
    }
    q
}

fn bad(what: &str) -> ProxmoxError {
    ProxmoxError::Other(format!("Not a valid {what}"))
}

fn check_backup(ns: &str, kind: &str, id: &str) -> Result<(), ProxmoxError> {
    if !valid_ns(ns) {
        return Err(bad("namespace"));
    }
    if !valid_backup_type(kind) || !valid_backup_id(id) {
        return Err(bad("backup group"));
    }
    Ok(())
}

impl ProxmoxConn {
    fn pbs_only(&self) -> Result<(), ProxmoxError> {
        match self.client.product {
            Product::Pbs => Ok(()),
            Product::Pve => Err(ProxmoxError::Other("This page is for Proxmox Backup Server".into())),
        }
    }

    fn remember_stores(&self, list: &[Datastore]) {
        if let Ok(mut k) = self.known.lock() {
            k.stores.extend(list.iter().map(|d| d.store.clone()));
        }
    }

    /// A datastore the window named: one this connection listed.
    async fn check_store(&self, store: &str) -> Result<String, ProxmoxError> {
        self.pbs_only()?;
        if !valid_store(store) {
            return Err(bad("datastore name"));
        }
        let known = |c: &Self| c.known.lock().map(|k| k.stores.contains(store)).unwrap_or(false);
        if !known(self) {
            self.pbs_datastores().await?;
            if !known(self) {
                return Err(ProxmoxError::Other(format!("No datastore {store} here; refresh the page")));
            }
        }
        Ok(format!("/api2/json/admin/datastore/{store}"))
    }

    async fn pbs_datastores(&self) -> Result<Vec<Datastore>, ProxmoxError> {
        let (config, usage) = tokio::join!(self.client.get("/api2/json/admin/datastore"), self.client.get("/api2/json/status/datastore-usage"));
        // A user may see the usage but not the configuration, or the other way round.
        let (config, usage) = match (config, usage) {
            (Err(e), Err(_)) => return Err(e),
            (c, u) => (c.unwrap_or(Value::Null), u.unwrap_or(Value::Null)),
        };
        let list = datastores(&config, &usage);
        self.remember_stores(&list);
        Ok(list)
    }

    pub async fn pbs_overview(&self) -> Result<PbsOverview, ProxmoxError> {
        self.pbs_only()?;
        let status = format!("/api2/json/nodes/{PBS_NODE}/status");
        let (node, stores) = tokio::join!(self.client.get(&status), self.pbs_datastores());
        Ok(PbsOverview { node: node.map(|v| pbs_node(&v)).unwrap_or_default(), datastores: stores? })
    }

    pub async fn pbs_namespaces(&self, store: &str) -> Result<Vec<String>, ProxmoxError> {
        let base = self.check_store(store).await?;
        match self.client.get(&format!("{base}/namespace")).await {
            Ok(v) => Ok(namespaces(&v)),
            // Older servers have no namespaces: just the root.
            Err(ProxmoxError::Status { code: 404 | 501, .. }) => Ok(vec![String::new()]),
            Err(e) => Err(e),
        }
    }

    pub async fn pbs_groups(&self, store: &str, ns: &str) -> Result<Vec<BackupGroup>, ProxmoxError> {
        let base = self.check_store(store).await?;
        if !valid_ns(ns) {
            return Err(bad("namespace"));
        }
        let q: Vec<(&str, String)> = if ns.is_empty() { vec![] } else { vec![("ns", ns.to_string())] };
        Ok(groups(&self.client.get_q(&format!("{base}/groups"), &q).await?))
    }

    pub async fn pbs_snapshots(&self, store: &str, ns: &str, kind: &str, id: &str) -> Result<Vec<BackupSnapshot>, ProxmoxError> {
        let base = self.check_store(store).await?;
        check_backup(ns, kind, id)?;
        let mut q = vec![("backup-type", kind.to_string()), ("backup-id", id.to_string())];
        if !ns.is_empty() {
            q.push(("ns", ns.to_string()));
        }
        Ok(snapshots(&self.client.get_q(&format!("{base}/snapshots"), &q).await?))
    }

    /// Start garbage collection on a datastore; its task.
    pub async fn pbs_gc(&self, store: &str) -> Result<String, ProxmoxError> {
        let base = self.check_store(store).await?;
        let v = self.client.post(&format!("{base}/gc"), &json!({})).await?;
        Ok(self.started(&v))
    }

    /// Verify a whole datastore, or one backup; its task.
    pub async fn pbs_verify(&self, store: &str, ns: &str, backup: Option<(&str, &str, u64)>) -> Result<String, ProxmoxError> {
        let base = self.check_store(store).await?;
        let mut body = json!({});
        if let Some((kind, id, time)) = backup {
            check_backup(ns, kind, id)?;
            body = json!({ "backup-type": kind, "backup-id": id, "backup-time": time });
        } else if !valid_ns(ns) {
            return Err(bad("namespace"));
        }
        if !ns.is_empty() {
            body["ns"] = json!(ns);
        }
        let v = self.client.post(&format!("{base}/verify"), &body).await?;
        Ok(self.started(&v))
    }

    /// Protect a backup from pruning and forgetting, or stop protecting it.
    pub async fn pbs_protect(&self, store: &str, ns: &str, kind: &str, id: &str, time: u64, protected: bool) -> Result<(), ProxmoxError> {
        let base = self.check_store(store).await?;
        check_backup(ns, kind, id)?;
        let mut body = json!({ "backup-type": kind, "backup-id": id, "backup-time": time, "protected": protected });
        if !ns.is_empty() {
            body["ns"] = json!(ns);
        }
        self.client.send_as(None, reqwest::Method::PUT, &format!("{base}/protected"), &[], Some(&body)).await.map(|_| ())
    }

    /// Forget (delete) a backup.
    pub async fn pbs_forget(&self, store: &str, ns: &str, kind: &str, id: &str, time: u64) -> Result<(), ProxmoxError> {
        let base = self.check_store(store).await?;
        check_backup(ns, kind, id)?;
        let q = snapshot_query(ns, kind, id, time);
        self.client.send_as(None, reqwest::Method::DELETE, &format!("{base}/snapshots"), &q, None).await.map(|_| ())
    }
}

#[cfg(test)]
#[path = "pbs_tests.rs"]
mod tests;
