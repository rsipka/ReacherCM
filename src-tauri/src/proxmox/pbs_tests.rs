use super::*;

#[test]
fn names_are_checked() {
    assert!(valid_store("store1") && valid_store("_b.k-2") && !valid_store("") && !valid_store("a/b") && !valid_store("../x"));
    assert!(valid_backup_type("vm") && valid_backup_type("host") && !valid_backup_type("disk"));
    assert!(valid_backup_id("100") && valid_backup_id("web.example") && !valid_backup_id("a b") && !valid_backup_id(".x"));
    assert!(valid_ns("") && valid_ns("lab/old") && !valid_ns("lab/") && !valid_ns("/lab") && !valid_ns("a/../b"));
    assert!(!valid_ns("a/b/c/d/e/f/g/h"));
}

#[test]
fn namespaces_always_have_the_root() {
    assert_eq!(namespaces(&json!([{"ns": "b"}, {"ns": "a"}])), vec!["", "a", "b"]);
    assert_eq!(namespaces(&json!([{"ns": "ok"}, {"ns": "../bad"}])), vec!["", "ok"]);
}

#[test]
fn groups_sort_by_type_then_number() {
    let g = groups(&json!([
        {"backup-type": "host", "backup-id": "laptop"},
        {"backup-type": "vm", "backup-id": "1000"},
        {"backup-type": "ct", "backup-id": "101"},
        {"backup-type": "vm", "backup-id": "200"}
    ]));
    let ids: Vec<_> = g.iter().map(|x| format!("{}/{}", x.kind, x.id)).collect();
    assert_eq!(ids, vec!["vm/200", "vm/1000", "ct/101", "host/laptop"]);
}

#[test]
fn a_datastore_only_in_the_usage_is_still_shown() {
    let d = datastores(&Value::Null, &json!([{"store": "b", "total": 10}, {"store": "a", "total": 5, "estimated-full-date": -1}]));
    assert_eq!(d.iter().map(|x| x.store.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    assert_eq!(d[0].full_at, None);
}

#[test]
fn a_pbs_node_reads_its_root_disk() {
    let n = pbs_node(&json!({"root": {"used": 1, "total": 2}, "info": {"hostname": "backup1"}}));
    assert_eq!((n.root_used, n.root_total, n.name.as_str()), (Some(1), Some(2), "backup1"));
}
