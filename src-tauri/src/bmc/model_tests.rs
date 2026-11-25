//! Samples shaped like what an iDRAC 9, an iLO 5 and the DMTF mockups send
//! (trimmed, with made-up serial numbers and addresses).

use super::*;
use serde_json::json;

fn dell_root() -> Value {
    json!({"@odata.id": "/redfish/v1", "RedfishVersion": "1.17.0", "Vendor": "Dell", "Product": "Integrated Dell Remote Access Controller",
        "Systems": {"@odata.id": "/redfish/v1/Systems"}, "Oem": {"Dell": {}}})
}

fn dell_system() -> Value {
    json!({
        "@odata.id": "/redfish/v1/Systems/System.Embedded.1",
        "Model": "PowerEdge R640", "Manufacturer": "Dell Inc.", "SerialNumber": "CN0000000000000", "SKU": "ABC1234",
        "BiosVersion": "2.19.1", "HostName": "server1.example.com", "PowerState": "On",
        "Status": {"Health": "OK", "HealthRollup": "Warning", "State": "Enabled"},
        "ProcessorSummary": {"Count": 2, "Model": "Intel(R) Xeon(R) Gold 6130 CPU @ 2.10GHz"},
        "MemorySummary": {"TotalSystemMemoryGiB": 192},
        "IndicatorLED": "Off", "LocationIndicatorActive": false,
        "Boot": {"BootSourceOverrideEnabled": "Disabled", "BootSourceOverrideTarget": "None",
            "BootSourceOverrideTarget@Redfish.AllowableValues": ["None", "Pxe", "Floppy", "Cd", "Hdd", "BiosSetup", "Utilities", "UefiTarget", "SDCard", "UefiHttp"]},
        "Actions": {"#ComputerSystem.Reset": {"target": "/redfish/v1/Systems/System.Embedded.1/Actions/ComputerSystem.Reset",
            "ResetType@Redfish.AllowableValues": ["On", "ForceOff", "ForceRestart", "GracefulRestart", "GracefulShutdown", "PushPowerButton", "Nmi", "PowerCycle"]}}
    })
}

#[test]
fn dell_overview() {
    let mgr = json!({"Model": "14G Monolithic", "FirmwareVersion": "7.00.00.172", "DateTime": "2026-11-24T09:00:00+01:00",
        "Actions": {"#Manager.Reset": {"target": "/redfish/v1/Managers/iDRAC.Embedded.1/Actions/Manager.Reset", "ResetType@Redfish.AllowableValues": ["GracefulRestart"]}}});
    let o = overview(&dell_root(), &dell_system(), &mgr);
    assert_eq!(o.vendor, "dell");
    assert_eq!(o.model.as_deref(), Some("PowerEdge R640"));
    assert_eq!(o.sku.as_deref(), Some("ABC1234"));
    assert_eq!(o.health_rollup.as_deref(), Some("Warning"));
    assert_eq!(o.cpu_count, Some(2.0));
    assert_eq!(o.memory_gib, Some(192.0));
    assert_eq!(o.identify, Some(false));
    assert!(o.reset_types.contains(&"GracefulShutdown".to_string()));
    assert!(o.boot_targets.contains(&"Pxe".to_string()));
    assert_eq!(o.bmc_firmware.as_deref(), Some("7.00.00.172"));
    assert!(o.can_reset_bmc);
}

#[test]
fn hpe_overview_without_allowable_values() {
    let root = json!({"RedfishVersion": "1.6.0", "Vendor": "HPE", "Product": "ProLiant DL360 Gen10", "Oem": {"Hpe": {}}});
    let sys = json!({"Model": "ProLiant DL360 Gen10", "PowerState": "Off", "IndicatorLED": "Lit",
        "Status": {"Health": "OK", "HealthRollup": "OK"},
        "Actions": {"#ComputerSystem.Reset": {"target": "/redfish/v1/Systems/1/Actions/ComputerSystem.Reset"}}});
    let o = overview(&root, &sys, &Value::Null);
    assert_eq!(o.vendor, "hpe");
    assert_eq!(o.identify, Some(true));
    assert_eq!(o.reset_types.len(), 5);
    assert!(!o.can_reset_bmc);
    // iLO 4's root says "Hp".
    assert_eq!(vendor(&json!({"Oem": {"Hp": {}}})), "hpe");
    assert_eq!(vendor(&json!({"Vendor": "Contoso"})), "other");
}

#[test]
fn more_vendors_and_action_info() {
    assert_eq!(vendor(&json!({"Vendor": "Lenovo", "Oem": {"Lenovo": {}}})), "lenovo");
    assert_eq!(vendor(&json!({"Vendor": "Supermicro"})), "supermicro");
    assert_eq!(vendor(&json!({"Vendor": "AMI"})), "ami");
    assert_eq!(vendor(&json!({"Oem": {"Ami": {}}})), "ami");
    assert_eq!(vendor(&json!({"Vendor": "Fujitsu", "Oem": {"ts_fujitsu": {}}})), "fujitsu");
    assert_eq!(vendor(&json!({"Vendor": "OpenBMC"})), "openbmc");
    let info = json!({"@odata.id": "/redfish/v1/Systems/1/ResetActionInfo",
        "Parameters": [{"Name": "ResetType", "Required": true, "DataType": "String", "AllowableValues": ["On", "ForceOff", "GracefulShutdown", "GracefulRestart", "ForceRestart", "Nmi", "ForceOn"]}]});
    assert_eq!(action_info_values(&info, "ResetType").len(), 7);
    assert!(action_info_values(&info, "Other").is_empty());
}

#[test]
fn classic_sensors() {
    let thermal = json!({
        "Temperatures": [
            {"Name": "System Board Inlet Temp", "ReadingCelsius": 22, "UpperThresholdNonCritical": 38, "UpperThresholdCritical": 42, "Status": {"Health": "OK", "State": "Enabled"}},
            {"Name": "CPU2 Temp", "ReadingCelsius": null, "Status": {"State": "Absent"}}
        ],
        "Fans": [
            {"Name": "System Board Fan1A", "Reading": 7080, "ReadingUnits": "RPM", "LowerThresholdCritical": 600, "Status": {"Health": "OK", "State": "Enabled"}},
            {"FanName": "Fan 1", "Reading": 23, "ReadingUnits": "Percent", "Status": {"Health": "OK", "State": "Enabled"}}
        ]
    });
    let power = json!({
        "PowerControl": [{"PowerConsumedWatts": 182}],
        "PowerSupplies": [
            {"Name": "PS1 Status", "Model": "PWR SPLY,750W,RDNT,DELTA", "PowerCapacityWatts": 750, "LineInputVoltage": 230, "FirmwareVersion": "00.1B.53", "Status": {"Health": "OK", "State": "Enabled"}},
            {"Name": "PS2 Status", "Status": {"Health": "Critical", "State": "Enabled"}}
        ],
        "Voltages": [{"Name": "CPU1 VCORE PG", "ReadingVolts": 1.0, "Status": {"Health": "OK", "State": "Enabled"}}]
    });
    let s = sensors_classic(&thermal, &power);
    assert_eq!(s.temperatures.len(), 1);
    assert_eq!(inlet(&s.temperatures), Some(22.0));
    assert_eq!(s.temperatures[0].critical, Some(42.0));
    assert_eq!(s.fans.len(), 2);
    assert_eq!(s.fans[1].name, "Fan 1");
    assert_eq!(s.fans[1].units, "%");
    assert_eq!(s.power_watts, Some(182.0));
    assert_eq!(s.power_supplies[0].detail, "PWR SPLY,750W,RDNT,DELTA · 750 W · in 230 V · FW 00.1B.53");
    assert_eq!(s.power_supplies[1].health.as_deref(), Some("Critical"));
    assert_eq!(s.voltages.len(), 1);
}

#[test]
fn modern_sensors() {
    let members = vec![
        json!({"Name": "Inlet", "ReadingType": "Temperature", "Reading": 24.5, "Thresholds": {"UpperCritical": {"Reading": 45}}, "Status": {"Health": "OK"}}),
        json!({"Name": "Fan 3", "ReadingType": "Rotational", "Reading": 5000}),
        json!({"Name": "Total power", "ReadingType": "Power", "PhysicalContext": "Chassis", "Reading": 300}),
    ];
    let s = sensors_modern(&members);
    assert_eq!(s.temperatures[0].critical, Some(45.0));
    assert_eq!(s.fans.len(), 1);
    assert_eq!(s.power_watts, Some(300.0));
}

#[test]
fn hardware_items() {
    let cpu = json!({"Socket": "CPU.Socket.1", "Model": "Intel Xeon Gold 6130", "TotalCores": 16, "TotalThreads": 32, "MaxSpeedMHz": 4000, "Status": {"Health": "OK", "State": "Enabled"}});
    assert_eq!(processor(&cpu).unwrap().detail, "Intel Xeon Gold 6130 · 16 cores · 32 threads · 4.0 GHz");
    assert!(processor(&json!({"Status": {"State": "Absent"}})).is_none());

    let dimm = json!({"DeviceLocator": "DIMM.Socket.A1", "CapacityMiB": 32768, "MemoryDeviceType": "DDR4", "OperatingSpeedMhz": 2666, "Manufacturer": "Hynix"});
    assert_eq!(memory(&dimm).unwrap().detail, "32 GB · DDR4 · 2666 MT/s · Hynix");
    // iLO lists empty slots with no size.
    assert!(memory(&json!({"Name": "proc1dimm2", "CapacityMiB": 0})).is_none());

    let disk = json!({"Name": "Physical Disk 0:1:0", "CapacityBytes": 479559942144u64, "MediaType": "SSD", "Protocol": "SATA", "FailurePredicted": true, "Status": {"Health": "OK"}});
    let d = drive(&disk).unwrap();
    assert_eq!(d.health.as_deref(), Some("Warning"));
    assert!(d.detail.starts_with("480 GB · SSD · SATA"));
    assert!(d.detail.ends_with("failure predicted"));

    let nic_v = json!({"Name": "NIC.Integrated.1-1-1", "MACAddress": "00:00:5E:00:53:01", "SpeedMbps": 10000, "LinkStatus": "LinkUp",
        "IPv4Addresses": [{"Address": "10.0.0.20"}, {"Address": "0.0.0.0"}]});
    assert_eq!(nic(&nic_v).unwrap().detail, "00:00:5E:00:53:01 · 10 Gb/s · LinkUp · 10.0.0.20");

    let st = json!({"Name": "PERC H730P Mini", "StorageControllers": [{"Model": "PERC H730P Mini", "FirmwareVersion": "25.5.9.0001", "Status": {"Health": "OK"}}]});
    let c = controller(&st);
    assert_eq!(c.health.as_deref(), Some("OK"));
    assert_eq!(c.detail, "PERC H730P Mini · FW 25.5.9.0001");
    assert!(firmware(&json!({"Name": "BIOS"})).is_none());
}

#[test]
fn logs_and_jobs() {
    let svc = json!({"Name": "Log Service SEL", "Entries": {"@odata.id": "/redfish/v1/Managers/iDRAC.Embedded.1/LogServices/Sel/Entries"},
        "Actions": {"#LogService.ClearLog": {"target": "/redfish/v1/Managers/iDRAC.Embedded.1/LogServices/Sel/Actions/LogService.ClearLog"}}});
    let l = log_service(&svc, "/redfish/v1/Managers/iDRAC.Embedded.1/LogServices/Sel").unwrap();
    assert!(l.can_clear);
    assert!(log_service(&json!({"Name": "No entries"}), "/redfish/v1/x").is_none());

    let mut entries = vec![
        log_entry(&json!({"Id": "1", "Created": "2026-11-01T10:00:00-05:00", "Severity": "OK", "Message": "Log cleared."})),
        log_entry(&json!({"Id": "2", "Created": "2026-11-20T08:00:00+01:00", "Severity": "Critical", "Message": "The power supply is not working.", "MessageId": "PSU0003", "SensorType": "Power Supply"})),
        log_entry(&json!({"Id": "3", "Message": "No time"})),
    ];
    sort_entries(&mut entries);
    assert_eq!(entries[0].id, "2");
    assert_eq!(entries[2].id, "3");

    let dell = job(&json!({"Id": "JID_000000000001", "Name": "Configure: BIOS.Setup.1-1", "JobState": "Completed", "PercentComplete": 100,
        "Message": "Job completed successfully.", "StartTime": "TIME_NOW", "CompletionTime": "2026-11-20T10:00:00"}));
    assert_eq!(dell.state.as_deref(), Some("Completed"));
    assert_eq!(dell.start.as_deref(), Some("TIME_NOW"));
    let task = job(&json!({"Id": "1", "Name": "Firmware update", "TaskState": "Running", "PercentComplete": 40, "Messages": [{"Message": "Downloading"}]}));
    assert_eq!(task.state.as_deref(), Some("Running"));
    assert_eq!(task.message.as_deref(), Some("Downloading"));
}

#[test]
fn media_and_images() {
    let vm = virtual_media(&json!({"Name": "Virtual CD", "MediaTypes": ["CD", "DVD"], "Image": "http://10.0.0.1/a.iso", "Inserted": true}), "/redfish/v1/Managers/1/VirtualMedia/2");
    assert!(vm.inserted);
    assert_eq!(vm.media_types, vec!["CD", "DVD"]);
    assert!(valid_image("http://10.0.0.1/boot.iso"));
    assert!(valid_image("nfs://10.0.0.1:/isos/boot.iso"));
    assert!(!valid_image("file:///etc/passwd"));
    assert!(!valid_image("http://x/\n"));
}

#[test]
fn action_targets_stay_on_the_service() {
    let v = json!({"Actions": {"#A": {"target": "https://evil.example.com/x"}, "#B": {"target": "/redfish/v1/Systems/1/Actions/B"}}});
    assert!(action_target(&v, "#A").is_none());
    assert_eq!(action_target(&v, "#B").as_deref(), Some("/redfish/v1/Systems/1/Actions/B"));
}

#[test]
fn console_paths_and_commands() {
    assert_eq!(text_console_command("dell"), "console com2");
    assert_eq!(text_console_command("hpe"), "vsp");
    assert_eq!(text_console_command("openbmc"), "");
}
