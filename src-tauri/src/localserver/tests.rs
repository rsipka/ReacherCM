//! The servers against real clients over loopback: a TFTP exchange by hand,
//! FTP and HTTP over raw sockets, SFTP with the app's own SFTP client.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

use super::fsroot::Root;
use super::*;

fn config(kind: ServerKind, root: &Path, allow_write: bool) -> ServerConfig {
    ServerConfig {
        kind,
        root: root.display().to_string(),
        bind: "127.0.0.1".into(),
        port: 1,
        users: vec![ServerUser { username: "alice".into(), password: "s3cret".into() }],
        anonymous: false,
        allow_write,
    }
}

fn shared(kind: ServerKind, root: &Path, allow_write: bool) -> Arc<Shared> {
    let config = config(kind, root, allow_write);
    Arc::new(Shared::new(&config, Root::new(&config.root).unwrap(), Arc::new(|_| {})))
}

fn folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"hello world").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("big.bin"), vec![7u8; 5000]).unwrap();
    dir
}

#[test]
fn normalize_stays_inside() {
    assert_eq!(Root::normalize("/", "a/b").unwrap(), "/a/b");
    assert_eq!(Root::normalize("/a", "../../..").unwrap(), "/");
    assert_eq!(Root::normalize("/a", "../../etc/passwd").unwrap(), "/etc/passwd");
    assert_eq!(Root::normalize("/a", "b\\..\\c").unwrap(), "/a/c");
    assert_eq!(Root::normalize("/a", "/x/./y/").unwrap(), "/x/y");
    assert!(Root::normalize("/", "a\0b").is_err());
}

#[test]
fn real_paths_stay_in_the_folder() {
    let dir = folder();
    let root = Root::new(&dir.path().display().to_string()).unwrap();
    let real = root.real("/../../hello.txt").unwrap();
    assert_eq!(real, root.path().join("hello.txt"));
    assert_eq!(root.virtual_of(&root.path().join("sub")), "/sub");
}

#[cfg(unix)]
#[test]
fn links_out_of_the_folder_are_refused() {
    let dir = folder();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"x").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("hello.txt"), dir.path().join("inside")).unwrap();
    let root = Root::new(&dir.path().display().to_string()).unwrap();
    assert!(root.real("/escape/secret").is_err());
    assert!(root.real("/escape/new-file").is_err());
    assert!(root.real("/inside").is_ok());
}

#[test]
fn logins_match_user_and_password() {
    let dir = folder();
    let s = shared(ServerKind::Ftp, dir.path(), false);
    assert!(s.check_login("alice", "s3cret"));
    assert!(!s.check_login("alice", "wrong"));
    assert!(!s.check_login("bob", "s3cret"));
    assert!(!s.is_anonymous_name("anonymous"));
}

#[test]
fn a_taken_port_gets_a_free_suggestion() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let check = net::check_port(ServerKind::Http, "127.0.0.1".parse().unwrap(), port);
    assert!(!check.free);
    assert_eq!(check.problem, Some(net::PortProblem::InUse));
    let suggestion = check.suggestion.unwrap();
    assert_ne!(suggestion, port);
    assert!(net::try_port(ServerKind::Http, SocketAddr::from(([127, 0, 0, 1], suggestion))).is_ok());
}

#[test]
fn tftp_requests_and_options_parse() {
    let mut packet = vec![0, 1];
    packet.extend_from_slice(b"fw/image.bin\0octet\0blksize\x001428\x00tsize\x000\x00");
    let req = tftp::parse_request(&packet).unwrap();
    assert!(!req.write);
    assert_eq!(req.filename, "fw/image.bin");
    assert_eq!(req.mode, "octet");
    let n = tftp::negotiate(&req, Some(5000));
    assert_eq!(n.block, 1428);
    assert_eq!(n.ack, vec![("blksize".into(), "1428".into()), ("tsize".into(), "5000".into())]);
    assert!(tftp::parse_request(&[0, 3, 0, 1]).is_none());
}

#[test]
fn ftp_addresses_and_listing() {
    assert_eq!(ftp::parse_port("10,0,0,5,23,112").unwrap(), "10.0.0.5:6000".parse().unwrap());
    assert!(ftp::parse_port("10,0,0,5,23").is_none());
    assert_eq!(ftp::parse_eprt("|1|10.0.0.5|6000|").unwrap(), "10.0.0.5:6000".parse().unwrap());
    assert_eq!(ftp::parse_eprt("|2|::1|6000|").unwrap(), "[::1]:6000".parse().unwrap());
    let dir = folder();
    let meta = std::fs::metadata(dir.path().join("hello.txt")).unwrap();
    assert!(ftp::format_entry("LIST", "hello.txt", &meta).starts_with("-rw-r--r-- 1 owner group           11 "));
    assert!(ftp::format_entry("MLSD", "hello.txt", &meta).starts_with("type=file;size=11;modify="));
}

#[test]
fn http_ranges_and_names() {
    assert_eq!(http::parse_range("bytes=0-9", 100), Some(Ok((0, 9))));
    assert_eq!(http::parse_range("bytes=90-", 100), Some(Ok((90, 99))));
    assert_eq!(http::parse_range("bytes=-10", 100), Some(Ok((90, 99))));
    assert_eq!(http::parse_range("bytes=0-500", 100), Some(Ok((0, 99))));
    assert_eq!(http::parse_range("bytes=200-", 100), Some(Err(())));
    assert_eq!(http::parse_range("bytes=0-1,5-6", 100), None);
    assert_eq!(http::percent_decode("/a%20b/%C4%8D").unwrap(), "/a b/č");
    assert!(http::percent_decode("/%zz").is_none());
    assert_eq!(http::html_escape("<a href=\"x\">"), "&lt;a href=&quot;x&quot;&gt;");
}

// ---- Live exchanges over loopback ----

async fn tftp_server(allow_write: bool) -> (tempfile::TempDir, SocketAddr, CancellationToken) {
    let dir = folder();
    let socket = net::bind_udp("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    let cancel = CancellationToken::new();
    tokio::spawn(tftp::run(socket, shared(ServerKind::Tftp, dir.path(), allow_write), cancel.clone()));
    (dir, addr, cancel)
}

async fn recv(sock: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buf = vec![0u8; 70000];
    let (n, from) = tokio::time::timeout(Duration::from_secs(5), sock.recv_from(&mut buf)).await.unwrap().unwrap();
    buf.truncate(n);
    (buf, from)
}

#[tokio::test]
async fn tftp_download_with_block_size() {
    let (_dir, addr, cancel) = tftp_server(false).await;
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut rrq = vec![0, 1];
    rrq.extend_from_slice(b"sub/big.bin\0octet\0blksize\x001024\x00tsize\x000\x00");
    client.send_to(&rrq, addr).await.unwrap();
    let (oack, tid) = recv(&client).await;
    assert_eq!(&oack[..2], &[0, 6]);
    assert!(String::from_utf8_lossy(&oack).contains("5000"));
    client.send_to(&[0, 4, 0, 0], tid).await.unwrap();
    let mut got = Vec::new();
    let mut block = 1u16;
    loop {
        let (data, from) = recv(&client).await;
        assert_eq!(from, tid);
        assert_eq!(&data[..2], &[0, 3]);
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), block);
        got.extend_from_slice(&data[4..]);
        client.send_to(&[0, 4, data[2], data[3]], tid).await.unwrap();
        if data.len() - 4 < 1024 {
            break;
        }
        block += 1;
    }
    assert_eq!(got, vec![7u8; 5000]);
    cancel.cancel();
}

#[tokio::test]
async fn tftp_upload_needs_write_and_works_with_it() {
    let (dir, addr, cancel) = tftp_server(false).await;
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(b"\0\x02new.cfg\0octet\0", addr).await.unwrap();
    let (err, _) = recv(&client).await;
    assert_eq!(&err[..4], &[0, 5, 0, 2]);
    assert!(!dir.path().join("new.cfg").exists());
    cancel.cancel();

    let (dir, addr, cancel) = tftp_server(true).await;
    client.send_to(b"\0\x02new.cfg\0octet\0", addr).await.unwrap();
    let (ack, tid) = recv(&client).await;
    assert_eq!(ack, vec![0, 4, 0, 0]);
    client.send_to(b"\0\x03\0\x01hostname sw1\n", tid).await.unwrap();
    let (ack, _) = recv(&client).await;
    assert_eq!(ack, vec![0, 4, 0, 1]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(std::fs::read(dir.path().join("new.cfg")).unwrap(), b"hostname sw1\n");
    cancel.cancel();
}

#[tokio::test]
async fn tftp_refuses_paths_out_of_the_folder() {
    let (_dir, addr, cancel) = tftp_server(false).await;
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(b"\0\x01../../../etc/passwd\0octet\0", addr).await.unwrap();
    let (err, _) = recv(&client).await;
    // "/etc/passwd" inside the folder, which does not exist there.
    assert_eq!(&err[..4], &[0, 5, 0, 1]);
    cancel.cancel();
}

struct FtpClient {
    r: BufReader<TcpStream>,
}

impl FtpClient {
    async fn connect(addr: SocketAddr) -> Self {
        let mut c = Self { r: BufReader::new(TcpStream::connect(addr).await.unwrap()) };
        assert!(c.line().await.starts_with("220"));
        c
    }
    async fn line(&mut self) -> String {
        let mut s = String::new();
        tokio::time::timeout(Duration::from_secs(5), self.r.read_line(&mut s)).await.unwrap().unwrap();
        s
    }
    async fn cmd(&mut self, c: &str) -> String {
        self.r.get_mut().write_all(format!("{c}\r\n").as_bytes()).await.unwrap();
        let mut reply = self.line().await;
        // Multi-line replies end with "NNN " again.
        if reply.len() > 3 && &reply[3..4] == "-" {
            let code = reply[..3].to_string();
            loop {
                let l = self.line().await;
                reply.push_str(&l);
                if l.starts_with(&format!("{code} ")) {
                    break;
                }
            }
        }
        reply
    }
    async fn pasv(&mut self) -> TcpStream {
        let reply = self.cmd("EPSV").await;
        assert!(reply.starts_with("229"), "{reply}");
        let port: u16 = reply.split("|||").nth(1).unwrap().split('|').next().unwrap().parse().unwrap();
        TcpStream::connect(("127.0.0.1", port)).await.unwrap()
    }
}

async fn ftp_server(allow_write: bool) -> (tempfile::TempDir, SocketAddr, CancellationToken) {
    let dir = folder();
    let listener = net::bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    tokio::spawn(ftp::run(listener, shared(ServerKind::Ftp, dir.path(), allow_write), cancel.clone()));
    (dir, addr, cancel)
}

#[tokio::test]
async fn ftp_login_list_download_upload() {
    let (dir, addr, cancel) = ftp_server(true).await;
    let mut c = FtpClient::connect(addr).await;
    assert!(c.cmd("LIST").await.starts_with("530"));
    assert!(c.cmd("USER alice").await.starts_with("331"));
    assert!(c.cmd("PASS wrong").await.starts_with("530"));
    c.cmd("USER alice").await;
    assert!(c.cmd("PASS s3cret").await.starts_with("230"));
    assert!(c.cmd("FEAT").await.contains("EPSV"));
    assert!(c.cmd("PWD").await.starts_with("257 \"/\""));

    let mut data = c.pasv().await;
    assert!(c.cmd("MLSD").await.starts_with("150"));
    let mut listing = String::new();
    data.read_to_string(&mut listing).await.unwrap();
    assert!(c.line().await.starts_with("226"));
    assert!(listing.contains("type=file;size=11;"), "{listing}");
    assert!(listing.contains("type=dir;"), "{listing}");

    assert!(c.cmd("CWD sub").await.starts_with("250"));
    assert!(c.cmd("SIZE big.bin").await.starts_with("213 5000"));
    assert!(c.cmd("CWD ../..").await.starts_with("250"));
    assert!(c.cmd("PWD").await.starts_with("257 \"/\""));

    let mut data = c.pasv().await;
    assert!(c.cmd("RETR hello.txt").await.starts_with("150"));
    let mut body = Vec::new();
    data.read_to_end(&mut body).await.unwrap();
    assert!(c.line().await.starts_with("226"));
    assert_eq!(body, b"hello world");

    let mut data = c.pasv().await;
    assert!(c.cmd("STOR up.txt").await.starts_with("150"));
    data.write_all(b"uploaded").await.unwrap();
    data.shutdown().await.unwrap();
    drop(data);
    assert!(c.line().await.starts_with("226"));
    assert_eq!(std::fs::read(dir.path().join("up.txt")).unwrap(), b"uploaded");

    assert!(c.cmd("RNFR up.txt").await.starts_with("350"));
    assert!(c.cmd("RNTO sub/moved.txt").await.starts_with("250"));
    assert!(dir.path().join("sub/moved.txt").exists());
    assert!(c.cmd("MKD newdir").await.starts_with("257"));
    assert!(c.cmd("RMD newdir").await.starts_with("250"));
    assert!(c.cmd("PORT 10,0,0,99,23,112").await.starts_with("504"));
    assert!(c.cmd("QUIT").await.starts_with("221"));
    cancel.cancel();
}

#[tokio::test]
async fn ftp_read_only_refuses_changes() {
    let (dir, addr, cancel) = ftp_server(false).await;
    let mut c = FtpClient::connect(addr).await;
    c.cmd("USER alice").await;
    c.cmd("PASS s3cret").await;
    assert!(c.cmd("DELE hello.txt").await.starts_with("550"));
    assert!(c.cmd("STOR x.txt").await.starts_with("550"));
    assert!(c.cmd("MKD d").await.starts_with("550"));
    assert!(dir.path().join("hello.txt").exists());
    cancel.cancel();
}

async fn http_server(allow_write: bool, anonymous: bool) -> (tempfile::TempDir, SocketAddr, CancellationToken) {
    let dir = folder();
    let mut config = config(ServerKind::Http, dir.path(), allow_write);
    config.anonymous = anonymous;
    let shared = Arc::new(Shared::new(&config, Root::new(&config.root).unwrap(), Arc::new(|_| {})));
    let listener = net::bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    tokio::spawn(http::run(listener, shared, cancel.clone()));
    (dir, addr, cancel)
}

async fn http(addr: SocketAddr, request: &str, body: &[u8]) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await.unwrap().unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn http_login_listing_range_and_upload() {
    let (dir, addr, cancel) = http_server(true, false).await;
    let r = http(addr, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", b"").await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(r.contains("www-authenticate: Basic"), "{r}");

    // alice:s3cret
    let auth = "Authorization: Basic YWxpY2U6czNjcmV0\r\n";
    let r = http(addr, &format!("GET / HTTP/1.1\r\nHost: x\r\n{auth}Connection: close\r\n\r\n"), b"").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains("href=\"hello.txt\"") && r.contains("href=\"sub/\""), "{r}");

    let r = http(addr, &format!("GET /sub HTTP/1.1\r\nHost: x\r\n{auth}Connection: close\r\n\r\n"), b"").await;
    assert!(r.starts_with("HTTP/1.1 301") && r.contains("location: /sub/"), "{r}");

    let r = http(addr, &format!("GET /hello.txt HTTP/1.1\r\nHost: x\r\n{auth}Range: bytes=6-\r\nConnection: close\r\n\r\n"), b"").await;
    assert!(r.starts_with("HTTP/1.1 206"), "{r}");
    assert!(r.ends_with("\r\n\r\nworld"), "{r}");

    let r = http(addr, &format!("GET /%2e%2e/%2e%2e/etc/passwd HTTP/1.1\r\nHost: x\r\n{auth}Connection: close\r\n\r\n"), b"").await;
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");

    let r = http(addr, &format!("PUT /sub/new%20file.txt HTTP/1.1\r\nHost: x\r\n{auth}Content-Length: 4\r\nConnection: close\r\n\r\n"), b"data").await;
    assert!(r.starts_with("HTTP/1.1 201"), "{r}");
    assert_eq!(std::fs::read(dir.path().join("sub/new file.txt")).unwrap(), b"data");
    cancel.cancel();

    let (dir, addr, cancel) = http_server(false, true).await;
    let r = http(addr, "GET /hello.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", b"").await;
    assert!(r.ends_with("hello world"), "{r}");
    let r = http(addr, "PUT /x.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nConnection: close\r\n\r\n", b"x").await;
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");
    assert!(!dir.path().join("x.txt").exists());
    cancel.cancel();
}

struct AnyHost;

impl russh::client::Handler for AnyHost {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

#[tokio::test]
async fn sftp_login_list_download_upload() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = folder();
    let listener = net::bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let seed: [u8; 32] = rand::random();
    let key = russh::keys::PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed));
    let config = config(ServerKind::Sftp, dir.path(), true);
    let shared = Arc::new(Shared::new(&config, Root::new(&config.root).unwrap(), Arc::new(|_: &LogEntry| {})));
    tokio::spawn(sftp::run(listener, key, shared, cancel.clone()));

    let config = Arc::new(russh::client::Config::default());
    let mut ssh = russh::client::connect(config, addr, AnyHost).await.unwrap();
    assert!(!ssh.authenticate_password("alice", "wrong").await.unwrap().success());
    assert!(ssh.authenticate_password("alice", "s3cret").await.unwrap().success());

    // No shell.
    let ch = ssh.channel_open_session().await.unwrap();
    ch.exec(true, "id").await.unwrap();
    let mut ch = ch;
    let mut refused = false;
    while let Some(msg) = tokio::time::timeout(Duration::from_secs(5), ch.wait()).await.unwrap() {
        if matches!(msg, russh::ChannelMsg::Failure) {
            refused = true;
            break;
        }
    }
    assert!(refused);

    let ch = ssh.channel_open_session().await.unwrap();
    ch.request_subsystem(true, "sftp").await.unwrap();
    let sftp = russh_sftp::client::SftpSession::new(ch.into_stream()).await.unwrap();
    assert_eq!(sftp.canonicalize(".").await.unwrap(), "/");
    let names: Vec<String> = sftp.read_dir("/").await.unwrap().map(|e| e.file_name()).collect();
    assert!(names.contains(&"hello.txt".to_string()) && names.contains(&"sub".to_string()), "{names:?}");
    assert_eq!(sftp.read("/hello.txt").await.unwrap(), b"hello world");
    assert!(sftp.read("/../../etc/hostname").await.is_err());
    let mut f = sftp.create("/sub/put.txt").await.unwrap();
    f.write_all(b"via sftp").await.unwrap();
    f.shutdown().await.unwrap();
    drop(f);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(std::fs::read(dir.path().join("sub/put.txt")).unwrap(), b"via sftp");
    sftp.rename("/sub/put.txt", "/put2.txt").await.unwrap();
    assert!(dir.path().join("put2.txt").exists());
    sftp.remove_file("/put2.txt").await.unwrap();
    assert!(!dir.path().join("put2.txt").exists());
    cancel.cancel();
}

#[tokio::test]
async fn manager_starts_checks_and_stops() {
    let dir = folder();
    let mut manager = LocalServerManager::default();
    let mut cfg = config(ServerKind::Http, dir.path(), false);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    cfg.port = port;
    let status = manager.start(cfg.clone(), Arc::new(|_| {})).await.unwrap();
    assert!(status.running);
    assert_eq!(status.addresses, vec!["127.0.0.1".to_string()]);
    assert!(manager.holds(ServerKind::Http, port));
    // Taken now, by our own server.
    assert!(!net::check_port(ServerKind::Http, "127.0.0.1".parse().unwrap(), port).free);
    manager.stop(ServerKind::Http);
    let status = manager.status(ServerKind::Http);
    assert!(!status.running);
    assert!(status.log.iter().any(|l| l.message == "Stopped"));

    cfg.users.clear();
    assert!(manager.start(cfg.clone(), Arc::new(|_| {})).await.is_err());
    cfg.root = dir.path().join("nope").display().to_string();
    assert!(manager.start(cfg, Arc::new(|_| {})).await.is_err());
}
