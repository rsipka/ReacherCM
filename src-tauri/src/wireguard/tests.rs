//! Two tunnels in one process talk to each other over UDP on loopback: one
//! is the "server" (ListenPort, no Endpoint), the other connects to it.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::config::{encode_key, public_key, WgConfig};
use super::stack::Tunnel;

fn key() -> [u8; 32] {
    rand::random()
}

fn pair(port: u16) -> (WgConfig, WgConfig) {
    let (a, b) = (key(), key());
    let server = format!(
        "[Interface]\nPrivateKey = {}\nAddress = 10.66.0.1/24\nListenPort = {port}\n[Peer]\nPublicKey = {}\nAllowedIPs = 10.66.0.2/32\n",
        encode_key(&a),
        encode_key(&public_key(&b))
    );
    let client = format!(
        "[Interface]\nPrivateKey = {}\nAddress = 10.66.0.2/32\n[Peer]\nPublicKey = {}\nEndpoint = 127.0.0.1:{port}\nAllowedIPs = 10.66.0.0/24\n",
        encode_key(&b),
        encode_key(&public_key(&a))
    );
    (WgConfig::parse(&server).unwrap(), WgConfig::parse(&client).unwrap())
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[tokio::test]
async fn handshake_and_echo() {
    let (s, c) = pair(free_udp_port());
    let server = Tunnel::start(&s).await.unwrap();
    let client = Tunnel::start(&c).await.unwrap();
    client.wait_handshake(Duration::from_secs(5)).await.expect("handshake");

    let accept = server.accept(22);
    let connect = client.connect("10.66.0.1", 22);
    let (srv, cli) = tokio::join!(accept, connect);
    let (mut srv, mut cli) = (srv.unwrap(), cli.unwrap());

    cli.write_all(b"hello through the tunnel").await.unwrap();
    let mut got = [0u8; 24];
    srv.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"hello through the tunnel");
    srv.write_all(b"pong").await.unwrap();
    let mut pong = [0u8; 4];
    cli.read_exact(&mut pong).await.unwrap();
    assert_eq!(&pong, b"pong");

    // Closing one side ends the other's stream.
    drop(cli);
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), srv.read_to_end(&mut rest)).await.expect("EOF arrives").unwrap();
    assert!(rest.is_empty());
}

#[tokio::test]
async fn moves_megabytes_both_ways() {
    let (s, c) = pair(free_udp_port());
    let server = Tunnel::start(&s).await.unwrap();
    let client = Tunnel::start(&c).await.unwrap();
    let (srv, cli) = tokio::join!(server.accept(9000), client.connect("10.66.0.1", 9000));
    let (srv, cli) = (srv.unwrap(), cli.unwrap());
    let data: Vec<u8> = (0..8 * 1024 * 1024u32).map(|i| (i * 7 % 251) as u8).collect();

    let (mut sr, mut sw) = tokio::io::split(srv);
    // The server echoes everything back.
    let echo = tokio::spawn(async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = sr.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            sw.write_all(&buf[..n]).await.unwrap();
        }
        sw.shutdown().await.unwrap();
    });
    let (mut cr, mut cw) = tokio::io::split(cli);
    let send = data.clone();
    let writer = tokio::spawn(async move {
        cw.write_all(&send).await.unwrap();
        cw.shutdown().await.unwrap();
    });
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), cr.read_to_end(&mut back)).await.expect("in time").unwrap();
    writer.await.unwrap();
    echo.await.unwrap();
    assert_eq!(back.len(), data.len());
    assert!(back == data, "the data came back unchanged");
    assert!(client.stats().tx_bytes > 8_000_000);
}

#[tokio::test]
async fn refused_and_wrong_key() {
    let (s, c) = pair(free_udp_port());
    let _server = Tunnel::start(&s).await.unwrap();
    let client = Tunnel::start(&c).await.unwrap();
    client.wait_handshake(Duration::from_secs(5)).await.unwrap();
    // Nothing listens on port 23: the server's stack resets the connection.
    let err = client.connect("10.66.0.1", 23).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused, "{err}");

    // A client whose key the server does not know never gets a handshake.
    let mut stranger = c.clone();
    stranger.private_key = key();
    let t = Tunnel::start(&stranger).await.unwrap();
    let err = t.wait_handshake(Duration::from_millis(1500)).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn several_connections_at_once() {
    let (s, c) = pair(free_udp_port());
    let server = std::sync::Arc::new(Tunnel::start(&s).await.unwrap());
    let client = std::sync::Arc::new(Tunnel::start(&c).await.unwrap());
    let mut jobs = Vec::new();
    for i in 0..5u16 {
        let (server, client) = (server.clone(), client.clone());
        jobs.push(tokio::spawn(async move {
            let (srv, cli) = tokio::join!(server.accept(2000 + i), client.connect("10.66.0.1", 2000 + i));
            let (mut srv, mut cli) = (srv.unwrap(), cli.unwrap());
            let msg = format!("connection {i}");
            cli.write_all(msg.as_bytes()).await.unwrap();
            let mut got = vec![0u8; msg.len()];
            srv.read_exact(&mut got).await.unwrap();
            assert_eq!(got, msg.as_bytes());
        }));
    }
    for j in jobs {
        tokio::time::timeout(Duration::from_secs(10), j).await.expect("in time").unwrap();
    }
}

/// Answers one connection on `port` inside the server's tunnel with what it
/// read, upper-cased.
async fn shout_once(server: &Tunnel, port: u16) {
    let mut srv = server.accept(port).await.unwrap();
    let mut buf = [0u8; 5];
    srv.read_exact(&mut buf).await.unwrap();
    srv.write_all(&buf.to_ascii_uppercase()).await.unwrap();
}

#[tokio::test]
async fn local_port_forward_and_socks() {
    let (s, c) = pair(free_udp_port());
    let server = Tunnel::start(&s).await.unwrap();
    super::manager::up("test-forward", &c).await.unwrap();

    // A plain local port into the tunnel, the same one when asked again.
    let local = super::manager::forward("test-forward", "10.66.0.1", 5900).await.unwrap();
    assert_eq!(super::manager::forward("test-forward", "10.66.0.1", 5900).await.unwrap(), local);
    let client = async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", local)).await.unwrap();
        tcp.write_all(b"hello").await.unwrap();
        let mut got = [0u8; 5];
        tcp.read_exact(&mut got).await.unwrap();
        got
    };
    let (got, ()) = tokio::join!(client, shout_once(&server, 5900));
    assert_eq!(&got, b"HELLO");

    // SOCKS5: greeting, then CONNECT to 10.66.0.1:443 by address.
    let socks = super::manager::socks("test-forward").await.unwrap();
    let client = async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", socks)).await.unwrap();
        tcp.write_all(&[5, 1, 0]).await.unwrap();
        let mut hello = [0u8; 2];
        tcp.read_exact(&mut hello).await.unwrap();
        assert_eq!(hello, [5, 0]);
        tcp.write_all(&[5, 1, 0, 1, 10, 66, 0, 1, 0x01, 0xbb]).await.unwrap();
        let mut reply = [0u8; 10];
        tcp.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 0, "connect succeeded");
        tcp.write_all(b"socks").await.unwrap();
        let mut got = [0u8; 5];
        tcp.read_exact(&mut got).await.unwrap();
        got
    };
    let (got, ()) = tokio::join!(client, shout_once(&server, 443));
    assert_eq!(&got, b"SOCKS");

    // Turning the tunnel off closes its local ports.
    super::manager::down("test-forward").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", local)).await.is_err());
    assert!(super::manager::forward("test-forward", "10.66.0.1", 5900).await.is_err());
}
