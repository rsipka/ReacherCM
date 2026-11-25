//! What the pages show, read out of the Proxmox API's answers. Plain
//! functions over JSON, so they are tested on recorded answers.

use serde::Serialize;
use serde_json::Value;

fn s(v: &Value, key: &str) -> Option<String> {
    match &v[key] {
        Value::String(x) if !x.trim().is_empty() => Some(x.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn f(v: &Value, key: &str) -> Option<f64> {
    match &v[key] {
        Value::Number(n) => n.as_f64(),
        Value::String(x) => x.trim().parse().ok(),
        _ => None,
    }
}

fn u(v: &Value, key: &str) -> Option<u64> {
    f(v, key).filter(|x| *x >= 0.0).map(|x| x as u64)
}

fn flag(v: &Value, key: &str) -> bool {
    match &v[key] {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(x) => x == "1" || x.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Tags as Proxmox keeps them: `a;b` (also `,` and spaces in older ones).
pub fn tags(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default().split([';', ',', ' ']).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// A node name as Proxmox allows it: a host name.
pub fn valid_node(node: &str) -> bool {
    !node.is_empty() && node.len() <= 63 && node.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') && !node.starts_with('.')
}

/// A guest kind in an API path.
pub fn valid_kind(kind: &str) -> bool {
    kind == "qemu" || kind == "lxc"
}

/// A task id (UPID): `UPID:node:pid:pstart:starttime:type:id:user:`.
pub fn valid_upid(upid: &str) -> bool {
    upid.starts_with("UPID:")
        && upid.len() < 256
        && upid.chars().all(|c| c.is_ascii_alphanumeric() || ":@!._-=".contains(c))
}

/// A snapshot name: a letter, then letters, digits, `-` and `_`.
pub fn valid_snapshot(name: &str) -> bool {
    let mut c = name.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic()) && name.len() <= 40 && c.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One entry of `/cluster/resources`: a node, a guest, a storage or a pool.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    /// `node`, `qemu`, `lxc`, `storage`, `pool`, `sdn`.
    pub kind: String,
    pub id: String,
    pub node: Option<String>,
    pub vmid: Option<u32>,
    pub name: Option<String>,
    pub status: Option<String>,
    /// CPU use, 0 to 1 of `maxcpu`.
    pub cpu: Option<f64>,
    pub maxcpu: Option<f64>,
    pub mem: Option<u64>,
    pub maxmem: Option<u64>,
    pub disk: Option<u64>,
    pub maxdisk: Option<u64>,
    pub uptime: Option<u64>,
    pub tags: Vec<String>,
    pub template: bool,
    pub lock: Option<String>,
    pub hastate: Option<String>,
    pub pool: Option<String>,
    /// Storage only: its id, type and what it holds.
    pub storage: Option<String>,
    pub plugintype: Option<String>,
    pub content: Option<String>,
    pub shared: bool,
}

pub fn resource(v: &Value) -> Option<Resource> {
    let kind = s(v, "type")?;
    Some(Resource {
        id: s(v, "id").unwrap_or_default(),
        node: s(v, "node"),
        vmid: u(v, "vmid").and_then(|x| u32::try_from(x).ok()),
        name: s(v, "name"),
        status: s(v, "status"),
        cpu: f(v, "cpu"),
        maxcpu: f(v, "maxcpu"),
        mem: u(v, "mem"),
        maxmem: u(v, "maxmem"),
        disk: u(v, "disk"),
        maxdisk: u(v, "maxdisk"),
        uptime: u(v, "uptime"),
        tags: tags(v["tags"].as_str()),
        template: flag(v, "template"),
        lock: s(v, "lock"),
        hastate: s(v, "hastate"),
        pool: s(v, "pool"),
        storage: s(v, "storage"),
        plugintype: s(v, "plugintype"),
        content: s(v, "content"),
        shared: flag(v, "shared"),
        kind,
    })
}

/// Every resource, guests by id, nodes and storage by name.
pub fn resources(list: &Value) -> Vec<Resource> {
    let mut out: Vec<Resource> = list.as_array().map(|a| a.iter().filter_map(resource).collect()).unwrap_or_default();
    out.sort_by(|a, b| {
        let rank = |r: &Resource| match r.kind.as_str() {
            "node" => 0,
            "qemu" | "lxc" => 1,
            "storage" => 2,
            _ => 3,
        };
        rank(a)
            .cmp(&rank(b))
            .then(a.vmid.cmp(&b.vmid))
            .then(a.node.cmp(&b.node))
            .then(a.storage.cmp(&b.storage))
            .then(a.id.cmp(&b.id))
    });
    out
}

/// A node's own facts, from `/nodes/{node}/status`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub name: String,
    pub online: bool,
    pub ip: Option<String>,
    pub pve_version: Option<String>,
    pub kernel: Option<String>,
    pub cpu_model: Option<String>,
    pub cpus: Option<u64>,
    pub sockets: Option<u64>,
    pub cpu: Option<f64>,
    /// Load averages over 1, 5 and 15 minutes.
    pub load: Vec<f64>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub swap_used: Option<u64>,
    pub swap_total: Option<u64>,
    pub root_used: Option<u64>,
    pub root_total: Option<u64>,
    pub uptime: Option<u64>,
    /// `pve-manager/9.0.0/abc`: the release alone.
    pub boot_mode: Option<String>,
}

pub fn node_status(name: &str, online: bool, ip: Option<String>, v: &Value) -> NodeStatus {
    let kernel = v["current-kernel"]["release"].as_str().map(str::to_owned).or_else(|| s(v, "kversion"));
    NodeStatus {
        name: name.to_string(),
        online,
        ip,
        pve_version: s(v, "pveversion").map(|p| p.split('/').nth(1).unwrap_or(&p).to_string()),
        kernel,
        cpu_model: s(&v["cpuinfo"], "model"),
        cpus: u(&v["cpuinfo"], "cpus"),
        sockets: u(&v["cpuinfo"], "sockets"),
        cpu: f(v, "cpu"),
        load: v["loadavg"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_f64())).collect()).unwrap_or_default(),
        mem_used: u(&v["memory"], "used"),
        mem_total: u(&v["memory"], "total"),
        swap_used: u(&v["swap"], "used"),
        swap_total: u(&v["swap"], "total"),
        root_used: u(&v["rootfs"], "used"),
        root_total: u(&v["rootfs"], "total"),
        uptime: u(v, "uptime"),
        boot_mode: s(&v["boot-info"], "mode"),
    }
}

/// The cluster, from `/cluster/status`: its name and whether it has quorum
/// (none for a node on its own), and each node's address.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    pub name: Option<String>,
    pub quorate: Option<bool>,
    pub nodes: Vec<(String, bool, Option<String>)>,
}

pub fn cluster(v: &Value) -> Cluster {
    let mut c = Cluster::default();
    for e in v.as_array().into_iter().flatten() {
        match e["type"].as_str() {
            Some("cluster") => {
                c.name = s(e, "name");
                c.quorate = Some(flag(e, "quorate"));
            }
            Some("node") => {
                if let Some(n) = s(e, "name") {
                    c.nodes.push((n, flag(e, "online"), s(e, "ip")));
                }
            }
            _ => {}
        }
    }
    c.nodes.sort();
    c
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub product: String,
    pub version: Option<String>,
    pub release: Option<String>,
    pub cluster_name: Option<String>,
    pub quorate: Option<bool>,
    pub nodes: Vec<NodeStatus>,
}

/// A guest's snapshot.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub name: String,
    pub description: Option<String>,
    pub time: Option<u64>,
    pub parent: Option<String>,
    /// With the guest's RAM (a VM snapshot taken running).
    pub vmstate: bool,
}

pub fn snapshots(v: &Value) -> Vec<Snapshot> {
    let mut list: Vec<Snapshot> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let name = s(e, "name")?;
            // "current" is where the guest is now, not a snapshot.
            (name != "current").then(|| Snapshot {
                name,
                description: s(e, "description"),
                time: u(e, "snaptime"),
                parent: s(e, "parent"),
                vmstate: flag(e, "vmstate"),
            })
        })
        .collect();
    list.sort_by(|a, b| b.time.cmp(&a.time).then(a.name.cmp(&b.name)));
    list
}

/// What a guest's page shows: its state, its settings in short, its
/// addresses and snapshots, and which consoles it has.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GuestDetail {
    pub node: String,
    pub kind: String,
    pub vmid: u32,
    pub name: Option<String>,
    pub status: Option<String>,
    /// QEMU's own state: `running`, `paused`, `prelaunch`...
    pub qmpstatus: Option<String>,
    pub uptime: Option<u64>,
    pub cpu: Option<f64>,
    pub cpus: Option<f64>,
    pub mem: Option<u64>,
    pub maxmem: Option<u64>,
    pub disk: Option<u64>,
    pub maxdisk: Option<u64>,
    pub ostype: Option<String>,
    pub onboot: bool,
    pub lock: Option<String>,
    pub ha: Option<String>,
    pub tags: Vec<String>,
    pub description: Option<String>,
    /// The guest agent is turned on in its settings (VMs).
    pub agent: bool,
    /// The display is a SPICE one (`qxl`): the SPICE console works.
    pub spice: bool,
    /// The serial ports set up (`serial0`...), for the text console.
    pub serials: Vec<String>,
    /// Addresses from the guest agent or the container's interfaces.
    pub addresses: Vec<String>,
    pub snapshots: Vec<Snapshot>,
    pub template: bool,
}

/// Whether a VM's display (`vga`) is a SPICE one.
pub fn spice_display(vga: Option<&str>) -> bool {
    vga.and_then(|v| v.split(',').next()).is_some_and(|t| {
        let t = t.trim_start_matches("type=");
        t.starts_with("qxl")
    })
}

pub fn guest_detail(node: &str, kind: &str, vmid: u32, status: &Value, config: &Value) -> GuestDetail {
    let mut serials: Vec<String> = config
        .as_object()
        .map(|o| o.keys().filter(|k| k.starts_with("serial") && k[6..].chars().all(|c| c.is_ascii_digit()) && k.len() > 6).cloned().collect())
        .unwrap_or_default();
    serials.sort();
    let agent = config["agent"].as_str().map(|a| a.split(',').any(|p| p == "1" || p == "enabled=1")).unwrap_or_else(|| flag(config, "agent"));
    GuestDetail {
        node: node.to_string(),
        kind: kind.to_string(),
        vmid,
        name: s(status, "name").or_else(|| s(config, "name")).or_else(|| s(config, "hostname")),
        status: s(status, "status"),
        qmpstatus: s(status, "qmpstatus"),
        uptime: u(status, "uptime"),
        cpu: f(status, "cpu"),
        cpus: f(status, "cpus"),
        mem: u(status, "mem"),
        maxmem: u(status, "maxmem"),
        disk: u(status, "disk"),
        maxdisk: u(status, "maxdisk"),
        ostype: s(config, "ostype"),
        onboot: flag(config, "onboot"),
        lock: s(status, "lock").or_else(|| s(config, "lock")),
        ha: s(&status["ha"], "state").filter(|h| h != "none"),
        tags: tags(config["tags"].as_str()),
        description: s(config, "description"),
        agent: kind == "qemu" && agent,
        spice: kind == "qemu" && (spice_display(config["vga"].as_str()) || flag(status, "spice")),
        serials: if kind == "qemu" { serials } else { Vec::new() },
        addresses: Vec::new(),
        snapshots: Vec::new(),
        template: flag(config, "template") || flag(status, "template"),
    }
}

fn keep_address(ip: &str) -> bool {
    let ip = ip.split('/').next().unwrap_or(ip);
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => !a.is_loopback() && !a.is_link_local(),
        Ok(std::net::IpAddr::V6(a)) => !a.is_loopback() && (a.segments()[0] & 0xffc0) != 0xfe80,
        Err(_) => false,
    }
}

/// Addresses the QEMU guest agent reports (`network-get-interfaces`).
pub fn agent_addresses(v: &Value) -> Vec<String> {
    let list = v["result"].as_array().or_else(|| v.as_array());
    let mut out = Vec::new();
    for i in list.into_iter().flatten() {
        for a in i["ip-addresses"].as_array().into_iter().flatten() {
            if let Some(ip) = a["ip-address"].as_str() {
                if keep_address(ip) && !out.iter().any(|o| o == ip) {
                    out.push(ip.to_string());
                }
            }
        }
    }
    out
}

/// Addresses a container's interfaces have (`/lxc/{id}/interfaces`).
pub fn lxc_addresses(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for i in v.as_array().into_iter().flatten() {
        for key in ["inet", "inet6"] {
            if let Some(ip) = i[key].as_str() {
                let bare = ip.split('/').next().unwrap_or(ip);
                if keep_address(bare) && !out.iter().any(|o| o == bare) {
                    out.push(bare.to_string());
                }
            }
        }
        for a in i["ip-addresses"].as_array().into_iter().flatten() {
            if let Some(ip) = a["ip-address"].as_str() {
                if keep_address(ip) && !out.iter().any(|o| o == ip) {
                    out.push(ip.to_string());
                }
            }
        }
    }
    out
}

/// A task from `/cluster/tasks` or `/nodes/{node}/tasks`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub upid: String,
    pub node: String,
    /// `qmstart`, `vzdump`, `vncshell`...
    pub kind: String,
    /// What it worked on: a guest id, a storage...
    pub id: Option<String>,
    pub user: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    /// `OK`, an error, or `WARNINGS: n`; none while it runs.
    pub status: Option<String>,
    pub running: bool,
}

pub fn task(v: &Value) -> Option<Task> {
    let upid = s(v, "upid").filter(|u| valid_upid(u))?;
    let node = s(v, "node").or_else(|| upid.split(':').nth(1).map(str::to_owned))?;
    let end = u(v, "endtime");
    let status = s(v, "status");
    let running = end.is_none() && status.as_deref().is_none_or(|s| s == "running");
    Some(Task {
        // A Backup Server names them worker_type and worker_id.
        kind: s(v, "type").or_else(|| s(v, "worker_type")).unwrap_or_default(),
        id: s(v, "id").or_else(|| s(v, "worker_id")),
        user: s(v, "user"),
        start: u(v, "starttime"),
        end,
        status: if running { None } else { status },
        running,
        upid,
        node,
    })
}

/// Tasks newest first, those still running before all others.
pub fn tasks(v: &Value, max: usize) -> Vec<Task> {
    let mut list: Vec<Task> = v.as_array().into_iter().flatten().filter_map(task).collect();
    list.sort_by(|a, b| b.running.cmp(&a.running).then(b.start.cmp(&a.start)));
    list.truncate(max);
    list
}

/// A task's log and where it stands.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLog {
    pub lines: Vec<String>,
    pub running: bool,
    pub exitstatus: Option<String>,
    /// The line number to read on from.
    pub next: u64,
}

pub fn task_log(log: &Value, status: &Value, start: u64) -> TaskLog {
    let mut lines = Vec::new();
    let mut next = start;
    for l in log.as_array().into_iter().flatten() {
        let n = u(l, "n").unwrap_or(next + 1);
        if let Some(t) = l["t"].as_str() {
            // "no content" is what an empty log says.
            if !(t == "no content" && n == 1) {
                lines.push(t.to_string());
            }
        }
        next = next.max(n);
    }
    let running = status["status"].as_str() == Some("running");
    TaskLog { lines, running, exitstatus: if running { None } else { s(status, "exitstatus") }, next }
}

/// What a guest's SPICE console is reached with (`spiceproxy`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpiceTicket {
    pub host: String,
    pub password: String,
    pub tls_port: Option<u16>,
    pub ca: Option<String>,
    pub host_subject: Option<String>,
    pub proxy: Option<String>,
    pub title: Option<String>,
}

pub fn spice_ticket(v: &Value) -> Option<SpiceTicket> {
    Some(SpiceTicket {
        host: s(v, "host")?,
        password: s(v, "password")?,
        tls_port: u(v, "tls-port").and_then(|p| u16::try_from(p).ok()),
        // Proxmox writes the CA's line breaks as "\n".
        ca: s(v, "ca").map(|c| c.replace("\\n", "\n")),
        host_subject: s(v, "host-subject"),
        proxy: s(v, "proxy"),
        title: s(v, "title"),
    })
}

/// What `vncproxy` / `termproxy` give: the port behind the websocket and
/// its one-time ticket (and the VNC password, when one was generated).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxyTicket {
    pub port: u16,
    pub ticket: String,
    pub user: Option<String>,
    pub password: Option<String>,
}

pub fn proxy_ticket(v: &Value) -> Option<ProxyTicket> {
    Some(ProxyTicket {
        port: u(v, "port").and_then(|p| u16::try_from(p).ok())?,
        ticket: s(v, "ticket")?,
        user: s(v, "user"),
        password: s(v, "password"),
    })
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
