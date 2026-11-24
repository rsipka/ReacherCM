//! Server management sessions: a server's BMC (Dell iDRAC, HPE iLO,
//! Supermicro, Lenovo XCC, OpenBMC...) over Redfish, its standard REST API.
//! The pages show health, sensors, hardware, logs and jobs, and run the
//! power button, boot once, virtual media, identify light and BMC reset.
//!
//! The backend makes every request; the window only names resources this
//! connection found itself (see [`BmcConn::known`]), so it cannot send the
//! BMC's login anywhere else.

pub mod client;
pub mod model;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use client::{BmcError, Params, Redfish};
use model::*;

/// How many requests one page sends at once: BMCs are small computers.
const PARALLEL: usize = 6;
/// Log entries read at most per log.
const MAX_ENTRIES: usize = 500;
/// Jobs and tasks shown at most.
const MAX_JOBS: usize = 60;

/// A server management session's own settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BmcOptions {
    /// SHA-256 fingerprints (`AB:CD:..`) of certificates this session trusts
    /// although the system does not (a BMC's own is mostly self-signed).
    /// Changed only by answering the prompt; an editor's save keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certs: Vec<String>,
}

impl BmcOptions {
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }
}

/// One logged-in BMC, and where its resources are.
pub struct BmcConn {
    pub client: Redfish,
    root: Value,
    pub vendor: &'static str,
    system: String,
    manager: Option<String>,
    chassis: Option<String>,
    /// Paths found by this connection that the window may name back.
    known: std::sync::Mutex<HashSet<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub vendor: String,
    pub product: Option<String>,
    /// What to type for the serial console after an SSH login (may be empty).
    pub console_command: String,
}

fn first(v: &Value, key: &str) -> Option<String> {
    v[key].as_array().and_then(|a| a.first()).and_then(|m| m["@odata.id"].as_str()).filter(|p| client::valid_path(p)).map(str::to_owned)
}

impl BmcConn {
    /// Log in and find the server, its BMC and its chassis.
    pub async fn open(params: Params) -> Result<Self, BmcError> {
        let client = Redfish::new(&params)?;
        let root = client.login().await?;
        let systems = link(&root, "Systems").unwrap_or_else(|| "/redfish/v1/Systems".into());
        let system = client
            .members(&systems, 1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| BmcError::Other("The BMC lists no server".into()))?;
        let sys = client.get(&system).await?;
        let manager = match first(&sys["Links"], "ManagedBy") {
            Some(m) => Some(m),
            None => match link(&root, "Managers") {
                Some(c) => client.members(&c, 1).await.ok().and_then(|m| m.into_iter().next()),
                None => None,
            },
        };
        let chassis = match first(&sys["Links"], "Chassis") {
            Some(c) => Some(c),
            None => match link(&root, "Chassis") {
                Some(c) => client.members(&c, 1).await.ok().and_then(|m| m.into_iter().next()),
                None => None,
            },
        };
        Ok(Self {
            client,
            vendor: vendor(&root),
            root,
            system,
            manager,
            chassis,
            known: Default::default(),
        })
    }

    pub fn opened(&self) -> Opened {
        Opened {
            vendor: self.vendor.to_string(),
            product: s(&self.root, "Product"),
            console_command: self.text_console_command().to_string(),
        }
    }

    fn remember<'a>(&self, paths: impl IntoIterator<Item = &'a String>) {
        if let Ok(mut k) = self.known.lock() {
            k.extend(paths.into_iter().cloned());
        }
    }

    /// A path the window sent back: only one this connection found.
    fn check(&self, path: &str) -> Result<(), BmcError> {
        if self.known.lock().map(|k| k.contains(path)).unwrap_or(false) {
            Ok(())
        } else {
            Err(BmcError::Other("Unknown resource; refresh the page".into()))
        }
    }

    /// GET many resources a few at a time, in their order; failures dropped.
    async fn get_many(self: &Arc<Self>, paths: Vec<String>) -> Vec<(String, Value)> {
        let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
        let mut set = tokio::task::JoinSet::new();
        for (i, p) in paths.into_iter().enumerate() {
            let me = self.clone();
            let sem = sem.clone();
            set.spawn(async move {
                let _permit = sem.acquire().await.ok();
                let v = me.client.get(&p).await;
                (i, p, v)
            });
        }
        let mut out = Vec::new();
        while let Some(r) = set.join_next().await {
            if let Ok((i, p, Ok(v))) = r {
                out.push((i, p, v));
            }
        }
        out.sort_by_key(|(i, _, _)| *i);
        out.into_iter().map(|(_, p, v)| (p, v)).collect()
    }

    /// The members of a collection, each read whole.
    async fn expand(self: &Arc<Self>, collection: Option<String>, max: usize) -> Vec<(String, Value)> {
        let Some(c) = collection else { return Vec::new() };
        let members = self.client.members(&c, max).await.unwrap_or_default();
        self.get_many(members).await
    }

    async fn system(&self) -> Result<Value, BmcError> {
        self.client.get(&self.system).await
    }

    async fn manager(&self) -> Value {
        match &self.manager {
            Some(m) => self.client.get(m).await.unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    /// An action's allowable values for `param`: on the action itself, or
    /// in the `ActionInfo` resource it links to.
    async fn allowed(&self, resource: &Value, action: &str, param: &str) -> Vec<String> {
        let a = &resource["Actions"][action];
        let inline = allowable(a, param);
        if !inline.is_empty() {
            return inline;
        }
        match a["@Redfish.ActionInfo"].as_str().filter(|p| client::valid_path(p)) {
            Some(info) => self.client.get(info).await.map(|i| action_info_values(&i, param)).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    pub async fn overview(self: &Arc<Self>) -> Result<Overview, BmcError> {
        let (system, manager, sensors) = tokio::join!(self.system(), self.manager(), self.sensors());
        let system = system?;
        let mut o = overview(&self.root, &system, &manager);
        let from_info = self.allowed(&system, "#ComputerSystem.Reset", "ResetType").await;
        if !from_info.is_empty() {
            o.reset_types = from_info;
        }
        if let Ok(sensors) = sensors {
            o.power_watts = sensors.power_watts;
            o.inlet_temp = inlet(&sensors.temperatures);
        }
        Ok(o)
    }

    pub async fn sensors(self: &Arc<Self>) -> Result<Sensors, BmcError> {
        let Some(chassis) = &self.chassis else { return Ok(Sensors::default()) };
        let ch = self.client.get(chassis).await?;
        if let Some(thermal) = link(&ch, "Thermal") {
            let power = match link(&ch, "Power") {
                Some(p) => self.client.get(&p).await.unwrap_or(Value::Null),
                None => Value::Null,
            };
            let thermal = self.client.get(&thermal).await.unwrap_or(Value::Null);
            return Ok(sensors_classic(&thermal, &power));
        }
        let members: Vec<Value> = self.expand(link(&ch, "Sensors"), 400).await.into_iter().map(|(_, v)| v).collect();
        Ok(sensors_modern(&members))
    }

    pub async fn hardware(self: &Arc<Self>) -> Result<Hardware, BmcError> {
        let sys = self.system().await?;
        let update = link(&self.root, "UpdateService");
        let (cpus, dimms, storage, nics, fw) = tokio::join!(
            self.expand(link(&sys, "Processors"), 16),
            self.expand(link(&sys, "Memory"), 96),
            self.expand(link(&sys, "Storage"), 16),
            self.expand(link(&sys, "EthernetInterfaces"), 32),
            async {
                let Some(u) = update else { return Vec::new() };
                let u = self.client.get(&u).await.unwrap_or(Value::Null);
                self.expand(link(&u, "FirmwareInventory"), 120).await
            },
        );
        let mut hw = Hardware {
            processors: cpus.iter().filter_map(|(_, v)| processor(v)).collect(),
            memory: dimms.iter().filter_map(|(_, v)| memory(v)).collect(),
            network: nics.iter().filter_map(|(_, v)| nic(v)).collect(),
            firmware: fw.iter().filter_map(|(_, v)| firmware(v)).collect(),
            ..Default::default()
        };
        // Dell lists every firmware twice (installed and previous): keep
        // the installed ones, which come first under the same name.
        let mut seen = HashSet::new();
        hw.firmware.retain(|f| seen.insert((f.name.clone(), f.detail.clone())));
        for (_, st) in &storage {
            hw.storage.push(controller(st));
            let drives: Vec<String> = st["Drives"].as_array().map(|a| a.iter().filter_map(|d| d["@odata.id"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
            hw.drives.extend(self.get_many(drives).await.iter().filter_map(|(_, v)| drive(v)));
            hw.volumes.extend(self.expand(link(st, "Volumes"), 32).await.iter().filter_map(|(_, v)| volume(v)));
        }
        Ok(hw)
    }

    pub async fn log_services(self: &Arc<Self>) -> Result<Vec<LogService>, BmcError> {
        let sys = self.system().await?;
        let mgr = self.manager().await;
        let (a, b) = tokio::join!(self.expand(link(&sys, "LogServices"), 16), self.expand(link(&mgr, "LogServices"), 16));
        let list: Vec<LogService> = a.iter().chain(b.iter()).filter_map(|(p, v)| log_service(v, p)).collect();
        self.remember(list.iter().map(|l| &l.path));
        Ok(list)
    }

    pub async fn log_entries(&self, service: &str) -> Result<Vec<LogEntry>, BmcError> {
        self.check(service)?;
        let svc = self.client.get(service).await?;
        let entries = link(&svc, "Entries").ok_or_else(|| BmcError::Other("This log has no entries".into()))?;
        let mut list: Vec<LogEntry> = self.client.collection(&entries, MAX_ENTRIES).await?.iter().map(log_entry).collect();
        sort_entries(&mut list);
        Ok(list)
    }

    pub async fn log_clear(&self, service: &str) -> Result<(), BmcError> {
        self.check(service)?;
        let svc = self.client.get(service).await?;
        let target = action_target(&svc, "#LogService.ClearLog").ok_or_else(|| BmcError::Other("This log cannot be cleared".into()))?;
        self.client.post(&target, &json!({})).await.map(|_| ())
    }

    pub async fn jobs(self: &Arc<Self>) -> Result<Vec<Job>, BmcError> {
        let mut paths = Vec::new();
        if self.vendor == "dell" {
            if let Some(m) = &self.manager {
                let dell = format!("{m}/Oem/Dell/Jobs");
                let mut ids = self.client.members(&dell, 1000).await.unwrap_or_default();
                // The newest are listed last.
                let skip = ids.len().saturating_sub(MAX_JOBS);
                paths.extend(ids.drain(skip..));
            }
        }
        if paths.is_empty() {
            let tasks = match link(&self.root, "Tasks") {
                Some(t) => self.client.get(&t).await.ok().and_then(|t| link(&t, "Tasks")),
                None => None,
            };
            if let Some(t) = tasks {
                let mut ids = self.client.members(&t, 1000).await.unwrap_or_default();
                let skip = ids.len().saturating_sub(MAX_JOBS);
                paths.extend(ids.drain(skip..));
            }
        }
        let mut jobs: Vec<Job> = self.get_many(paths).await.iter().map(|(_, v)| job(v)).collect();
        jobs.reverse();
        Ok(jobs)
    }

    pub async fn virtual_media(self: &Arc<Self>) -> Result<Vec<VirtualMedia>, BmcError> {
        // iDRAC 9 from 6.x keeps it under the system, others under the BMC.
        let sys = self.system().await?;
        let mgr = self.manager().await;
        let (a, b) = tokio::join!(self.expand(link(&sys, "VirtualMedia"), 8), self.expand(link(&mgr, "VirtualMedia"), 8));
        let mut seen = HashSet::new();
        let list: Vec<VirtualMedia> = a
            .iter()
            .chain(b.iter())
            .map(|(p, v)| virtual_media(v, p))
            .filter(|m| seen.insert(m.path.clone()))
            .collect();
        self.remember(list.iter().map(|m| &m.path));
        Ok(list)
    }

    /// The power button: `On`, `GracefulShutdown`, `ForceOff`, `ForceRestart`...
    pub async fn power(&self, reset_type: &str) -> Result<Option<String>, BmcError> {
        let sys = self.system().await?;
        let target = action_target(&sys, "#ComputerSystem.Reset").ok_or_else(|| BmcError::Other("This server has no power control".into()))?;
        let allowed = self.allowed(&sys, "#ComputerSystem.Reset", "ResetType").await;
        let known = ["On", "ForceOff", "GracefulShutdown", "GracefulRestart", "ForceRestart", "Nmi", "ForceOn", "PushPowerButton", "PowerCycle"];
        if !(allowed.iter().any(|a| a == reset_type) || (allowed.is_empty() && known.contains(&reset_type))) {
            return Err(BmcError::Other(format!("The server does not offer {reset_type}")));
        }
        self.client.post(&target, &json!({ "ResetType": reset_type })).await
    }

    /// Boot once from `target` (`Pxe`, `Cd`, `BiosSetup`...), or `None` to cancel.
    pub async fn boot_once(&self, target: &str) -> Result<(), BmcError> {
        let (sys, tag) = self.client.get_tagged(&self.system).await?;
        let allowed = allowable(&sys["Boot"], "BootSourceOverrideTarget");
        if target != "None" && !allowed.is_empty() && !allowed.iter().any(|a| a == target) {
            return Err(BmcError::Other(format!("The server cannot boot from {target}")));
        }
        if !target.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(BmcError::Other("Not a boot target".into()));
        }
        let body = if target == "None" {
            json!({ "Boot": { "BootSourceOverrideEnabled": "Disabled" } })
        } else {
            json!({ "Boot": { "BootSourceOverrideTarget": target, "BootSourceOverrideEnabled": "Once" } })
        };
        self.client.patch(&self.system, &body, tag.as_deref()).await
    }

    /// Turn the identify light on or off.
    pub async fn identify(&self, on: bool) -> Result<(), BmcError> {
        let (sys, tag) = self.client.get_tagged(&self.system).await?;
        let body = if sys["LocationIndicatorActive"].is_boolean() {
            json!({ "LocationIndicatorActive": on })
        } else if sys.get("IndicatorLED").is_some() {
            let allowed = allowable(&sys, "IndicatorLED");
            let lit = if !on { "Off" } else if allowed.iter().any(|a| a == "Blinking") && self.vendor == "dell" { "Blinking" } else { "Lit" };
            json!({ "IndicatorLED": lit })
        } else {
            return Err(BmcError::Other("This server has no identify light".into()));
        };
        self.client.patch(&self.system, &body, tag.as_deref()).await
    }

    pub async fn media_insert(&self, path: &str, image: &str) -> Result<(), BmcError> {
        self.check(path)?;
        if !valid_image(image) {
            return Err(BmcError::Other("The image must be an http(s), NFS or CIFS address".into()));
        }
        let (vm, tag) = self.client.get_tagged(path).await?;
        match action_target(&vm, "#VirtualMedia.InsertMedia") {
            Some(t) => self.client.post(&t, &json!({ "Image": image.trim(), "Inserted": true, "WriteProtected": true })).await.map(|_| ()),
            None => self.client.patch(path, &json!({ "Image": image.trim() }), tag.as_deref()).await,
        }
    }

    pub async fn media_eject(&self, path: &str) -> Result<(), BmcError> {
        self.check(path)?;
        let (vm, tag) = self.client.get_tagged(path).await?;
        match action_target(&vm, "#VirtualMedia.EjectMedia") {
            Some(t) => self.client.post(&t, &json!({})).await.map(|_| ()),
            None => self.client.patch(path, &json!({ "Image": null }), tag.as_deref()).await,
        }
    }

    /// Restart the BMC itself (not the server).
    pub async fn reset_bmc(&self) -> Result<(), BmcError> {
        let m = self.manager.as_ref().ok_or_else(|| BmcError::Other("No BMC found".into()))?;
        let mgr = self.client.get(m).await?;
        let target = action_target(&mgr, "#Manager.Reset").ok_or_else(|| BmcError::Other("The BMC cannot be restarted from here".into()))?;
        let allowed = self.allowed(&mgr, "#Manager.Reset", "ResetType").await;
        let body = if allowed.iter().any(|a| a == "GracefulRestart") {
            json!({ "ResetType": "GracefulRestart" })
        } else if let Some(a) = allowed.first() {
            json!({ "ResetType": a })
        } else if self.vendor == "dell" {
            json!({ "ResetType": "GracefulRestart" })
        } else {
            json!({})
        };
        self.client.post(&target, &body).await.map(|_| ())
    }
}

/// How the controller's own console page opens.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleLaunch {
    /// Path and query on the controller's web server.
    pub path: String,
    /// The sign-in preset for its login form (`WebOptions::login`): `off`
    /// when the address signs in by itself, `None` for any login form.
    pub login: Option<&'static str>,
}

impl BmcConn {
    /// Where the controller's remote console opens. An iDRAC 9 hands out a
    /// one-time login for its HTML5 console; the others open their web
    /// interface, signed in, with the console a click away.
    pub async fn console(&self) -> Result<ConsoleLaunch, BmcError> {
        match self.vendor {
            "dell" => match self.dell_kvm().await {
                Ok(path) => Ok(ConsoleLaunch { path, login: Some("off") }),
                // No console without the licence: said as it is.
                Err(e @ BmcError::Licence(_)) => Err(e),
                Err(e) => {
                    tracing::info!("iDRAC one-time console login not offered ({e}); opening its web interface");
                    Ok(ConsoleLaunch { path: "/".into(), login: Some("idrac") })
                }
            },
            "hpe" => Ok(ConsoleLaunch { path: "/".into(), login: Some("ilo") }),
            _ => Ok(ConsoleLaunch { path: "/".into(), login: None }),
        }
    }

    async fn dell_kvm(&self) -> Result<String, BmcError> {
        let m = self.manager.as_ref().ok_or_else(|| BmcError::Other("No BMC found".into()))?;
        let service = format!("{m}/Oem/Dell/DelliDRACCardService");
        let actions = format!("{service}/Actions/DelliDRACCardService");
        // Dell's own scripts export the certificate first; some firmware
        // wants it done before a console session is given.
        let _ = self.client.post_for(&format!("{actions}.ExportSSLCertificate"), &json!({ "SSLCertType": "Server" })).await;
        let v = self.client.post_for(&format!("{actions}.GetKVMSession"), &json!({ "SessionTypeName": "idrac_cert_file.txt" })).await?;
        let (Some(user), Some(pass)) = (s(&v, "TempUsername"), s(&v, "TempPassword")) else {
            return Err(BmcError::Other("The iDRAC gave no console login".into()));
        };
        Ok(model::dell_console_path(self.client.username(), &user, &pass))
    }

    /// What to type for the serial console after an SSH login to it.
    pub fn text_console_command(&self) -> &'static str {
        model::text_console_command(self.vendor)
    }
}

/// The open server management sessions, by connection id.
#[derive(Default)]
pub struct BmcManager {
    conns: std::sync::Mutex<HashMap<String, Arc<BmcConn>>>,
}

impl BmcManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<BmcConn>> {
        self.conns.lock().ok()?.get(id).cloned()
    }

    pub fn insert(&self, id: String, conn: BmcConn) {
        if let Ok(mut m) = self.conns.lock() {
            m.insert(id, Arc::new(conn));
        }
    }

    pub fn remove(&self, id: &str) -> Option<Arc<BmcConn>> {
        self.conns.lock().ok()?.remove(id)
    }

    /// Every open one, for logging them all out when the app quits.
    pub fn drain(&self) -> Vec<Arc<BmcConn>> {
        self.conns.lock().map(|mut m| m.drain().map(|(_, c)| c).collect()).unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
