use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use super::*;

const ID: &str = "3f2c1a9e-5b7d-4e0a-9c1b-2d3e4f5a6b7c";
const SESSION: &str = "c2Vzc2lvbmtleXNlc3Npb25rZXlzZXNzaW9ua2V5c2Vzc2lvbmtleQ==";

fn parent_env() -> Vec<(OsString, OsString)> {
    [
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("BW_SESSION", "inherited"),
        ("BW_PASSWORD", "inherited"),
        ("BW_CLIENTSECRET", "x"),
        ("BITWARDENCLI_APPDATA_DIR", "/tmp/elsewhere"),
        ("NODE_OPTIONS", "--require /tmp/evil.js"),
        ("NODE_EXTRA_CA_CERTS", "/tmp/ca.pem"),
        ("NODE_TLS_REJECT_UNAUTHORIZED", "0"),
        ("AWS_SECRET_ACCESS_KEY", "y"),
    ]
    .into_iter()
    .map(|(k, v)| (OsString::from(k), OsString::from(v)))
    .collect()
}

fn build(op: &Op, secret: Option<&str>) -> (Vec<String>, HashMap<String, Option<String>>) {
    let cmd = build_command_with_env(Path::new("/usr/bin/bw"), op, secret, parent_env()).unwrap();
    let std = cmd.as_std();
    let args = std.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    let env = std
        .get_envs()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned())))
        .collect();
    (args, env)
}

fn set(env: &HashMap<String, Option<String>>, k: &str) -> Option<String> {
    env.get(k).cloned().flatten()
}

#[test]
fn entry_ids_are_uuids() {
    assert_eq!(validate_entry_id(&format!("  {} ", ID.to_uppercase())).unwrap(), ID);
    for bad in ["", "--session", "prod db", "../x", &format!("{ID}/password")] {
        assert!(validate_entry_id(bad).is_err(), "accepted {bad}");
    }
}

#[test]
fn the_master_password_goes_in_the_environment_only() {
    let (args, env) = build(&Op::Unlock, Some("correct horse"));
    assert_eq!(args, ["unlock", "--passwordenv", "BW_PASSWORD", "--raw", "--nointeraction"]);
    assert_eq!(set(&env, "BW_PASSWORD").as_deref(), Some("correct horse"));
    assert_eq!(set(&env, "BW_SESSION"), None, "an inherited session is not used");
    assert!(build_command_with_env(Path::new("/usr/bin/bw"), &Op::Unlock, None, parent_env()).is_err());
}

#[test]
fn the_session_key_goes_in_the_environment_only() {
    for (op, want) in [
        (Op::Sync, vec!["sync", "--nointeraction"]),
        (Op::List, vec!["list", "items", "--nointeraction"]),
        (Op::Get(ID.into()), vec!["get", "password", ID, "--raw", "--nointeraction"]),
    ] {
        let (args, env) = build(&op, Some(SESSION));
        assert_eq!(args, want);
        assert_eq!(set(&env, "BW_SESSION").as_deref(), Some(SESSION));
        assert_eq!(set(&env, "BW_PASSWORD"), None);
        assert!(build_command_with_env(Path::new("/usr/bin/bw"), &op, None, parent_env()).is_err());
    }
}

#[test]
fn only_the_allowlist_is_inherited() {
    let (args, env) = build(&Op::Version, None);
    assert_eq!(args, ["--version"]);
    assert_eq!(set(&env, "PATH").as_deref(), Some("/usr/bin"));
    assert_eq!(set(&env, "HOME").as_deref(), Some("/home/u"));
    for k in [
        "BW_SESSION",
        "BW_PASSWORD",
        "BW_CLIENTSECRET",
        "BITWARDENCLI_APPDATA_DIR",
        "NODE_OPTIONS",
        "NODE_EXTRA_CA_CERTS",
        "NODE_TLS_REJECT_UNAUTHORIZED",
        "AWS_SECRET_ACCESS_KEY",
    ] {
        assert_eq!(set(&env, k), None, "{k} got through");
    }
}

fn items_json() -> String {
    format!(
        r#"[
        {{"object":"item","id":"{ID}","folderId":"11111111-2222-3333-4444-555555555555","type":1,"name":"prod db",
          "notes":"note-secret","fields":[{{"name":"pin","value":"field-secret","type":1}}],
          "login":{{"username":"admin","password":"hunter2","totp":"totp-secret",
                   "uris":[{{"match":null,"uri":"ssh://db.example.com"}},{{"uri":"https://other"}}]}}}},
        {{"object":"item","id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","type":2,"name":"A note","notes":"x"}},
        {{"object":"item","id":"bbbbbbbb-bbbb-cccc-dddd-eeeeeeeeeeee","type":1,"name":"Bare","login":{{"username":null,"password":null,"uris":null}}}},
        {{"object":"item","id":"--bad","type":1,"name":"x","login":{{}}}}
    ]"#
    )
}

#[test]
fn the_list_keeps_logins_and_metadata_only() {
    let list = parse_list(items_json().as_bytes()).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "Bare");
    assert_eq!(list[0].username, None);
    assert_eq!(list[0].uri, None);
    assert_eq!(list[1].name, "prod db");
    assert_eq!(list[1].id, ID);
    assert_eq!(list[1].username.as_deref(), Some("admin"));
    assert_eq!(list[1].uri.as_deref(), Some("ssh://db.example.com"));
    let shown = format!("{list:?}");
    for secret in ["hunter2", "totp-secret", "note-secret", "field-secret"] {
        assert!(!shown.contains(secret), "{secret} is in the listing");
    }
    assert!(parse_list(b"not json").is_err());
}

#[test]
fn the_get_output_is_the_password() {
    assert_eq!(parse_get(b"s3cret").unwrap().as_str(), "s3cret");
    assert_eq!(parse_get(b"s3cret\n").unwrap().as_str(), "s3cret");
    assert_eq!(parse_get(b" spaces kept ").unwrap().as_str(), " spaces kept ");
    assert!(parse_get(b"").is_err());
}

#[test]
fn a_session_key_is_base64_and_never_printed() {
    let key = parse_session(format!("{SESSION}\n").as_bytes()).unwrap();
    assert_eq!(key.reveal().as_str(), SESSION);
    assert!(!format!("{key:?}").contains(SESSION));
    for bad in ["", "You are not logged in.", "? Master password: [hidden]"] {
        assert!(parse_session(bad.as_bytes()).is_err(), "accepted {bad:?}");
    }
    assert!(parse_session("A".repeat(600).as_bytes()).is_err());
}

#[test]
fn errors_are_told_apart_and_explained() {
    assert_eq!(error_kind(b"\x1b[91mInvalid master password.\x1b[39m\n"), ErrorKind::WrongPassword);
    assert_eq!(error_kind(b"You are not logged in."), ErrorKind::NotLoggedIn);
    assert_eq!(error_kind(b"Vault is locked."), ErrorKind::Locked);
    assert_eq!(error_kind(b"Not found."), ErrorKind::Other);
    assert!(matches!(failure(b"Invalid master password."), Failure::WrongPassphrase(m) if m.contains("not accepted")));
    assert!(matches!(failure(b"You are not logged in."), Failure::Other(m) if m.contains("bw login")));
    assert!(friendly_error(b"Not found.").contains("entry not found"));
    assert_eq!(friendly_error(b"something \x1b[31mnew\x1b[0m\n"), "Bitwarden: something new");
    assert_eq!(friendly_error(b""), "Bitwarden: the CLI failed without saying why.");
}

#[test]
fn versions_are_read() {
    assert_eq!(parse_version(b"2025.1.3\n").as_deref(), Some("2025.1.3"));
    assert_eq!(parse_version(b""), None);
}

#[test]
fn the_master_password_is_always_asked_for() {
    let cfg = |remember_secs| BitwardenConfig { enabled: true, remember_secs, ..Default::default() };
    let p = Bitwarden::new(cfg(300), true);
    assert_eq!(p.passphrase_prompt(), Some(300));
    assert_eq!(p.secret_kind(), SecretKind::MasterPassword);
    assert_eq!(Bitwarden::new(cfg(0), true).passphrase_prompt(), Some(0));
    assert_eq!(p.check_entry(ID, None).unwrap(), ID);
    assert!(p.check_entry(ID, Some("totp")).is_err());
}

#[tokio::test]
async fn a_session_key_is_kept_for_its_master_password_only() {
    sessions::isolated(async {
        let key = std::sync::Arc::new(parse_session(SESSION.as_bytes()).unwrap());
        // Serials no real typing in this test run has.
        let (master, other) = (u64::MAX - 11, u64::MAX - 12);
        sessions::keep(master, key.clone());
        assert!(sessions::get(other).is_none());
        assert_eq!(sessions::get(master).map(|k| k.serial()), Some(key.serial()));
        sessions::drop_key(key.serial());
        assert!(sessions::get(master).is_none());
    })
    .await;
}

/// A stand-in `bw`: unlocks with "right", wants the session key it gave,
/// and writes down how it was called.
#[cfg(all(unix, desktop))]
fn fake_bw(tag: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("reach-bw-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("items.json"), items_json()).unwrap();
    let script = format!(
        r#"#!/bin/sh
here=$(dirname "$0")
echo "$1 $2" >> "$here/calls.log"
locked() {{ [ "$BW_SESSION" = "{SESSION}" ] || {{ echo "Vault is locked." >&2; exit 1; }}; }}
case "$1" in
  --version) echo 2025.1.3 ;;
  unlock) if [ "$BW_PASSWORD" = "right" ]; then printf '%s' "{SESSION}"; else echo "Invalid master password." >&2; exit 1; fi ;;
  sync) locked; echo "Syncing complete." ;;
  list) locked; cat "$here/items.json" ;;
  get) locked; if [ "$3" = "{ID}" ]; then printf 'hunter2'; else echo "Not found." >&2; exit 1; fi ;;
esac
"#
    );
    let bin = dir.join("bw");
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[cfg(all(unix, desktop))]
#[tokio::test]
async fn a_whole_run_against_a_stand_in_cli() {
    sessions::isolated(async {
        let bin = fake_bw("run");
        let dir = bin.parent().unwrap().to_path_buf();
        let cfg = BitwardenConfig { cli_path: Some(bin.to_str().unwrap().into()), enabled: true, ..Default::default() };
        let p = Bitwarden::new(cfg, true);
        assert_eq!(p.status().await.version.as_deref(), Some("2025.1.3"));

        let wrong = Passphrase::new("wrong".into()).unwrap();
        assert!(matches!(p.list(Some(&wrong)).await, Err(Failure::WrongPassphrase(_))));
        assert!(matches!(p.list(None).await, Err(Failure::Other(_))));

        let right = Passphrase::new("right".into()).unwrap();
        let list = p.list(Some(&right)).await.map_err(Failure::into_message).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(p.resolve(ID, Some(&right)).await.map_err(Failure::into_message).unwrap().as_str(), "hunter2");
        let missing = p.resolve("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", Some(&right)).await.unwrap_err();
        assert!(missing.into_message().contains("entry not found"));

        // One unlock for the right password, however many calls followed.
        let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap();
        let unlocks = calls.lines().filter(|l| l.starts_with("unlock")).count();
        assert_eq!(unlocks, 2, "one for the wrong password, one for the right: {calls}");
        assert!(calls.contains("sync"), "{calls}");

        // The key was dropped elsewhere (`bw lock`, "Forget now"): unlocked anew.
        sessions::forget();
        assert_eq!(p.resolve(ID, Some(&right)).await.map_err(Failure::into_message).unwrap().as_str(), "hunter2");
        let _ = std::fs::remove_dir_all(dir);
    })
    .await;
}
