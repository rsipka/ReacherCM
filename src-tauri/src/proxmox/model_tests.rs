use super::*;
use serde_json::json;

#[test]
fn names_and_ids_are_checked() {
    assert!(valid_node("pve1"));
    assert!(valid_node("node-2.lab"));
    assert!(!valid_node("../etc"));
    assert!(!valid_node(""));
    assert!(!valid_node("a/b"));
    assert!(valid_kind("qemu") && valid_kind("lxc") && !valid_kind("node"));
    assert!(valid_upid("UPID:pve1:0000ABCD:00112233:6750A1B2:qmstart:100:radek@pam!ci:"));
    assert!(!valid_upid("UPID:pve1:x/../y"));
    assert!(!valid_upid("pve1"));
    assert!(valid_snapshot("before_update-2"));
    assert!(!valid_snapshot("2fast"));
    assert!(!valid_snapshot("a b"));
}

#[test]
fn tags_split_every_way_proxmox_wrote_them() {
    assert_eq!(tags(Some("a;b")), vec!["a", "b"]);
    assert_eq!(tags(Some("a, b c")), vec!["a", "b", "c"]);
    assert!(tags(None).is_empty());
}

#[test]
fn resources_sort_nodes_then_guests_then_storage() {
    let list = resources(&json!([
        {"type": "storage", "id": "storage/pve1/local", "storage": "local", "node": "pve1"},
        {"type": "lxc", "id": "lxc/101", "vmid": 101, "node": "pve1", "template": 0},
        {"type": "qemu", "id": "qemu/100", "vmid": "100", "node": "pve1", "template": 1, "maxcpu": 2},
        {"type": "node", "id": "node/pve1", "node": "pve1"},
        {"id": "no type"}
    ]));
    let kinds: Vec<&str> = list.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["node", "qemu", "lxc", "storage"]);
    assert_eq!(list[1].vmid, Some(100));
    assert!(list[1].template);
    assert_eq!(list[1].maxcpu, Some(2.0));
}

#[test]
fn a_single_node_has_no_cluster_name() {
    let c = cluster(&json!([{"type": "node", "name": "pve1", "online": 1, "ip": "10.0.0.2"}]));
    assert_eq!(c.name, None);
    assert_eq!(c.quorate, None);
    assert_eq!(c.nodes, vec![("pve1".to_string(), true, Some("10.0.0.2".to_string()))]);
    let c = cluster(&json!([{"type": "cluster", "name": "lab", "quorate": 1}, {"type": "node", "name": "b", "online": 0}, {"type": "node", "name": "a", "online": 1}]));
    assert_eq!(c.name.as_deref(), Some("lab"));
    assert_eq!(c.quorate, Some(true));
    assert_eq!(c.nodes[0].0, "a");
}

#[test]
fn spice_needs_a_qxl_display() {
    assert!(spice_display(Some("qxl")));
    assert!(spice_display(Some("qxl2,memory=32")));
    assert!(spice_display(Some("type=qxl,memory=32")));
    assert!(!spice_display(Some("std")));
    assert!(!spice_display(Some("virtio")));
    assert!(!spice_display(None));
}

#[test]
fn a_containers_detail_has_no_spice_or_serials() {
    let d = guest_detail("pve1", "lxc", 101, &json!({"status": "running", "name": "dns"}), &json!({"hostname": "dns", "serial0": "socket", "vga": "qxl"}));
    assert!(!d.spice);
    assert!(d.serials.is_empty());
    assert!(!d.agent);
    assert_eq!(d.name.as_deref(), Some("dns"));
}

#[test]
fn addresses_skip_loopback_and_link_local() {
    let a = agent_addresses(&json!({"result": [
        {"ip-addresses": [{"ip-address": "127.0.0.1"}, {"ip-address": "::1"}]},
        {"ip-addresses": [{"ip-address": "10.0.0.5"}, {"ip-address": "fe80::5"}, {"ip-address": "fd00::5"}, {"ip-address": "169.254.1.1"}]}
    ]}));
    assert_eq!(a, vec!["10.0.0.5", "fd00::5"]);
    let l = lxc_addresses(&json!([{"name": "lo", "inet": "127.0.0.1/8"}, {"name": "eth0", "inet": "10.0.0.6/24", "inet6": "fe80::6/64"}]));
    assert_eq!(l, vec!["10.0.0.6"]);
}

#[test]
fn tasks_put_running_ones_first() {
    let list = tasks(
        &json!([
            {"upid": "UPID:pve1:1:1:A:vzdump::root@pam:", "type": "vzdump", "starttime": 10, "endtime": 20, "status": "OK"},
            {"upid": "UPID:pve1:2:2:B:qmstart:100:root@pam:", "type": "qmstart", "starttime": 5},
            {"upid": "not one", "starttime": 30}
        ]),
        10,
    );
    assert_eq!(list.len(), 2);
    assert!(list[0].running && list[0].status.is_none());
    assert_eq!(list[0].node, "pve1");
    assert_eq!(list[1].status.as_deref(), Some("OK"));
}

#[test]
fn task_logs_read_on() {
    let l = task_log(&json!([{"n": 1, "t": "no content"}]), &json!({"status": "stopped", "exitstatus": "OK"}), 0);
    assert!(l.lines.is_empty());
    assert!(!l.running);
    assert_eq!(l.exitstatus.as_deref(), Some("OK"));
    let l = task_log(&json!([{"n": 11, "t": "b"}, {"n": 12, "t": "c"}]), &json!({"status": "running"}), 10);
    assert_eq!(l.lines, vec!["b", "c"]);
    assert_eq!(l.next, 12);
    assert!(l.running && l.exitstatus.is_none());
}

#[test]
fn spice_and_console_tickets() {
    let s = spice_ticket(&json!({"host": "pvespiceproxy:abc:100:pve1::def", "password": "pw", "tls-port": 61000, "proxy": "http://10.0.0.2:3128",
        "host-subject": "OU=PVE Cluster Node,O=Proxmox Virtual Environment,CN=pve1", "ca": "-----BEGIN CERTIFICATE-----\\nAAA\\n-----END CERTIFICATE-----\\n", "type": "spice"}))
    .unwrap();
    assert_eq!(s.tls_port, Some(61000));
    assert!(s.ca.unwrap().contains("\nAAA\n"));
    assert!(spice_ticket(&json!({"host": "x"})).is_none());
    let p = proxy_ticket(&json!({"port": "5900", "ticket": "PVEVNC:x", "user": "radek@pam", "password": "abcd1234"})).unwrap();
    assert_eq!((p.port, p.password.as_deref()), (5900, Some("abcd1234")));
    assert!(proxy_ticket(&json!({"ticket": "x"})).is_none());
}
