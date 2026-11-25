//! A Proxmox VE mock over HTTPS with a self-signed certificate: the login
//! (password, then TOTP), an API token, the pages, a power button, a task
//! log, and a text console over its websocket.

use super::*;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_tungstenite::tungstenite::Message;

// A self-signed test certificate for localhost; DER, as *.pem is ignored by git.
const CERT: &[u8] = include_bytes!("testdata/cert.der");
const KEY: &[u8] = include_bytes!("testdata/key.der");
const FINGERPRINT: &str = "D0:D6:4D:85:26:A4:69:2B:1C:29:BA:7A:D8:C4:0A:E7:82:43:88:F1:DB:CB:00:EE:D3:F9:B6:0C:0B:0C:F3:67";
const FULL: &str = "PVE:alice@pam:6750A1B2::full";
const TOKEN: &str = "PVEAPIToken=alice@pam!ci=11111111-2222-3333-4444-555555555555";
const PBS_TOKEN: &str = "PBSAPIToken=alice@pbs!ci:11111111-2222-3333-4444-555555555555";
const GC_UPID: &str = "UPID:pbs1:000004D2:0000162E:00000003:6750A1B2:garbage_collection:store1:alice@pbs!ci:";

/// Bytes already read, then the stream.
struct Prefixed<S> {
    head: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.pos < self.head.len() {
            let n = (self.head.len() - self.pos).min(buf.remaining());
            let p = self.pos;
            buf.put_slice(&self.head[p..p + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, b: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct Req {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn by_ticket(&self) -> bool {
        self.header("cookie").is_some_and(|c| c == format!("PVEAuthCookie={FULL}"))
    }
    fn by_token(&self) -> bool {
        self.header("authorization") == Some(TOKEN) || self.header("authorization") == Some(PBS_TOKEN)
    }
}

/// The answer to a request: status, reason, body.
fn answer(r: &Req) -> (u16, &'static str, Value) {
    let p = r.path.strip_prefix("/api2/json").unwrap_or("");
    if p == "/access/ticket" && r.method == "POST" {
        let user = r.body["username"].as_str().unwrap_or_default();
        let pass = r.body["password"].as_str().unwrap_or_default();
        return match (user, pass, r.body["tfa-challenge"].as_str()) {
            ("alice@pam", "secret", None) => (200, "OK", json!({"data": {"username": "alice@pam", "ticket": "PVE:alice@pam:6750A1B2::!tfa!challenge", "NeedTFA": 1}})),
            ("alice@pam", "totp:123456", Some(c)) if c.contains("challenge") => {
                (200, "OK", json!({"data": {"username": "alice@pam", "ticket": FULL, "CSRFPreventionToken": "csrf1"}}))
            }
            ("alice@pam", p, None) if p == FULL => (200, "OK", json!({"data": {"username": "alice@pam", "ticket": FULL, "CSRFPreventionToken": "csrf1"}})),
            _ => (401, "authentication failure", json!({"data": null})),
        };
    }
    if !(r.by_ticket() || r.by_token()) {
        return (401, "No ticket", json!({"data": null}));
    }
    if r.method != "GET" && r.by_ticket() && r.header("csrfpreventiontoken") != Some("csrf1") {
        return (401, "Permission denied - invalid csrf token", json!({"data": null}));
    }
    let data = match (r.method.as_str(), p) {
        ("GET", "/version") => json!({"version": "9.2.1", "release": "9.2", "repoid": "abc"}),
        ("GET", "/cluster/status") => json!([{"type": "node", "name": "pve1", "online": 1, "ip": "10.0.0.2", "local": 1}]),
        ("GET", "/cluster/resources") => json!([
            {"type": "qemu", "id": "qemu/100", "vmid": 100, "node": "pve1", "name": "win11", "status": "running", "cpu": 0.12, "maxcpu": 4, "mem": 4294967296u64, "maxmem": 8589934592u64, "uptime": 3600, "tags": "lab;windows", "template": 0},
            {"type": "lxc", "id": "lxc/101", "vmid": 101, "node": "pve1", "name": "dns", "status": "running", "cpu": 0.01, "maxcpu": 1, "mem": 104857600, "maxmem": 536870912},
            {"type": "qemu", "id": "qemu/9000", "vmid": 9000, "node": "pve1", "name": "tpl", "status": "stopped", "template": 1},
            {"type": "node", "id": "node/pve1", "node": "pve1", "status": "online", "cpu": 0.05, "maxcpu": 16, "mem": 1, "maxmem": 2, "uptime": 99},
            {"type": "storage", "id": "storage/pve1/local", "storage": "local", "node": "pve1", "status": "available", "disk": 10, "maxdisk": 100, "plugintype": "dir", "content": "iso,vztmpl,backup", "shared": 0}
        ]),
        ("GET", "/nodes/pve1/status") => json!({"pveversion": "pve-manager/9.2.1/abc", "kversion": "Linux 6.14.8-2-pve #1 SMP", "current-kernel": {"release": "6.14.8-2-pve"}, "cpuinfo": {"model": "AMD Ryzen 7", "cpus": 16, "sockets": 1}, "cpu": 0.05, "loadavg": ["0.10", "0.20", "0.30"], "memory": {"used": 1, "total": 2}, "rootfs": {"used": 3, "total": 4}, "uptime": 99}),
        ("GET", "/nodes/pve1/qemu/100/status/current") => json!({"status": "running", "qmpstatus": "running", "name": "win11", "cpus": 4, "agent": 1, "ha": {"managed": 0}}),
        ("GET", "/nodes/pve1/qemu/100/config") => json!({"name": "win11", "vga": "qxl", "agent": "1,fstrim_cloned_disks=1", "serial0": "socket", "ostype": "win11", "onboot": 1, "tags": "lab;windows"}),
        ("GET", "/nodes/pve1/qemu/100/snapshot") => json!([{"name": "current", "running": 1}, {"name": "before_update", "snaptime": 1700000000, "description": "x", "vmstate": 1}]),
        ("GET", "/nodes/pve1/qemu/100/agent/network-get-interfaces") => json!({"result": [{"name": "lo", "ip-addresses": [{"ip-address": "127.0.0.1"}]}, {"name": "Ethernet", "ip-addresses": [{"ip-address": "10.0.0.50"}, {"ip-address": "fe80::1"}]}]}),
        ("POST", "/nodes/pve1/qemu/100/status/shutdown") => json!("UPID:pve1:0000ABCD:00112233:6750A1B2:qmshutdown:100:alice@pam:"),
        ("GET", "/cluster/tasks") => json!([
            {"upid": "UPID:pve1:0000ABCD:00112233:6750A1B2:qmshutdown:100:alice@pam:", "node": "pve1", "type": "qmshutdown", "id": "100", "user": "alice@pam", "starttime": 1733000000},
            {"upid": "UPID:pve1:00000001:00000001:6750A000:vzdump::root@pam:", "node": "pve1", "type": "vzdump", "user": "root@pam", "starttime": 1732990000, "endtime": 1732990100, "status": "OK"}
        ]),
        ("GET", p) if p.ends_with("/log") && p.contains("qmshutdown") => {
            assert!(r.query.contains("start=0"), "{}", r.query);
            json!([{"n": 1, "t": "shutting down"}, {"n": 2, "t": "waiting"}])
        }
        ("GET", p) if p.ends_with("/status") && p.contains("qmshutdown") => json!({"status": "running"}),
        ("POST", "/nodes/pve1/lxc/101/termproxy") => {
            if !r.by_ticket() {
                return (401, "invalid ticket", json!({"data": null}));
            }
            json!({"port": 5900, "ticket": "PVEVNC:termticket", "user": "alice@pam", "upid": "UPID:pve1:1:1:1:vncproxy:101:alice@pam:"})
        }
        // A Backup Server.
        ("GET", "/nodes/localhost/status") => json!({"cpu": 0.02, "loadavg": [0.5, 0.4, 0.3], "memory": {"used": 5, "total": 8}, "swap": {"used": 0, "total": 0}, "root": {"used": 2, "total": 9, "avail": 7}, "uptime": 600, "kversion": "Linux 6.14.8-2-pve", "cpuinfo": {"model": "QEMU Virtual CPU", "cpus": 4, "sockets": 1}, "info": {"fingerprint": "aa"}}),
        ("GET", "/admin/datastore") => json!([{"name": "store1", "comment": "main", "path": "/mnt/datastore/store1"}, {"name": "usb", "maintenance-mode": "type=offline,message=unplugged"}]),
        ("GET", "/status/datastore-usage") => json!([{"store": "store1", "total": 1000, "used": 400, "avail": 600, "estimated-full-date": 1800000000, "gc-status": {"removed-bytes": 10, "pending-bytes": 5}}, {"store": "usb", "error": "datastore is not mounted"}]),
        ("GET", "/admin/datastore/store1/namespace") => json!([{"ns": ""}, {"ns": "lab"}, {"ns": "lab/old"}]),
        ("GET", "/admin/datastore/store1/groups") => {
            assert_eq!(r.query, "ns=lab");
            json!([{"backup-type": "vm", "backup-id": "100", "last-backup": 1733000000, "backup-count": 2, "owner": "alice@pbs"}, {"backup-type": "ct", "backup-id": "101", "last-backup": 1733000100, "backup-count": 1}, {"backup-type": "vm", "backup-id": "../x", "backup-count": 1}])
        }
        ("GET", "/admin/datastore/store1/snapshots") => {
            assert!(r.query.contains("backup-type=vm") && r.query.contains("backup-id=100") && r.query.contains("ns=lab"), "{}", r.query);
            json!([
                {"backup-type": "vm", "backup-id": "100", "backup-time": 1732000000, "size": 2048, "protected": true, "verification": {"state": "ok", "upid": "x"}, "files": [{"filename": "index.json.blob", "crypt-mode": "none"}]},
                {"backup-type": "vm", "backup-id": "100", "backup-time": 1733000000, "size": 4096, "protected": false, "files": [{"filename": "drive-scsi0.img.fidx", "crypt-mode": "encrypt"}]}
            ])
        }
        ("POST", "/admin/datastore/store1/gc") => json!(GC_UPID),
        ("POST", "/admin/datastore/store1/verify") => {
            assert_eq!(r.body, json!({"backup-type": "vm", "backup-id": "100", "backup-time": 1733000000u64, "ns": "lab"}));
            json!("UPID:pbs1:000004D3:0000162F:00000004:6750A1B3:verify_snapshot:store1:alice@pbs!ci:")
        }
        ("PUT", "/admin/datastore/store1/protected") => {
            assert_eq!(r.body, json!({"backup-type": "vm", "backup-id": "100", "backup-time": 1733000000u64, "protected": true, "ns": "lab"}));
            Value::Null
        }
        ("DELETE", "/admin/datastore/store1/snapshots") => {
            assert!(r.query.contains("backup-time=1732000000") && r.query.contains("ns=lab"), "{}", r.query);
            Value::Null
        }
        ("GET", "/nodes/localhost/tasks") => json!([
            {"upid": GC_UPID, "node": "pbs1", "worker_type": "garbage_collection", "worker_id": "store1", "user": "alice@pbs!ci", "starttime": 1733000000},
            {"upid": "UPID:pbs1:00000001:00000001:00000001:6750A000:backup:store1%3Avm%2F100:alice@pbs:", "node": "pbs1", "worker_type": "backup", "worker_id": "store1:vm/100", "starttime": 1732990000, "endtime": 1732990100, "status": "OK"}
        ]),
        ("GET", p) if p.starts_with("/nodes/localhost/tasks/") && p.ends_with("/log") => json!([{"n": 1, "t": "starting garbage collection on store store1"}]),
        ("GET", p) if p.starts_with("/nodes/localhost/tasks/") && p.ends_with("/status") => json!({"status": "stopped", "exitstatus": "OK"}),
        _ => return (501, "Method not implemented", json!({"data": null})),
    };
    (200, "OK", json!({ "data": data }))
}

async fn serve_one<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(mut s: S) {
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    let end = loop {
        let n = match s.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        head.extend_from_slice(&buf[..n]);
        if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let text = String::from_utf8_lossy(&head[..end]).to_string();
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_string();
    let target = first.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let (path, query) = target.split_once('?').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((target.clone(), String::new()));
    let upgrade = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("upgrade") && v.eq_ignore_ascii_case("websocket"));
    if upgrade {
        // The console: termproxy's login, then what is typed comes back.
        let ok_cookie = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("cookie") && v == &format!("PVEAuthCookie={FULL}"));
        let ok_proto = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("sec-websocket-protocol") && v == "binary");
        assert!(ok_cookie && ok_proto && path == "/api2/json/nodes/pve1/lxc/101/vncwebsocket" && query.contains("port=5900"), "{text}");
        let stream = Prefixed { head, pos: 0, inner: s };
        // The callback's type is tungstenite's, large error and all.
        #[allow(clippy::result_large_err)]
        let cb = |_req: &tokio_tungstenite::tungstenite::handshake::server::Request, mut res: tokio_tungstenite::tungstenite::handshake::server::Response| {
            res.headers_mut().insert("Sec-WebSocket-Protocol", "binary".parse().unwrap());
            Ok(res)
        };
        let mut ws = tokio_tungstenite::accept_hdr_async(stream, cb).await.unwrap();
        let mut logged_in = false;
        while let Some(Ok(m)) = ws.next().await {
            let data = match m {
                Message::Binary(b) => b.to_vec(),
                Message::Text(t) => t.as_bytes().to_vec(),
                _ => continue,
            };
            if !logged_in {
                assert_eq!(data, b"alice@pam:PVEVNC:termticket\n");
                logged_in = true;
                ws.send(Message::binary(b"OKwelcome\r\n".to_vec())).await.unwrap();
                continue;
            }
            // "0:<len>:<bytes>" back as output; resize as a line.
            let s = String::from_utf8_lossy(&data).to_string();
            let reply = if let Some(rest) = s.strip_prefix("0:") {
                rest.split_once(':').map(|(_, d)| d.to_string()).unwrap_or_default()
            } else {
                format!("[{s}]")
            };
            ws.send(Message::binary(reply.into_bytes())).await.unwrap();
        }
        return;
    }
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = head[end..].to_vec();
    while body.len() < len {
        let n = s.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    let req = Req { method, path, query, headers, body: serde_json::from_slice(&body).unwrap_or(Value::Null) };
    let (code, reason, value) = answer(&req);
    let out = value.to_string();
    let res = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
    let _ = s.write_all(res.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// Start the mock; its port.
async fn mock() -> u16 {
    let certs = vec![CertificateDer::from(CERT.to_vec())];
    let key = PrivatePkcs8KeyDer::from(KEY.to_vec()).into();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    serve_one(tls).await;
                }
            });
        }
    });
    port
}

fn client(port: u16, trusted: Vec<String>) -> Client {
    Client::new(&Params { host: "127.0.0.1".into(), port, product: Product::Pve, trusted }).unwrap()
}

#[tokio::test]
async fn an_unknown_certificate_is_asked_about() {
    let port = mock().await;
    let c = client(port, vec![]);
    let login = UserLogin { user: "alice@pam".into(), password: "secret".into(), otp: None };
    match c.ticket_login(&login).await {
        Err(ProxmoxError::UntrustedCert(fp)) => assert_eq!(fp, FINGERPRINT),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn password_then_totp_then_the_pages() {
    let port = mock().await;
    let c = client(port, vec![FINGERPRINT.into()]);
    let mut login = UserLogin { user: "alice@pam".into(), password: "secret".into(), otp: None };
    assert!(matches!(c.ticket_login(&login).await, Err(ProxmoxError::TfaNeeded)));
    login.otp = Some("000000".into());
    assert!(matches!(c.ticket_login(&login).await, Err(ProxmoxError::Login(_))));
    login.otp = Some("123 456".into());
    let t = c.ticket_login(&login).await.unwrap();
    assert_eq!(t.user, "alice@pam");
    let conn = ProxmoxConn::open(c, "s1".into(), Auth::Ticket(t), "alice@pam".into()).await.unwrap();
    let opened = conn.opened().await;
    assert_eq!((opened.product, opened.version.as_deref(), opened.token), (Product::Pve, Some("9.2.1"), false));

    let o = conn.overview().await.unwrap();
    assert_eq!(o.nodes.len(), 1);
    assert_eq!(o.nodes[0].pve_version.as_deref(), Some("9.2.1"));
    assert_eq!(o.nodes[0].kernel.as_deref(), Some("6.14.8-2-pve"));
    assert_eq!(o.nodes[0].load, vec![0.1, 0.2, 0.3]);

    let g = conn.guest("pve1", "qemu", 100).await.unwrap();
    assert!(g.spice && g.agent && g.onboot);
    assert_eq!(g.serials, vec!["serial0"]);
    assert_eq!(g.addresses, vec!["10.0.0.50"]);
    assert_eq!(g.snapshots.len(), 1);
    assert_eq!(g.snapshots[0].name, "before_update");

    // Guests it did not list are refused before anything is sent.
    assert!(conn.guest("pve1", "qemu", 555).await.is_err());
    assert!(conn.guest("../x", "qemu", 100).await.is_err());
    assert!(conn.guest_action("pve1", "qemu", 100, "destroy").await.is_err());

    // A change carries the CSRF token (the mock refuses one without).
    let upid = conn.guest_action("pve1", "qemu", 100, "shutdown").await.unwrap();
    assert!(upid.contains("qmshutdown"));
    let log = conn.task_log("pve1", &upid, 0).await.unwrap();
    assert_eq!(log.lines, vec!["shutting down", "waiting"]);
    assert!(log.running);
    assert_eq!(log.next, 2);
    assert!(conn.task_log("pve2", &upid, 0).await.is_err());

    let tasks = conn.tasks().await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks[0].running);
    assert_eq!(tasks[1].status.as_deref(), Some("OK"));
}

#[tokio::test]
async fn a_token_runs_the_pages_but_a_console_needs_a_login() {
    let port = mock().await;
    let c = client(port, vec![FINGERPRINT.into()]);
    let header = Product::Pve.token_header("alice@pam!ci", "11111111-2222-3333-4444-555555555555");
    let conn = ProxmoxConn::open(c, "s1".into(), Auth::Token { header }, "alice@pam!ci".into()).await.unwrap();
    assert!(conn.opened().await.token);
    assert_eq!(conn.resources().await.unwrap().len(), 5);
    let target = ConsoleTarget { session_id: "s1".into(), node: "pve1".into(), kind: "lxc".into(), vmid: Some(101), serial: None };
    assert!(matches!(conn.term(&target).await, Err(ProxmoxError::ConsoleLoginNeeded)));

    // A user login for the consoles, then the container's console.
    let login = UserLogin { user: "alice@pam".into(), password: "secret".into(), otp: Some("123456".into()) };
    let t = conn.client.ticket_login(&login).await.unwrap();
    conn.client.set_console(t).await;
    let mut link = conn.term(&target).await.unwrap();
    let mut welcome = [0u8; 9];
    link.read_exact(&mut welcome).await.unwrap();
    assert_eq!(&welcome, b"welcome\r\n");
    link.write_all(&ws::term_input(b"ls\r")).await.unwrap();
    let mut echo = [0u8; 3];
    link.read_exact(&mut echo).await.unwrap();
    assert_eq!(&echo, b"ls\r");
    link.write_all(&ws::term_resize(100, 30)).await.unwrap();
    let mut resized = [0u8; 11];
    link.read_exact(&mut resized).await.unwrap();
    assert_eq!(&resized, b"[1:100:30:]");
}

#[tokio::test]
async fn a_wrong_token_is_a_refused_login() {
    let port = mock().await;
    let c = client(port, vec![FINGERPRINT.into()]);
    let header = Product::Pve.token_header("alice@pam!ci", "wrong");
    assert!(matches!(ProxmoxConn::open(c, "s1".into(), Auth::Token { header }, "x".into()).await, Err(ProxmoxError::Login(_))));
}

#[tokio::test]
async fn a_backup_server_shows_its_datastores_and_backups() {
    let port = mock().await;
    let c = Client::new(&Params { host: "127.0.0.1".into(), port, product: Product::Pbs, trusted: vec![FINGERPRINT.into()] }).unwrap();
    let header = Product::Pbs.token_header("alice@pbs!ci", "11111111-2222-3333-4444-555555555555");
    let conn = ProxmoxConn::open(c, "s2".into(), Auth::Token { header }, "alice@pbs!ci".into()).await.unwrap();
    assert_eq!(conn.opened().await.product, Product::Pbs);

    // The Proxmox VE pages are not a Backup Server's.
    assert!(conn.resources().await.is_err());

    let o = conn.pbs_overview().await.unwrap();
    assert_eq!((o.node.root_used, o.node.root_total, o.node.cpus), (Some(2), Some(9), Some(4)));
    assert_eq!(o.node.load, vec![0.5, 0.4, 0.3]);
    assert_eq!(o.datastores.len(), 2);
    assert_eq!((o.datastores[0].store.as_str(), o.datastores[0].used, o.datastores[0].full_at), ("store1", Some(400), Some(1800000000)));
    assert_eq!(o.datastores[0].gc_pending, Some(5));
    assert_eq!(o.datastores[1].maintenance.as_deref(), Some("offline"));
    assert_eq!(o.datastores[1].error.as_deref(), Some("datastore is not mounted"));

    assert_eq!(conn.pbs_namespaces("store1").await.unwrap(), vec!["", "lab", "lab/old"]);
    let g = conn.pbs_groups("store1", "lab").await.unwrap();
    assert_eq!(g.len(), 2, "the group with a bad id is left out");
    assert_eq!((g[0].kind.as_str(), g[0].id.as_str(), g[0].count), ("vm", "100", 2));
    let snaps = conn.pbs_snapshots("store1", "lab", "vm", "100").await.unwrap();
    assert_eq!(snaps[0].time, 1733000000, "newest first");
    assert!(snaps[0].encrypted && !snaps[0].protected && snaps[0].verified.is_none());
    assert!(snaps[1].protected && !snaps[1].encrypted);
    assert_eq!(snaps[1].verified.as_deref(), Some("ok"));

    // Datastores it did not list, and names that are not names, are refused.
    assert!(conn.pbs_groups("other", "").await.is_err());
    assert!(conn.pbs_groups("store1", "../x").await.is_err());
    assert!(conn.pbs_snapshots("store1", "", "disk", "100").await.is_err());

    assert_eq!(conn.pbs_gc("store1").await.unwrap(), GC_UPID);
    assert!(conn.pbs_verify("store1", "lab", Some(("vm", "100", 1733000000))).await.unwrap().contains("verify_snapshot"));
    conn.pbs_protect("store1", "lab", "vm", "100", 1733000000, true).await.unwrap();
    conn.pbs_forget("store1", "lab", "vm", "100", 1732000000).await.unwrap();

    // Tasks: PBS's field names, read through localhost.
    let tasks = conn.tasks().await.unwrap();
    assert_eq!((tasks[0].kind.as_str(), tasks[0].id.as_deref(), tasks[0].node.as_str()), ("garbage_collection", Some("store1"), "pbs1"));
    assert!(tasks[0].running);
    let log = conn.task_log("pbs1", GC_UPID, 0).await.unwrap();
    assert_eq!(log.lines, vec!["starting garbage collection on store store1"]);
    assert_eq!(log.exitstatus.as_deref(), Some("OK"));

    // Only the node's shell, and only as localhost.
    let guest = ConsoleTarget { session_id: "s2".into(), node: "localhost".into(), kind: "lxc".into(), vmid: Some(101), serial: None };
    assert!(conn.term(&guest).await.is_err());
    let other = ConsoleTarget { session_id: "s2".into(), node: "pbs1".into(), kind: "node".into(), vmid: None, serial: None };
    assert!(conn.term(&other).await.is_err());
    let shell = ConsoleTarget { node: "localhost".into(), ..other };
    assert!(matches!(conn.term(&shell).await, Err(ProxmoxError::ConsoleLoginNeeded)));
}
