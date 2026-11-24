//! Proxmox sessions: a Proxmox VE node or cluster over its REST API. The
//! pages show the nodes, guests, storage and tasks, run a guest's power
//! buttons and snapshots, and open its consoles in tabs of their own:
//! SPICE in the SPICE player, the display over VNC, a container's or serial
//! console and a node's shell in a terminal (see [`ws`]).
//!
//! An API token can run the pages, but Proxmox opens consoles for a user
//! login only, so a session may hold both: the token for the pages, and a
//! user login made when the first console opens.
//!
//! The backend makes every request; the window only names nodes, guests
//! and tasks this connection listed itself (see [`ProxmoxConn::check_guest`]).

pub mod client;
pub mod model;
pub mod ws;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use client::{Auth, Client, Params, Product, ProxmoxError, Ticket, UserLogin};
use model::*;

/// How many requests one page sends at once.
const PARALLEL: usize = 6;
/// Tasks shown at most.
const MAX_TASKS: usize = 200;
/// Task log lines read at a time.
const LOG_LINES: u64 = 1000;

/// A Proxmox session's own settings, stored with it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxmoxOptions {
    /// SHA-256 fingerprints (`AB:CD:..`) of certificates this session trusts
    /// although the system does not (Proxmox's own are self-signed). Changed
    /// only by answering the prompt; an editor's save keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certs: Vec<String>,
    /// The login's realm: `pam`, `pve`, `pbs`, or a directory's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
    /// An API token for the pages: `user@realm!name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
    /// The token's secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_secret: Option<String>,
}

impl ProxmoxOptions {
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty() && self.realm.is_none() && self.token_id.is_none() && self.token_secret.is_none()
    }

    /// The token, when both its id and secret are set.
    pub fn token(&self) -> Option<(&str, &str)> {
        let id = self.token_id.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
        let secret = self.token_secret.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
        Some((id, secret))
    }

    pub fn realm(&self) -> &str {
        self.realm.as_deref().map(str::trim).filter(|r| !r.is_empty()).unwrap_or("pam")
    }
}

/// A token id as Proxmox writes it: `user@realm!name`.
pub fn valid_token_id(id: &str) -> bool {
    let Some((user, name)) = id.split_once('!') else { return false };
    user.contains('@')
        && !name.is_empty()
        && !id.chars().any(|c| c.is_whitespace() || c.is_control() || c == '=' || c == ':')
}

/// What connecting found.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub product: Product,
    pub version: Option<String>,
    /// Logged in with an API token: consoles need a user login too.
    pub token: bool,
    /// `user@realm` (or the token's id) the pages run as.
    pub user: String,
}

/// A console a tab opens: a guest's, or a node's shell.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleTarget {
    /// The Proxmox session; its open connection makes the console.
    pub session_id: String,
    pub node: String,
    /// `qemu`, `lxc`, or `node` for the node's shell.
    pub kind: String,
    #[serde(default)]
    pub vmid: Option<u32>,
    /// A VM's serial port for a text console (`serial0`).
    #[serde(default)]
    pub serial: Option<String>,
}

#[derive(Default)]
struct Known {
    nodes: HashSet<String>,
    guests: HashSet<(String, String, u32)>,
    tasks: HashSet<String>,
}

/// One logged-in Proxmox server.
pub struct ProxmoxConn {
    pub client: Client,
    pub session_id: String,
    pub version: Option<String>,
    pub user: String,
    known: std::sync::Mutex<Known>,
}

fn guest_path(node: &str, kind: &str, vmid: u32) -> String {
    format!("/api2/json/nodes/{node}/{kind}/{vmid}")
}

impl ProxmoxConn {
    /// Log in, with `auth` made by the caller (a token or a ticket), and read
    /// the version, which also proves the login works.
    pub async fn open(client: Client, session_id: String, auth: Auth, user: String) -> Result<Self, ProxmoxError> {
        client.set_auth(auth).await;
        let v = client.get("/api2/json/version").await?;
        let version = v["version"].as_str().map(|s| {
            let r = v["release"].as_str().unwrap_or_default();
            if r.is_empty() || s.starts_with(r) { s.to_string() } else { format!("{s} ({r})") }
        });
        Ok(Self { client, session_id, version, user, known: Default::default() })
    }

    pub async fn opened(&self) -> Opened {
        Opened { product: self.client.product, version: self.version.clone(), token: self.client.uses_token().await, user: self.user.clone() }
    }

    fn pve_only(&self) -> Result<(), ProxmoxError> {
        match self.client.product {
            Product::Pve => Ok(()),
            Product::Pbs => Err(ProxmoxError::Other("This page is for Proxmox VE; Backup Server pages come in a later version".into())),
        }
    }

    fn remember(&self, list: &[Resource]) {
        if let Ok(mut k) = self.known.lock() {
            for r in list {
                match (r.kind.as_str(), &r.node, r.vmid) {
                    ("node", Some(n), _) => {
                        k.nodes.insert(n.clone());
                    }
                    ("qemu" | "lxc", Some(n), Some(id)) => {
                        k.nodes.insert(n.clone());
                        k.guests.insert((n.clone(), r.kind.clone(), id));
                    }
                    _ => {}
                }
            }
        }
    }

    fn remember_tasks<'a>(&self, upids: impl IntoIterator<Item = &'a str>) {
        if let Ok(mut k) = self.known.lock() {
            k.tasks.extend(upids.into_iter().map(str::to_owned));
        }
    }

    /// A node the window named: one this connection listed.
    async fn check_node(&self, node: &str) -> Result<(), ProxmoxError> {
        if !valid_node(node) {
            return Err(ProxmoxError::Other("Not a node name".into()));
        }
        if self.known.lock().map(|k| k.nodes.contains(node)).unwrap_or(false) {
            return Ok(());
        }
        // Not listed yet (a console tab of an earlier connection): list now.
        self.resources().await?;
        if self.known.lock().map(|k| k.nodes.contains(node)).unwrap_or(false) {
            Ok(())
        } else {
            Err(ProxmoxError::Other(format!("No node {node} here; refresh the page")))
        }
    }

    /// A guest the window named: one this connection listed.
    async fn check_guest(&self, node: &str, kind: &str, vmid: u32) -> Result<(), ProxmoxError> {
        if !valid_node(node) || !valid_kind(kind) {
            return Err(ProxmoxError::Other("Not a guest".into()));
        }
        let key = (node.to_string(), kind.to_string(), vmid);
        if self.known.lock().map(|k| k.guests.contains(&key)).unwrap_or(false) {
            return Ok(());
        }
        self.resources().await?;
        if self.known.lock().map(|k| k.guests.contains(&key)).unwrap_or(false) {
            Ok(())
        } else {
            Err(ProxmoxError::Other(format!("Guest {vmid} is not on {node} any more; refresh the page")))
        }
    }

    fn check_task(&self, node: &str, upid: &str) -> Result<(), ProxmoxError> {
        let ok = valid_node(node)
            && valid_upid(upid)
            && upid.split(':').nth(1) == Some(node)
            && self.known.lock().map(|k| k.tasks.contains(upid)).unwrap_or(false);
        if ok { Ok(()) } else { Err(ProxmoxError::Other("Unknown task; refresh the page".into())) }
    }

    /// Everything in the cluster: nodes, guests, storage, pools.
    pub async fn resources(&self) -> Result<Vec<Resource>, ProxmoxError> {
        self.pve_only()?;
        let list = resources(&self.client.get("/api2/json/cluster/resources").await?);
        self.remember(&list);
        Ok(list)
    }

    pub async fn overview(&self) -> Result<Overview, ProxmoxError> {
        self.pve_only()?;
        let (version, status, res) = tokio::join!(
            self.client.get("/api2/json/version"),
            self.client.get("/api2/json/cluster/status"),
            self.resources(),
        );
        let version = version.unwrap_or(Value::Null);
        let cluster = cluster(&status.unwrap_or(Value::Null));
        let res = res?;
        // Nodes as the cluster lists them, else as the resources do (a
        // token without Sys.Audit on /cluster sees no cluster status).
        let mut nodes: Vec<(String, bool, Option<String>)> = cluster.nodes.clone();
        if nodes.is_empty() {
            nodes = res
                .iter()
                .filter(|r| r.kind == "node")
                .filter_map(|r| r.node.clone().map(|n| (n, r.status.as_deref() == Some("online"), None)))
                .collect();
        }
        // Each online node's own status, a few at a time.
        let sem = tokio::sync::Semaphore::new(PARALLEL);
        let statuses = futures_util::future::join_all(nodes.iter().map(|(name, online, _)| {
            let sem = &sem;
            async move {
                if !*online || !valid_node(name) {
                    return Value::Null;
                }
                let _permit = sem.acquire().await.ok();
                self.client.get(&format!("/api2/json/nodes/{name}/status")).await.unwrap_or(Value::Null)
            }
        }))
        .await;
        let nodes = nodes.into_iter().zip(statuses).map(|((name, online, ip), st)| node_status(&name, online, ip, &st)).collect();
        Ok(Overview {
            product: "pve".into(),
            version: version["version"].as_str().map(str::to_owned).or_else(|| self.version.clone()),
            release: version["release"].as_str().map(str::to_owned),
            cluster_name: cluster.name,
            quorate: cluster.quorate,
            nodes,
        })
    }

    pub async fn guest(&self, node: &str, kind: &str, vmid: u32) -> Result<GuestDetail, ProxmoxError> {
        self.check_guest(node, kind, vmid).await?;
        let base = guest_path(node, kind, vmid);
        let (p_status, p_config, p_snaps) = (format!("{base}/status/current"), format!("{base}/config"), format!("{base}/snapshot"));
        let (status, config, snaps) = tokio::join!(self.client.get(&p_status), self.client.get(&p_config), self.client.get(&p_snaps));
        let status = status?;
        let config = config.unwrap_or(Value::Null);
        let mut d = guest_detail(node, kind, vmid, &status, &config);
        d.snapshots = snapshots(&snaps.unwrap_or(Value::Null));
        if d.status.as_deref() == Some("running") {
            d.addresses = if kind == "lxc" {
                self.client.get(&format!("{base}/interfaces")).await.map(|v| lxc_addresses(&v)).unwrap_or_default()
            } else if d.agent {
                self.client.get(&format!("{base}/agent/network-get-interfaces")).await.map(|v| agent_addresses(&v)).unwrap_or_default()
            } else {
                Vec::new()
            };
        }
        Ok(d)
    }

    /// A guest's power button; the task it started.
    pub async fn guest_action(&self, node: &str, kind: &str, vmid: u32, action: &str) -> Result<String, ProxmoxError> {
        self.check_guest(node, kind, vmid).await?;
        let allowed: &[&str] = if kind == "qemu" {
            &["start", "shutdown", "stop", "reboot", "reset", "suspend", "resume"]
        } else {
            &["start", "shutdown", "stop", "reboot", "suspend", "resume"]
        };
        if !allowed.contains(&action) {
            return Err(ProxmoxError::Other(format!("Not a guest action: {action}")));
        }
        let v = self.client.post(&format!("{}/status/{action}", guest_path(node, kind, vmid)), &json!({})).await?;
        Ok(self.started(&v))
    }

    fn started(&self, v: &Value) -> String {
        let upid = v.as_str().unwrap_or_default().to_string();
        if valid_upid(&upid) {
            self.remember_tasks([upid.as_str()]);
        }
        upid
    }

    pub async fn snapshot_create(&self, node: &str, kind: &str, vmid: u32, name: &str, description: &str, vmstate: bool) -> Result<String, ProxmoxError> {
        self.check_guest(node, kind, vmid).await?;
        if !valid_snapshot(name) {
            return Err(ProxmoxError::Other("A snapshot name starts with a letter, then letters, digits, - and _".into()));
        }
        let mut body = json!({ "snapname": name });
        if !description.trim().is_empty() {
            body["description"] = json!(description.trim());
        }
        if kind == "qemu" && vmstate {
            body["vmstate"] = json!(1);
        }
        let v = self.client.post(&format!("{}/snapshot", guest_path(node, kind, vmid)), &body).await?;
        Ok(self.started(&v))
    }

    pub async fn snapshot_rollback(&self, node: &str, kind: &str, vmid: u32, name: &str) -> Result<String, ProxmoxError> {
        self.check_guest(node, kind, vmid).await?;
        if !valid_snapshot(name) {
            return Err(ProxmoxError::Other("Not a snapshot name".into()));
        }
        let v = self.client.post(&format!("{}/snapshot/{name}/rollback", guest_path(node, kind, vmid)), &json!({})).await?;
        Ok(self.started(&v))
    }

    pub async fn snapshot_delete(&self, node: &str, kind: &str, vmid: u32, name: &str) -> Result<String, ProxmoxError> {
        self.check_guest(node, kind, vmid).await?;
        if !valid_snapshot(name) {
            return Err(ProxmoxError::Other("Not a snapshot name".into()));
        }
        let v = self.client.delete(&format!("{}/snapshot/{name}", guest_path(node, kind, vmid))).await?;
        Ok(self.started(&v))
    }

    /// Restart or shut down a node.
    pub async fn node_action(&self, node: &str, command: &str) -> Result<(), ProxmoxError> {
        self.check_node(node).await?;
        if command != "reboot" && command != "shutdown" {
            return Err(ProxmoxError::Other(format!("Not a node action: {command}")));
        }
        self.client.post(&format!("/api2/json/nodes/{node}/status"), &json!({ "command": command })).await.map(|_| ())
    }

    pub async fn tasks(&self) -> Result<Vec<Task>, ProxmoxError> {
        self.pve_only()?;
        let list = tasks(&self.client.get("/api2/json/cluster/tasks").await?, MAX_TASKS);
        self.remember_tasks(list.iter().map(|t| t.upid.as_str()));
        Ok(list)
    }

    /// A task's log from line `start` on, and whether it still runs.
    pub async fn task_log(&self, node: &str, upid: &str, start: u64) -> Result<TaskLog, ProxmoxError> {
        self.check_task(node, upid)?;
        let base = format!("/api2/json/nodes/{node}/tasks/{upid}");
        let (p_log, p_status) = (format!("{base}/log"), format!("{base}/status"));
        let query = [("start", start.to_string()), ("limit", LOG_LINES.to_string())];
        let (log, status) = tokio::join!(self.client.get_q(&p_log, &query), self.client.get(&p_status));
        Ok(task_log(&log?, &status.unwrap_or(Value::Null), start))
    }

    pub async fn task_stop(&self, node: &str, upid: &str) -> Result<(), ProxmoxError> {
        self.check_task(node, upid)?;
        self.client.delete(&format!("/api2/json/nodes/{node}/tasks/{upid}")).await.map(|_| ())
    }

    /// The login the consoles use; none: the user must log in for them.
    async fn console_login(&self) -> Result<Ticket, ProxmoxError> {
        self.client.console_ticket().await.ok_or(ProxmoxError::ConsoleLoginNeeded)
    }

    /// Where a console's API calls go: the guest's, or the node's.
    async fn console_base(&self, t: &ConsoleTarget) -> Result<String, ProxmoxError> {
        self.pve_only()?;
        match t.kind.as_str() {
            "node" => {
                self.check_node(&t.node).await?;
                Ok(format!("/api2/json/nodes/{}", t.node))
            }
            kind => {
                let vmid = t.vmid.ok_or_else(|| ProxmoxError::Other("No guest id".into()))?;
                self.check_guest(&t.node, kind, vmid).await?;
                Ok(guest_path(&t.node, kind, vmid))
            }
        }
    }

    /// A VM's SPICE console: what the SPICE player connects with. Good for
    /// about 30 seconds.
    pub async fn spice(&self, t: &ConsoleTarget) -> Result<SpiceTicket, ProxmoxError> {
        if t.kind != "qemu" {
            return Err(ProxmoxError::Other("Only a VM has a SPICE console".into()));
        }
        let base = self.console_base(t).await?;
        let login = self.console_login().await?;
        // spiceproxy answers on the address this app reaches the node at.
        let body = json!({ "proxy": self.client.host });
        let v = self.client.send_as(Some(&Auth::Ticket(login)), reqwest::Method::POST, &format!("{base}/spiceproxy"), &[], Some(&body)).await?;
        spice_ticket(&v).ok_or_else(|| ProxmoxError::Other("Proxmox gave no SPICE ticket; is the VM's display set to SPICE?".into()))
    }

    /// A guest's display over VNC: the stream and the VNC password.
    pub async fn vnc(&self, t: &ConsoleTarget) -> Result<(ws::WsLink, String), ProxmoxError> {
        if t.kind == "node" {
            return Err(ProxmoxError::Other("A node's console opens as a terminal".into()));
        }
        let base = self.console_base(t).await?;
        let login = self.console_login().await?;
        let auth = Auth::Ticket(login.clone());
        let path = format!("{base}/vncproxy");
        let mut body = json!({ "websocket": 1 });
        if t.kind == "qemu" {
            // A short random password instead of the ticket: VNC's own login
            // only looks at the first eight characters.
            body["generate-password"] = json!(1);
        }
        let v = match self.client.send_as(Some(&auth), reqwest::Method::POST, &path, &[], Some(&body)).await {
            Err(ProxmoxError::Status { code: 400, .. }) if t.kind == "qemu" => {
                self.client.send_as(Some(&auth), reqwest::Method::POST, &path, &[], Some(&json!({ "websocket": 1 }))).await?
            }
            other => other?,
        };
        let p = proxy_ticket(&v).ok_or_else(|| ProxmoxError::Other("Proxmox gave no console ticket".into()))?;
        let link = ws::open(&self.client, &login, &format!("{base}/vncwebsocket"), p.port, &p.ticket, ws::Keepalive::Ping).await?;
        Ok((link, p.password.unwrap_or(p.ticket)))
    }

    /// A text console: a container's, a VM's serial port, or a node's shell.
    pub async fn term(&self, t: &ConsoleTarget) -> Result<ws::WsLink, ProxmoxError> {
        let base = self.console_base(t).await?;
        let login = self.console_login().await?;
        let mut body = json!({});
        if t.kind == "qemu" {
            let serial = t.serial.as_deref().unwrap_or("serial0");
            let ok = serial.strip_prefix("serial").is_some_and(|n| matches!(n, "0" | "1" | "2" | "3"));
            if !ok {
                return Err(ProxmoxError::Other("Not a serial port".into()));
            }
            body["serial"] = json!(serial);
        }
        let v = self.client.send_as(Some(&Auth::Ticket(login.clone())), reqwest::Method::POST, &format!("{base}/termproxy"), &[], Some(&body)).await?;
        let p = proxy_ticket(&v).ok_or_else(|| ProxmoxError::Other("Proxmox gave no console ticket".into()))?;
        let mut link = ws::open(&self.client, &login, &format!("{base}/vncwebsocket"), p.port, &p.ticket, ws::Keepalive::Termproxy).await?;
        let user = p.user.unwrap_or_else(|| login.user.clone());
        ws::termproxy_login(&mut link, &user, &p.ticket).await?;
        Ok(link)
    }
}

/// The open Proxmox sessions, by connection id.
#[derive(Default)]
pub struct ProxmoxManager {
    conns: std::sync::Mutex<HashMap<String, Arc<ProxmoxConn>>>,
}

impl ProxmoxManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<ProxmoxConn>> {
        self.conns.lock().ok()?.get(id).cloned()
    }

    /// An open connection of a saved session: what its console tabs use.
    pub fn for_session(&self, session_id: &str) -> Option<Arc<ProxmoxConn>> {
        self.conns.lock().ok()?.values().find(|c| c.session_id == session_id).cloned()
    }

    pub fn insert(&self, id: String, conn: ProxmoxConn) {
        if let Ok(mut m) = self.conns.lock() {
            m.insert(id, Arc::new(conn));
        }
    }

    pub fn remove(&self, id: &str) -> Option<Arc<ProxmoxConn>> {
        self.conns.lock().ok()?.remove(id)
    }
}

#[cfg(test)]
#[path = "mock_tests.rs"]
mod mock_tests;
