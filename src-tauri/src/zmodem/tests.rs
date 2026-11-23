//! The tap against lrzsz's own `sz` and `rz`, over pipes, the way a session
//! loop drives it. Skipped where lrzsz is not installed.

use super::*;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

struct Run {
    events: Arc<Mutex<Vec<ZmodemEvent>>>,
    shown: Vec<u8>,
}

/// Drive `tap` against `child` until it exits; `answer` is called on each
/// new event and may decide for the user. Reads are cut to `chunk` bytes.
fn drive(id: &str, mut child: Child, chunk: usize, mut answer: impl FnMut(&ZmodemEvent) -> Option<Decision>) -> Run {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let mut tap = Tap::new(id, WINDOW_NETWORK, Box::new(move |e| sink.lock().unwrap().push(e)));
    let mut stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for piece in buf[..n].chunks(chunk) {
                        if tx.send(piece.to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    let mut shown = Vec::new();
    let mut seen = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "transfer hung");
        let out = match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(data) => tap.input(&data),
            Err(mpsc::RecvTimeoutError::Timeout) => tap.poll(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                tap.poll()
            }
        };
        shown.extend(out.display);
        if !out.wire.is_empty() && stdin.write_all(&out.wire).is_err() {
            // The program ended.
        }
        let _ = stdin.flush();
        let new: Vec<ZmodemEvent> = events.lock().unwrap()[seen..].to_vec();
        seen += new.len();
        for e in new {
            if let Some(d) = answer(&e) {
                decide(id, d).unwrap();
            }
        }
    }
    let _ = child.wait();
    // Let the tap see the end of its drain.
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(120));
        shown.extend(tap.poll().display);
    }
    Run { events, shown }
}

fn pattern(len: usize, seed: u32) -> Vec<u8> {
    // Every byte value, the ZMODEM specials among them, in a run that does
    // not repeat on 1 KiB boundaries.
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect()
}

fn spawn(cmd: &str, args: &[&str], dir: &std::path::Path) -> Child {
    Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn last(run: &Run) -> ZmodemEvent {
    run.events.lock().unwrap().last().cloned().unwrap()
}

#[test]
fn download_from_sz() {
    if !have("sz") {
        return;
    }
    for (chunk, size) in [(16 * 1024, 300_000), (7, 5000), (1, 1500)] {
        let remote = tempfile::tempdir().unwrap();
        let here = tempfile::tempdir().unwrap();
        let data = pattern(size, size as u32);
        std::fs::write(remote.path().join("fw.bin"), &data).unwrap();
        std::fs::write(remote.path().join("empty.txt"), b"").unwrap();
        // A file of the same name already here is kept.
        std::fs::write(here.path().join("fw.bin"), b"old").unwrap();
        let folder = here.path().to_path_buf();
        let child = spawn("sz", &["fw.bin", "empty.txt"], remote.path());
        let run = drive(&format!("dl-{chunk}"), child, chunk, |e| match e {
            ZmodemEvent::Offer { direction: Direction::Download } => Some(Decision::Download(folder.clone())),
            _ => None,
        });
        match last(&run) {
            ZmodemEvent::Finished { files, .. } => assert_eq!(files, vec!["fw (1).bin", "empty.txt"]),
            e => panic!("chunk {chunk}: {e:?}"),
        }
        assert_eq!(std::fs::read(here.path().join("fw (1).bin")).unwrap(), data, "chunk {chunk}");
        assert_eq!(std::fs::read(here.path().join("fw.bin")).unwrap(), b"old");
        assert_eq!(std::fs::read(here.path().join("empty.txt")).unwrap(), b"");
        // Only sz's own "rz\r" before the header shows, and no "OO" after.
        assert!(!run.shown.contains(&0x18), "chunk {chunk}: {:?}", String::from_utf8_lossy(&run.shown));
        assert!(!run.shown.ends_with(b"OO"), "chunk {chunk}");
    }
}

#[test]
fn upload_to_rz() {
    if !have("rz") {
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let here = tempfile::tempdir().unwrap();
    let a = pattern(200_000, 1);
    let b = pattern(1, 2);
    std::fs::write(here.path().join("config.txt"), &a).unwrap();
    std::fs::write(here.path().join("one.bin"), &b).unwrap();
    let files = vec![here.path().join("config.txt"), here.path().join("one.bin")];
    let child = spawn("rz", &["-y"], remote.path());
    let run = drive("ul", child, 4096, |e| match e {
        ZmodemEvent::Offer { direction: Direction::Upload } => Some(Decision::Upload(files.clone())),
        _ => None,
    });
    match last(&run) {
        ZmodemEvent::Finished { files, folder, .. } => {
            assert_eq!(files, vec!["config.txt", "one.bin"]);
            assert!(folder.is_none());
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(std::fs::read(remote.path().join("config.txt")).unwrap(), a);
    assert_eq!(std::fs::read(remote.path().join("one.bin")).unwrap(), b);
    let progress = run.events.lock().unwrap().iter().filter(|e| matches!(e, ZmodemEvent::Progress { .. })).count();
    assert!(progress > 0);
}

#[test]
fn declining_an_offer_stops_sz() {
    if !have("sz") {
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    std::fs::write(remote.path().join("x.bin"), pattern(10_000, 3)).unwrap();
    let child = spawn("sz", &["x.bin"], remote.path());
    let started = Instant::now();
    let run = drive("decline", child, 4096, |e| match e {
        ZmodemEvent::Offer { .. } => Some(Decision::Cancel),
        _ => None,
    });
    assert!(matches!(last(&run), ZmodemEvent::Cancelled { .. }));
    assert!(started.elapsed() < Duration::from_secs(15), "sz did not stop");
}

#[test]
fn cancelling_midway_removes_the_partial_file() {
    if !have("sz") {
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let here = tempfile::tempdir().unwrap();
    std::fs::write(remote.path().join("big.bin"), pattern(5_000_000, 4)).unwrap();
    let folder = here.path().to_path_buf();
    let child = spawn("sz", &["big.bin"], remote.path());
    let id = "cancel";
    // Small reads keep the transfer slow enough to stop halfway.
    let run = drive(id, child, 256, |e| match e {
        ZmodemEvent::Offer { .. } => Some(Decision::Download(folder.clone())),
        ZmodemEvent::Progress { .. } => Some(Decision::Cancel),
        _ => None,
    });
    assert!(matches!(last(&run), ZmodemEvent::Cancelled { .. }), "{:?}", run.events.lock().unwrap());
    assert_eq!(std::fs::read_dir(here.path()).unwrap().count(), 0);
}

#[test]
fn keys_are_held_while_busy_and_ctrl_c_cancels() {
    let mut tap = Tap::new("keys", WINDOW_SERIAL, Box::new(|_| {}));
    assert!(!intercept_input("keys", b"ls\r"));
    let out = tap.input(b"$ sz x\r\nrz\r**\x18B00000000000000\r\x8a\x11");
    assert_eq!(out.display, b"$ sz x\r\nrz\r\r\x1b[K");
    assert!(tap.busy());
    assert!(intercept_input("keys", b"a"));
    assert!(intercept_input("keys", b"\x03"));
    let out = tap.poll();
    assert_eq!(out.wire, CANCEL);
    drop(tap);
    assert!(!intercept_input("keys", b"a"));
}

#[test]
fn names_from_the_remote_are_made_safe() {
    use transfer::safe_name;
    assert_eq!(safe_name("../../etc/passwd"), "passwd");
    assert_eq!(safe_name("C:\\x\\boot.ini"), "boot.ini");
    assert_eq!(safe_name(".."), "download");
    assert_eq!(safe_name(".bashrc"), ".bashrc");
    assert_eq!(safe_name("..."), "download");
    assert_eq!(safe_name("x. "), "x");
    assert_eq!(safe_name("a\x1b[31mb"), "a_[31mb");
    assert_eq!(safe_name("con.txt"), "_con.txt");
    assert_eq!(safe_name("what?.txt"), "what_.txt");
}

#[test]
fn paths_from_the_remote_stay_inside_the_folder() {
    use transfer::safe_path;
    assert_eq!(safe_path("./proj/src/main.rs"), (vec!["proj".into(), "src".into()], "main.rs".into()));
    assert_eq!(safe_path("../../etc/passwd"), (vec!["etc".into()], "passwd".into()));
    assert_eq!(safe_path("/abs/x"), (vec!["abs".into()], "x".into()));
    assert_eq!(safe_path("a\\b\\c.txt"), (vec!["a".into(), "b".into()], "c.txt".into()));
}

#[test]
fn a_folder_goes_up_whole() {
    if !have("rz") {
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let here = tempfile::tempdir().unwrap();
    let top = here.path().join("site");
    std::fs::create_dir_all(top.join("css/deep")).unwrap();
    std::fs::write(top.join("index.html"), b"<html>").unwrap();
    std::fs::write(top.join(".htaccess"), b"x").unwrap();
    std::fs::write(top.join("css/deep/a.css"), pattern(5000, 9)).unwrap();
    let send = vec![top.clone()];
    let child = spawn("rz", &["-y"], remote.path());
    let run = drive("ul-dir", child, 4096, |e| match e {
        ZmodemEvent::Offer { direction: Direction::Upload } => Some(Decision::Upload(send.clone())),
        _ => None,
    });
    match last(&run) {
        ZmodemEvent::Finished { files, .. } => assert_eq!(files, vec!["site/.htaccess", "site/css/deep/a.css", "site/index.html"]),
        e => panic!("{e:?}"),
    }
    assert_eq!(std::fs::read(remote.path().join("site/css/deep/a.css")).unwrap(), pattern(5000, 9));
    assert_eq!(std::fs::read(remote.path().join("site/index.html")).unwrap(), b"<html>");
}

#[test]
fn a_folder_comes_down_whole() {
    if !have("sz") {
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let here = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(remote.path().join("logs/old")).unwrap();
    std::fs::write(remote.path().join("logs/now.log"), b"now").unwrap();
    std::fs::write(remote.path().join("logs/old/then.log"), b"then").unwrap();
    // Already here: kept, the new one beside it.
    std::fs::create_dir_all(here.path().join("logs")).unwrap();
    std::fs::write(here.path().join("logs/now.log"), b"mine").unwrap();
    let folder = here.path().to_path_buf();
    let child = spawn("sz", &["-f", "logs/now.log", "logs/old/then.log"], remote.path());
    let run = drive("dl-dir", child, 4096, |e| match e {
        ZmodemEvent::Offer { direction: Direction::Download } => Some(Decision::Download(folder.clone())),
        _ => None,
    });
    match last(&run) {
        ZmodemEvent::Finished { files, .. } => assert_eq!(files, vec!["logs/now (1).log", "logs/old/then.log"]),
        e => panic!("{e:?}"),
    }
    assert_eq!(std::fs::read(here.path().join("logs/now.log")).unwrap(), b"mine");
    assert_eq!(std::fs::read(here.path().join("logs/now (1).log")).unwrap(), b"now");
    assert_eq!(std::fs::read(here.path().join("logs/old/then.log")).unwrap(), b"then");
}

/// What the terminal shows around a transfer: the line rz or sz printed is
/// cleared, and no part of a header (`*`, CR with the high bit, XON, "OO")
/// is left on screen, however the reads are cut.
#[test]
fn nothing_of_the_protocol_shows() {
    if !have("rz") || !have("sz") {
        return;
    }
    for chunk in [4096, 3, 1] {
        for (name, prog, args, what) in [("cancel", "rz", vec![], 0), ("upload", "rz", vec!["-y"], 1), ("download", "sz", vec!["a.bin"], 2)] {
            let remote = tempfile::tempdir().unwrap();
            let here = tempfile::tempdir().unwrap();
            std::fs::write(remote.path().join("a.bin"), pattern(3000, 1)).unwrap();
            std::fs::write(here.path().join("u.bin"), pattern(3000, 2)).unwrap();
            let up = vec![here.path().join("u.bin")];
            let folder = here.path().to_path_buf();
            let child = spawn(prog, &args, remote.path());
            let run = drive(&format!("show-{name}-{chunk}"), child, chunk, |e| match e {
                ZmodemEvent::Offer { .. } => Some(match what {
                    0 => Decision::Cancel,
                    1 => Decision::Upload(up.clone()),
                    _ => Decision::Download(folder.clone()),
                }),
                _ => None,
            });
            // A `*` shown before the header is recognised is on the line
            // that gets cleared; after the clear nothing may follow.
            let shown = String::from_utf8_lossy(&run.shown).into_owned();
            let (before, after) = shown.rsplit_once("\r\x1b[K").unwrap_or_else(|| panic!("{name}, reads of {chunk}: no clear in {shown:?}"));
            assert_eq!(after, "", "{name}, reads of {chunk}: {shown:?}");
            assert!(!before.contains(['\x18', '\x11', '\u{fffd}']), "{name}, reads of {chunk}: {shown:?}");
        }
    }
}

/// Under tmux the header arrives drawn as text: sz or rz is stopped (it
/// would wait for minutes) and the terminal is told why.
#[test]
fn a_header_drawn_by_tmux_stops_the_program() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let mut tap = Tap::new("tmux-drawn", WINDOW_NETWORK, Box::new(move |e| sink.lock().unwrap().push(e)));
    // As tmux 3.4 sends it, cut in two.
    let a = tap.input(b"\x1b[1;3Hsz a.txt\r\n**B000000");
    let b = tap.input(b"00000000\r");
    assert_eq!([a.display, b.display].concat(), b"\x1b[1;3Hsz a.txt\r\n**B00000000000000\r");
    assert!(a.wire.is_empty());
    assert_eq!(b.wire, CANCEL);
    assert!(!tap.busy());
    assert_eq!(*events.lock().unwrap(), vec![ZmodemEvent::Unsupported { direction: Direction::Download }]);
    let c = tap.input(b"rz waiting to receive.**B0100000023be50\r");
    assert_eq!(c.wire, CANCEL);
    // Ordinary text that only looks a bit like it does nothing.
    assert!(tap.input(b"**B0 is not a header\r\n").wire.is_empty());
}
