//! Against a real FTP server; ignored unless asked for:
//!
//! REACH_FTP_PORT=2121 REACH_FTP_TLS=explicit REACH_FTP_USER=u REACH_FTP_PASS=p \
//!   cargo test --lib ftp::ops::live_tests -- --ignored
//!
//! `REACH_FTP_TLS` is `explicit`, `implicit` or `none`; the server's
//! certificate (self-signed, say) is trusted after the first refusal, as
//! the prompt would. Works in a folder of its own under the login folder.

use super::*;
use crate::ftp::{FtpConn, FtpError, Params, Security};

fn params() -> Option<Params> {
    let port = std::env::var("REACH_FTP_PORT").ok()?.parse().ok()?;
    Some(Params {
        host: std::env::var("REACH_FTP_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port,
        username: std::env::var("REACH_FTP_USER").unwrap_or_default(),
        password: std::env::var("REACH_FTP_PASS").unwrap_or_default(),
        security: Security::parse(std::env::var("REACH_FTP_TLS").ok().as_deref()),
        trusted: Vec::new(),
    })
}

async fn open() -> FtpConn {
    let mut p = params().expect("REACH_FTP_PORT not set");
    match FtpConn::open(p.clone()).await {
        Ok(c) => c,
        Err(FtpError::Tls { untrusted: Some(fp), .. }) => {
            eprintln!("trusting {fp}");
            p.trusted.push(fp);
            FtpConn::open(p).await.expect("connect with the certificate trusted")
        }
        Err(e) => panic!("connect: {e}"),
    }
}

#[tokio::test]
#[ignore = "needs an FTP server"]
async fn live_ftp_operations() {
    let conn = open().await;
    let base = list::join(&conn.home, &format!("reach-ftp-test-{}", std::process::id()));
    let nested = format!("{base}/a b/c");
    make_dir(&conn, &nested).await.unwrap();
    write_text(&conn, &format!("{nested}/hello.txt"), "hello\nworld\n").await.unwrap();
    assert_eq!(read_text(&conn, &format!("{nested}/hello.txt")).await.unwrap(), "hello\nworld\n");
    touch(&conn, &format!("{base}/empty")).await.unwrap();
    // Touching again keeps it, rename refuses an existing name.
    touch(&conn, &format!("{base}/empty")).await.unwrap();
    assert!(rename(&conn, &format!("{base}/empty"), &format!("{nested}/hello.txt")).await.is_err());
    rename(&conn, &format!("{base}/empty"), &format!("{base}/renamed")).await.unwrap();

    let names: Vec<_> = list_dir(&conn, &base).await.unwrap().into_iter().map(|e| (e.name, e.is_dir)).collect();
    assert_eq!(names, [("a b".to_string(), true), ("renamed".to_string(), false)]);

    // Transfers: a folder up, a file down, several at once.
    let tmp = tempfile::tempdir().unwrap();
    let up = tmp.path().join("up");
    std::fs::create_dir_all(up.join("sub")).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(up.join("big.bin"), &big).unwrap();
    std::fs::write(up.join("sub/small.txt"), b"small").unwrap();
    let never = CancellationToken::new();
    let seen = std::sync::Mutex::new(0u64);
    let progress = |done: u64, _total: u64| *seen.lock().unwrap() = done;
    upload(&conn, up.to_str().unwrap(), &format!("{base}/up"), &never, &progress).await.unwrap();
    let down = tmp.path().join("big.down");
    let (big_path, sub_path) = (format!("{base}/up/big.bin"), format!("{base}/up/sub"));
    let destination = DownloadTarget::Chosen(down.clone());
    let (a, b) = tokio::join!(
        download(&conn, &big_path, &destination, &never, &progress),
        list_dir(&conn, &sub_path),
    );
    assert_eq!(a.unwrap(), big.len() as u64);
    assert_eq!(b.unwrap()[0].name, "small.txt");
    assert_eq!(std::fs::read(&down).unwrap(), big);

    let replacement = tmp.path().join("replacement.bin");
    let contents = vec![0xAB; CHUNK * 3];
    std::fs::write(&replacement, &contents).unwrap();
    let stop = CancellationToken::new();
    let stop_on_progress = |sent, _| {
        if sent >= CHUNK as u64 {
            stop.cancel();
        }
    };
    upload(&conn, replacement.to_str().unwrap(), &big_path, &stop, &stop_on_progress).await.unwrap();
    assert!(stop.is_cancelled());
    assert!(matches!(upload(&conn, up.join("big.bin").to_str().unwrap(), &big_path, &stop, &progress).await, Err(FtpError::Cancelled)));
    download(&conn, &big_path, &destination, &never, &progress).await.unwrap();
    assert_eq!(std::fs::read(&down).unwrap(), contents);

    // A cancelled download leaves nothing behind.
    let stop = CancellationToken::new();
    stop.cancel();
    let gone = tmp.path().join("cancelled");
    assert!(matches!(download(&conn, &format!("{base}/up/big.bin"), &DownloadTarget::Chosen(gone.clone()), &stop, &progress).await, Err(FtpError::Cancelled)));
    assert!(!gone.exists());

    delete(&conn, &base).await.unwrap();
    assert!(!list_dir(&conn, &conn.home).await.unwrap().iter().any(|e| e.path == base));
    conn.close().await;
}
