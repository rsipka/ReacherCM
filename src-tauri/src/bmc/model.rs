//! What the server management pages show, read out of Redfish resources.
//! Only plain functions over JSON here, so they are tested on sample
//! resources from Dell, HPE and the DMTF mockups without a BMC.

use serde::Serialize;
use serde_json::Value;

pub fn s(v: &Value, key: &str) -> Option<String> {
    match &v[key] {
        Value::String(x) if !x.trim().is_empty() => Some(x.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn f(v: &Value, key: &str) -> Option<f64> {
    v[key].as_f64()
}

/// `Actions["#Name"].target`, checked to stay on the service.
pub fn action_target(v: &Value, name: &str) -> Option<String> {
    v["Actions"][name]["target"].as_str().filter(|t| super::client::valid_path(t)).map(str::to_owned)
}

/// `Key@Redfish.AllowableValues` of an action or a property's parent.
pub fn allowable(v: &Value, key: &str) -> Vec<String> {
    v[format!("{key}@Redfish.AllowableValues")]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

pub fn link(v: &Value, key: &str) -> Option<String> {
    v[key]["@odata.id"].as_str().filter(|t| super::client::valid_path(t)).map(str::to_owned)
}

/// A health: `OK`, `Warning` or `Critical`; none for a part that is absent
/// or turned off.
fn health(v: &Value) -> Option<String> {
    s(&v["Status"], "Health")
}

fn state(v: &Value) -> Option<String> {
    s(&v["Status"], "State")
}

fn absent(v: &Value) -> bool {
    matches!(state(v).as_deref(), Some("Absent"))
}

/// Who made the BMC, from the service root.
pub fn vendor(root: &Value) -> &'static str {
    let name = s(root, "Vendor").unwrap_or_default().to_ascii_lowercase();
    let oem = root["Oem"].as_object();
    let has = |k: &str| oem.is_some_and(|o| o.contains_key(k));
    if name.contains("dell") || has("Dell") {
        "dell"
    } else if name.starts_with("hp") || has("Hpe") || has("Hp") {
        "hpe"
    } else if name.contains("supermicro") || has("Supermicro") {
        "supermicro"
    } else if name.contains("lenovo") || name.contains("ibm") || has("Lenovo") {
        "lenovo"
    } else if name.contains("fujitsu") || has("ts_fujitsu") || has("Fujitsu") {
        "fujitsu"
    } else if name.contains("cisco") || has("Cisco") {
        "cisco"
    } else if name.contains("huawei") || has("Huawei") {
        "huawei"
    } else if name.contains("openbmc") || has("OpenBmc") || has("OpenBMC") {
        "openbmc"
    } else if name.contains("asrock") || has("ASRockRack") {
        "asrock"
    } else if name == "ami" || name.contains("american megatrends") || name.contains("megarac") || has("Ami") || has("AMI") {
        // ASRock Rack, Gigabyte, Tyan and many other boards run AMI MegaRAC.
        "ami"
    } else {
        "other"
    }
}

/// The allowable values of an action's parameter from its `ActionInfo`
/// resource, which some BMCs (Supermicro, AMI MegaRAC, Lenovo XCC) use
/// instead of listing them on the action itself.
pub fn action_info_values(info: &Value, param: &str) -> Vec<String> {
    info["Parameters"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["Name"] == param))
        .and_then(|p| p["AllowableValues"].as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub vendor: String,
    pub product: Option<String>,
    pub redfish_version: Option<String>,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    /// Dell's service tag, HPE's product number.
    pub sku: Option<String>,
    pub bios_version: Option<String>,
    pub host_name: Option<String>,
    pub power_state: Option<String>,
    pub health: Option<String>,
    pub health_rollup: Option<String>,
    pub cpu_count: Option<f64>,
    pub cpu_model: Option<String>,
    pub memory_gib: Option<f64>,
    /// Identify light: on or not; none where the BMC has no such light.
    pub identify: Option<bool>,
    pub bmc_model: Option<String>,
    pub bmc_firmware: Option<String>,
    pub bmc_time: Option<String>,
    pub power_watts: Option<f64>,
    pub inlet_temp: Option<f64>,
    /// What the power button offers (`ComputerSystem.Reset`).
    pub reset_types: Vec<String>,
    /// Where it can boot once.
    pub boot_targets: Vec<String>,
    pub boot_target: Option<String>,
    pub boot_enabled: Option<String>,
    pub can_reset_bmc: bool,
}

pub fn overview(root: &Value, system: &Value, manager: &Value) -> Overview {
    let identify = match &system["LocationIndicatorActive"] {
        Value::Bool(b) => Some(*b),
        _ => s(system, "IndicatorLED").map(|l| l != "Off"),
    };
    let mut reset_types = allowable(&system["Actions"]["#ComputerSystem.Reset"], "ResetType");
    if reset_types.is_empty() && action_target(system, "#ComputerSystem.Reset").is_some() {
        reset_types = ["On", "GracefulShutdown", "ForceOff", "GracefulRestart", "ForceRestart"].map(String::from).to_vec();
    }
    let boot = &system["Boot"];
    Overview {
        vendor: vendor(root).to_string(),
        product: s(root, "Product"),
        redfish_version: s(root, "RedfishVersion"),
        model: s(system, "Model"),
        manufacturer: s(system, "Manufacturer"),
        serial: s(system, "SerialNumber"),
        sku: s(system, "SKU"),
        bios_version: s(system, "BiosVersion"),
        host_name: s(system, "HostName"),
        power_state: s(system, "PowerState"),
        health: health(system),
        health_rollup: s(&system["Status"], "HealthRollup").or_else(|| health(system)),
        cpu_count: f(&system["ProcessorSummary"], "Count"),
        cpu_model: s(&system["ProcessorSummary"], "Model"),
        memory_gib: f(&system["MemorySummary"], "TotalSystemMemoryGiB"),
        identify,
        bmc_model: s(manager, "Model").or_else(|| s(manager, "Name")),
        bmc_firmware: s(manager, "FirmwareVersion"),
        bmc_time: s(manager, "DateTime"),
        power_watts: None,
        inlet_temp: None,
        reset_types,
        boot_targets: allowable(boot, "BootSourceOverrideTarget"),
        boot_target: s(boot, "BootSourceOverrideTarget"),
        boot_enabled: s(boot, "BootSourceOverrideEnabled"),
        can_reset_bmc: action_target(manager, "#Manager.Reset").is_some(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    pub name: String,
    pub reading: Option<f64>,
    /// `°C`, `RPM`, `%`, `V`, `W`.
    pub units: String,
    pub health: Option<String>,
    pub state: Option<String>,
    pub warning: Option<f64>,
    pub critical: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sensors {
    pub temperatures: Vec<Sensor>,
    pub fans: Vec<Sensor>,
    pub voltages: Vec<Sensor>,
    pub power_supplies: Vec<Item>,
    pub power_watts: Option<f64>,
}

fn name_of(v: &Value, fallback: &str) -> String {
    s(v, "Name").or_else(|| s(v, "MemberId")).or_else(|| s(v, "Id")).unwrap_or_else(|| fallback.to_string())
}

/// The old `Thermal` and `Power` resources, which every iDRAC and iLO has.
pub fn sensors_classic(thermal: &Value, power: &Value) -> Sensors {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let temperatures = list(&thermal["Temperatures"])
        .iter()
        .filter(|t| !absent(t) && (t["ReadingCelsius"].is_number() || health(t).is_some()))
        .map(|t| Sensor {
            name: name_of(t, "Temperature"),
            reading: f(t, "ReadingCelsius"),
            units: "°C".into(),
            health: health(t),
            state: state(t),
            warning: f(t, "UpperThresholdNonCritical"),
            critical: f(t, "UpperThresholdCritical"),
        })
        .collect();
    let fans = list(&thermal["Fans"])
        .iter()
        .filter(|t| !absent(t))
        .map(|t| {
            let units = match s(t, "ReadingUnits").as_deref() {
                Some("Percent") => "%",
                _ => "RPM",
            };
            Sensor {
                name: s(t, "FanName").unwrap_or_else(|| name_of(t, "Fan")),
                reading: f(t, "Reading"),
                units: units.into(),
                health: health(t),
                state: state(t),
                warning: None,
                critical: f(t, "LowerThresholdCritical"),
            }
        })
        .collect();
    let voltages = list(&power["Voltages"])
        .iter()
        .filter(|t| !absent(t) && t["ReadingVolts"].is_number())
        .map(|t| Sensor {
            name: name_of(t, "Voltage"),
            reading: f(t, "ReadingVolts"),
            units: "V".into(),
            health: health(t),
            state: state(t),
            warning: f(t, "UpperThresholdNonCritical"),
            critical: f(t, "UpperThresholdCritical"),
        })
        .collect();
    let power_supplies = list(&power["PowerSupplies"])
        .iter()
        .filter(|p| !absent(p))
        .map(|p| {
            let mut detail = Vec::new();
            if let Some(m) = s(p, "Model") {
                detail.push(m);
            }
            if let Some(w) = f(p, "PowerCapacityWatts") {
                detail.push(format!("{w:.0} W"));
            }
            if let Some(w) = f(p, "LastPowerOutputWatts").or_else(|| f(p, "PowerOutputWatts")) {
                detail.push(format!("out {w:.0} W"));
            }
            if let Some(v) = f(p, "LineInputVoltage") {
                detail.push(format!("in {v:.0} V"));
            }
            if let Some(fw) = s(p, "FirmwareVersion") {
                detail.push(format!("FW {fw}"));
            }
            Item { name: name_of(p, "Power supply"), detail: detail.join(" · "), health: health(p), state: state(p) }
        })
        .collect();
    let power_watts = power["PowerControl"].as_array().and_then(|a| a.first()).and_then(|c| f(c, "PowerConsumedWatts"));
    Sensors { temperatures, fans, voltages, power_supplies, power_watts }
}

/// Members of the newer `Sensors` collection, for a BMC without `Thermal`.
pub fn sensors_modern(members: &[Value]) -> Sensors {
    let mut out = Sensors::default();
    for m in members {
        if absent(m) {
            continue;
        }
        let threshold = |k: &str| m["Thresholds"][k]["Reading"].as_f64();
        let sensor = |units: &str| Sensor {
            name: name_of(m, "Sensor"),
            reading: f(m, "Reading"),
            units: units.into(),
            health: health(m),
            state: state(m),
            warning: threshold("UpperCaution"),
            critical: threshold("UpperCritical"),
        };
        match s(m, "ReadingType").as_deref() {
            Some("Temperature") => out.temperatures.push(sensor("°C")),
            Some("Rotational") => out.fans.push(sensor("RPM")),
            Some("Percent") if m["PhysicalContext"] == "Fan" => out.fans.push(sensor("%")),
            Some("Voltage") => out.voltages.push(sensor("V")),
            Some("Power") if out.power_watts.is_none() && m["PhysicalContext"] == "Chassis" => out.power_watts = f(m, "Reading"),
            _ => {}
        }
    }
    out
}

/// The inlet (front) temperature, for the overview.
pub fn inlet(temps: &[Sensor]) -> Option<f64> {
    temps
        .iter()
        .find(|t| {
            let n = t.name.to_ascii_lowercase();
            n.contains("inlet") || n.contains("ambient")
        })
        .and_then(|t| t.reading)
}

/// One line of the hardware page.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    pub detail: String,
    pub health: Option<String>,
    pub state: Option<String>,
}

fn join(parts: Vec<Option<String>>) -> String {
    parts.into_iter().flatten().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
}

pub fn processor(v: &Value) -> Option<Item> {
    if absent(v) {
        return None;
    }
    Some(Item {
        name: s(v, "Socket").or_else(|| s(v, "Id")).unwrap_or_else(|| name_of(v, "CPU")),
        detail: join(vec![
            s(v, "Model"),
            f(v, "TotalCores").map(|c| format!("{c:.0} cores")),
            f(v, "TotalThreads").map(|c| format!("{c:.0} threads")),
            f(v, "MaxSpeedMHz").map(|m| format!("{:.1} GHz", m / 1000.0)),
        ]),
        health: health(v),
        state: state(v),
    })
}

pub fn memory(v: &Value) -> Option<Item> {
    if absent(v) || v["CapacityMiB"].as_f64().is_some_and(|c| c == 0.0) {
        return None;
    }
    Some(Item {
        name: s(v, "DeviceLocator").or_else(|| s(&v["MemoryLocation"], "Slot").map(|n| format!("Slot {n}"))).unwrap_or_else(|| name_of(v, "DIMM")),
        detail: join(vec![
            f(v, "CapacityMiB").map(|m| format!("{:.0} GB", m / 1024.0)),
            s(v, "MemoryDeviceType"),
            f(v, "OperatingSpeedMhz").map(|m| format!("{m:.0} MT/s")),
            s(v, "Manufacturer"),
            s(v, "PartNumber"),
        ]),
        health: health(v),
        state: state(v),
    })
}

fn bytes(n: f64) -> String {
    const TB: f64 = 1e12;
    const GB: f64 = 1e9;
    if n >= TB { format!("{:.2} TB", n / TB) } else { format!("{:.0} GB", n / GB) }
}

pub fn drive(v: &Value) -> Option<Item> {
    if absent(v) {
        return None;
    }
    let failing = v["FailurePredicted"].as_bool() == Some(true);
    Some(Item {
        name: name_of(v, "Drive"),
        detail: join(vec![
            f(v, "CapacityBytes").map(bytes),
            s(v, "MediaType"),
            s(v, "Protocol"),
            s(v, "Model"),
            f(v, "PredictedMediaLifeLeftPercent").map(|p| format!("{p:.0}% life left")),
            failing.then(|| "failure predicted".to_string()),
        ]),
        health: if failing && health(v).as_deref() == Some("OK") { Some("Warning".into()) } else { health(v) },
        state: state(v),
    })
}

pub fn volume(v: &Value) -> Option<Item> {
    Some(Item {
        name: name_of(v, "Volume"),
        detail: join(vec![f(v, "CapacityBytes").map(bytes), s(v, "RAIDType").or_else(|| s(v, "VolumeType"))]),
        health: health(v),
        state: state(v),
    })
}

pub fn controller(v: &Value) -> Item {
    // A Storage resource, with its controllers inside it (older schema) or not.
    let c = v["StorageControllers"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
    Item {
        name: name_of(v, "Storage"),
        detail: join(vec![s(&c, "Model"), s(&c, "FirmwareVersion").map(|f| format!("FW {f}"))]),
        health: health(v).or_else(|| health(&c)),
        state: state(v).or_else(|| state(&c)),
    }
}

pub fn nic(v: &Value) -> Option<Item> {
    if absent(v) {
        return None;
    }
    let ips: Vec<String> = v["IPv4Addresses"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| s(i, "Address")).filter(|a| a != "0.0.0.0").collect())
        .unwrap_or_default();
    Some(Item {
        name: name_of(v, "NIC"),
        detail: join(vec![
            s(v, "MACAddress").or_else(|| s(v, "PermanentMACAddress")),
            f(v, "SpeedMbps").filter(|s| *s > 0.0).map(|m| if m >= 1000.0 { format!("{:.0} Gb/s", m / 1000.0) } else { format!("{m:.0} Mb/s") }),
            s(v, "LinkStatus"),
            (!ips.is_empty()).then(|| ips.join(", ")),
        ]),
        health: health(v),
        state: state(v),
    })
}

pub fn firmware(v: &Value) -> Option<Item> {
    let version = s(v, "Version")?;
    Some(Item { name: name_of(v, "Firmware"), detail: version, health: health(v), state: state(v) })
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    pub processors: Vec<Item>,
    pub memory: Vec<Item>,
    pub storage: Vec<Item>,
    pub drives: Vec<Item>,
    pub volumes: Vec<Item>,
    pub network: Vec<Item>,
    pub firmware: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogService {
    pub path: String,
    pub name: String,
    pub can_clear: bool,
}

pub fn log_service(v: &Value, path: &str) -> Option<LogService> {
    link(v, "Entries")?;
    Some(LogService { path: path.to_string(), name: name_of(v, "Log"), can_clear: action_target(v, "#LogService.ClearLog").is_some() })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: String,
    pub created: Option<String>,
    /// `OK`, `Warning` or `Critical`.
    pub severity: Option<String>,
    pub message: String,
    pub message_id: Option<String>,
    pub sensor: Option<String>,
}

pub fn log_entry(v: &Value) -> LogEntry {
    let message = s(v, "Message")
        .or_else(|| s(v, "Description"))
        .unwrap_or_default();
    LogEntry {
        id: s(v, "Id").unwrap_or_default(),
        created: s(v, "Created"),
        severity: s(v, "Severity").or_else(|| s(v, "MessageSeverity")),
        message,
        message_id: s(v, "MessageId"),
        sensor: s(v, "SensorType"),
    }
}

/// Newest first, by time where both have one.
pub fn sort_entries(entries: &mut [LogEntry]) {
    let key = |e: &LogEntry| e.created.as_deref().and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok()).map(|d| d.timestamp());
    entries.sort_by_key(|e| std::cmp::Reverse(key(e)));
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub name: String,
    /// `Running`, `Completed`, `Scheduled`, `Failed`...
    pub state: Option<String>,
    pub percent: Option<f64>,
    pub message: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
}

/// A Dell job or a Redfish task.
pub fn job(v: &Value) -> Job {
    let message = s(v, "Message").or_else(|| {
        v["Messages"].as_array().and_then(|m| m.last()).and_then(|m| s(m, "Message"))
    });
    Job {
        id: s(v, "Id").unwrap_or_default(),
        name: s(v, "Name").unwrap_or_else(|| s(v, "JobType").unwrap_or_default()),
        state: s(v, "JobState").or_else(|| s(v, "TaskState")),
        percent: f(v, "PercentComplete"),
        message,
        start: s(v, "StartTime").filter(|t| t != "TIME_NA"),
        end: s(v, "CompletionTime").or_else(|| s(v, "EndTime")).filter(|t| t != "TIME_NA"),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VirtualMedia {
    pub path: String,
    pub name: String,
    pub media_types: Vec<String>,
    pub image: Option<String>,
    pub inserted: bool,
}

pub fn virtual_media(v: &Value, path: &str) -> VirtualMedia {
    VirtualMedia {
        path: path.to_string(),
        name: name_of(v, "Virtual media"),
        media_types: v["MediaTypes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        image: s(v, "Image").or_else(|| s(v, "ImageName")),
        inserted: v["Inserted"].as_bool().unwrap_or(false),
    }
}

/// An image address the BMC can fetch itself.
pub fn valid_image(url: &str) -> bool {
    let u = url.trim();
    let scheme_ok = ["http://", "https://", "nfs://", "cifs://", "smb://"].iter().any(|p| u.to_ascii_lowercase().starts_with(p));
    scheme_ok && u.len() < 2048 && !url.chars().any(|c| c.is_control())
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;

/// The serial console command typed after an SSH login to the controller,
/// by vendor; empty where its shell is the console already or none is known.
pub fn text_console_command(vendor: &str) -> &'static str {
    match vendor {
        "dell" => "console com2",
        "hpe" => "vsp",
        "lenovo" => "console 1",
        "supermicro" | "asrock" | "ami" => "start /system1/sol1",
        "cisco" => "connect host",
        _ => "",
    }
}
