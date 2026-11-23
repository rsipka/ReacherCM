//! Against a real SSH server, as a user who cannot write the target folder
//! (issue #161). Run with `REACH_SSH_TEST=host:port REACH_SSH_USER=user
//! REACH_SSH_KEY=path REACH_SSH_HOSTKEY=SHA256-fingerprint
//! REACH_TEST_DATA_DIR=scratch cargo test --lib sftp::transfer::live -- --ignored --nocapture`.

use super::*;
use crate::ssh::client::{cascade_authenticate, AuthParams, KeyAuth, KeySource, SshClientHandler};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn connect() -> SharedHandle {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let addr = var("REACH_SSH_TEST");
    let (host, port) = addr.rsplit_once(':').unwrap();
    let port: u16 = port.parse().unwrap();
    // Keep the host-key store out of the user's real data folder.
    let dir = std::path::PathBuf::from(var("REACH_TEST_DATA_DIR"));
    let _ = crate::APP_DATA_DIR.set(dir.clone());
    std::fs::create_dir_all(dir.join("ssh")).unwrap();
    let known = serde_json::json!({ "entries": { format!("{host}:{port}"): var("REACH_SSH_HOSTKEY") } });
    std::fs::write(dir.join("ssh").join("known_hosts.json"), known.to_string()).unwrap();

    let config = Arc::new(russh::client::Config::default());
    let mut handle = russh::client::connect(config, (host, port), SshClientHandler::new(host, port, None))
        .await
        .unwrap();
    let auth = AuthParams {
        key: Some(KeyAuth { source: KeySource::Path(var("REACH_SSH_KEY")), passphrase: None }),
        password: None,
        allow_agent: false,
    };
    cascade_authenticate(&mut handle, &var("REACH_SSH_USER"), &auth).await.unwrap().into_result().unwrap();
    Arc::new(tokio::sync::Mutex::new(handle))
}

#[tokio::test]
#[ignore = "needs an SSH server"]
async fn live_upload_into_a_folder_the_user_cannot_write() {
    let handle = connect().await;
    for size in [1024usize, 20 * 1024 * 1024] {
        let data = vec![7u8; size];
        let started = Instant::now();
        let mut last = 0u64;
        let result = tokio::time::timeout(
            Duration::from_secs(90),
            send_bytes(&handle, &data, "/opt/reach-161-test", &CancellationToken::new(), |sent| last = sent),
        )
        .await;
        println!("{size:>9} bytes after {:?}: last progress {last}: {result:?}", started.elapsed());
        let outcome = result.expect("the upload hung instead of failing");
        let err = outcome.expect_err("writing into /opt must fail");
        assert!(err.to_string().contains("Permission denied"), "{err}");
    }
}

/// A cancelled upload stops at once and leaves no partial file behind.
#[tokio::test]
#[ignore = "needs an SSH server"]
async fn live_cancelled_upload_leaves_nothing() {
    let handle = connect().await;
    let target = "/tmp/reach-161-cancel";
    let data = vec![7u8; 200 * 1024 * 1024];
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        send_bytes(&handle, &data, target, &cancel, |sent| {
            if sent >= 4 * 1024 * 1024 {
                stopper.cancel();
            }
        }),
    )
    .await
    .expect("cancel did not stop the upload");
    println!("cancelled after {:?}: {result:?}", started.elapsed());
    assert!(matches!(result, Err(TransferError::Cancelled)));
    let (out, _, _) = exec_on_connection_with_exit_code(&handle, &format!("ls {target} 2>&1; true")).await.unwrap();
    println!("ls: {}", out.trim());
    assert!(out.contains("No such file"), "partial file left: {out}");
}

/// The ordinary case still arrives whole.
#[tokio::test]
#[ignore = "needs an SSH server"]
async fn live_upload_arrives_whole() {
    let handle = connect().await;
    let target = "/tmp/reach-161-ok";
    let data: Vec<u8> = (0..(20 * 1024 * 1024 + 7)).map(|i| (i * 31 % 251) as u8).collect();
    send_bytes(&handle, &data, target, &CancellationToken::new(), |_| {}).await.unwrap();
    let (out, _, _) = exec_on_connection_with_exit_code(&handle, &format!("stat -c%s {target}; rm -f {target}")).await.unwrap();
    assert_eq!(out.trim(), data.len().to_string());
    // Empty file into a folder the user cannot write: an error, not silence.
    let err = send_bytes(&handle, &[], "/opt/reach-161-empty", &CancellationToken::new(), |_| {}).await.unwrap_err();
    assert!(err.to_string().contains("Permission denied"), "{err}");
}

/// The explorer's "terminal folder" (#162): where the shell is, and where
/// its foreground job is while one runs.
#[tokio::test]
#[ignore = "needs an SSH server"]
async fn live_terminal_cwd() {
    use crate::sftp::terminal_cwd::terminal_cwd;
    let handle = connect().await;
    let ch = { handle.lock().await.channel_open_session().await.unwrap() };
    ch.request_pty(false, "xterm", 80, 24, 0, 0, &[]).await.unwrap();
    ch.request_shell(false).await.unwrap();
    let settle = || tokio::time::sleep(Duration::from_millis(400));
    settle().await;
    assert_eq!(terminal_cwd(&handle).await.unwrap().as_deref(), Some("/home/reach"));
    ch.data(&b"cd '/usr/share'
"[..]).await.unwrap();
    settle().await;
    assert_eq!(terminal_cwd(&handle).await.unwrap().as_deref(), Some("/usr/share"));
    ch.data(&b"cd /etc && sleep 30
"[..]).await.unwrap();
    settle().await;
    assert_eq!(terminal_cwd(&handle).await.unwrap().as_deref(), Some("/etc"));
    let started = Instant::now();
    for _ in 0..10 {
        terminal_cwd(&handle).await.unwrap();
    }
    println!("one lookup takes {:?}", started.elapsed() / 10);
}

/// The explorer's operations over SFTP and over the shell, on the same
/// server. `REACH_TEST_REMOTE_DIR` is a folder the user may write; for an
/// SFTP-only account in a chroot, a path inside the jail. Set
/// `REACH_SFTP_ONLY=1` for such an account to skip the shell run.
#[tokio::test]
#[ignore = "needs an SSH server"]
async fn live_explorer_operations() {
    use crate::sftp::backend::{RemoteFs, SftpSlot};
    use crate::sftp::browser;

    let handle = connect().await;
    let base = std::env::var("REACH_TEST_REMOTE_DIR").unwrap_or_else(|_| "/tmp".into());
    let mut runs = vec![("sftp", RemoteFs::new(handle.clone(), SftpSlot::default()))];
    if std::env::var("REACH_SFTP_ONLY").is_err() {
        runs.push(("shell", RemoteFs::shell_only(handle.clone())));
    }
    for (how, fs) in runs {
        assert_eq!(fs.sftp().await.is_some(), how == "sftp", "{how}");
        let root = format!("{base}/rcm-live-{how}");
        let _ = browser::delete_entry(&fs, &root).await;

        browser::make_directory(&fs, &format!("{root}/a/b")).await.unwrap();
        browser::write_text_file(&fs, &format!("{root}/a/note.txt"), "hello\nwörld\n").await.unwrap();
        assert_eq!(browser::read_text_file(&fs, &format!("{root}/a/note.txt")).await.unwrap(), "hello\nwörld\n");
        browser::touch_file(&fs, &format!("{root}/a/b/empty")).await.unwrap();
        browser::touch_file(&fs, &format!("{root}/a/note.txt")).await.unwrap();
        assert_eq!(browser::read_text_file(&fs, &format!("{root}/a/note.txt")).await.unwrap().len(), 13, "{how}: touch kept the content");

        let listed = browser::list_directory(&fs, &format!("{root}/a")).await.unwrap();
        let names: Vec<_> = listed.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, [("b", true), ("note.txt", false)], "{how}");
        let note = &listed[1];
        assert_eq!((note.size, note.permissions.len()), (13, 10), "{how}: {note:?}");
        assert!(!note.owner.is_empty() && note.modified > 0, "{how}: {note:?}");
        println!("{how}: {note:?}");

        let err = browser::rename_entry(&fs, &format!("{root}/a/note.txt"), &format!("{root}/a/b")).await.unwrap_err();
        assert!(err.to_string().contains("already exists"), "{how}: {err}");
        browser::rename_entry(&fs, &format!("{root}/a/note.txt"), &format!("{root}/a/renamed.txt")).await.unwrap();

        let err = browser::list_directory(&fs, &format!("{root}/missing")).await.unwrap_err();
        println!("{how}: missing folder: {err}");

        // A big file there and back, with its timing.
        let data: Vec<u8> = (0..(30 * 1024 * 1024 + 7)).map(|i| (i * 31 % 251) as u8).collect();
        let local = std::env::temp_dir().join(format!("rcm-live-{how}.bin"));
        std::fs::write(&local, &data).unwrap();
        let remote = format!("{root}/a/b/big.bin");
        let started = Instant::now();
        let sent = send_file(&fs, fs.sftp().await.as_deref(), &local, &remote, &CancellationToken::new(), |_| {}).await.unwrap();
        println!("{how}: upload {} MB in {:?}", sent >> 20, started.elapsed());
        assert_eq!(sent, data.len() as u64);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<Vec<u8>, String>>(1024);
        let reader = std::thread::spawn(move || {
            let mut back = Vec::new();
            for chunk in rx {
                back.extend(chunk.unwrap());
            }
            back
        });
        let started = Instant::now();
        stream_file(&fs, &remote, tx).await.unwrap();
        println!("{how}: download in {:?}", started.elapsed());
        assert!(reader.join().unwrap() == data, "{how}: the file came back different");

        browser::delete_entry(&fs, &root).await.unwrap();
        assert!(browser::list_directory(&fs, &root).await.is_err(), "{how}: still there");
        let _ = std::fs::remove_file(local);
    }
}

/// An SCP session's copies: a file up and back down whole, a refusal said
/// as the server says it, a folder refused by name.
#[tokio::test]
#[ignore = "needs an SSH server"]
async fn live_scp_round_trip() {
    let handle = connect().await;
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..5_000_000u32).map(|i| (i % 253) as u8).collect();
    let local = dir.path().join("up.bin");
    std::fs::write(&local, &data).unwrap();
    let remote = format!("/tmp/reach scp {}.bin", std::process::id());
    let never = CancellationToken::new();
    let sent = super::super::scp::send_file(&handle, &local, &remote, &never, |_| {}).await.unwrap();
    assert_eq!(sent, data.len() as u64);
    let back = dir.path().join("down.bin");
    let mut last = (0, 0);
    let got = super::super::scp::receive_file(&handle, &remote, &DownloadTarget::Chosen(back.clone()), &never, |d, t| last = (d, t)).await.unwrap();
    assert_eq!(got, data.len() as u64);
    assert_eq!(last, (got, got));
    assert_eq!(std::fs::read(&back).unwrap(), data);

    let denied = super::super::scp::send_file(&handle, &local, "/opt/reach-scp-denied", &never, |_| {}).await.unwrap_err();
    println!("denied: {denied}");
    assert!(denied.to_string().contains("Permission denied"), "{denied}");
    let folder = super::super::scp::receive_file(&handle, "/tmp", &DownloadTarget::Chosen(dir.path().join("x")), &never, |_, _| {}).await.unwrap_err();
    println!("folder: {folder}");
    assert!(!dir.path().join("x").exists());
    let missing = super::super::scp::receive_file(&handle, "/tmp/reach-no-such-file", &DownloadTarget::Chosen(dir.path().join("y")), &never, |_, _| {}).await.unwrap_err();
    println!("missing: {missing}");
    assert!(missing.to_string().contains("No such file"), "{missing}");
    let _ = exec_on_connection_with_exit_code(&handle, &format!("rm -f {}", shell_escape(&remote))).await;
}
