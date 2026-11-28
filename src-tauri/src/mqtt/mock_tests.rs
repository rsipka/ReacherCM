//! The client against a small broker on this computer.

use super::*;
use tokio::net::TcpListener;

/// The broker's side: whole packets in, as (first byte, body).
async fn read_packet<R: tokio::io::AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> (u8, Vec<u8>) {
    loop {
        if buf.len() >= 2 {
            let mut len = 0usize;
            let mut shift = 0;
            let mut i = 1;
            let mut whole = false;
            while i < buf.len() {
                let b = buf[i];
                len |= ((b & 0x7F) as usize) << shift;
                i += 1;
                shift += 7;
                if b & 0x80 == 0 {
                    whole = true;
                    break;
                }
            }
            if whole && buf.len() >= i + len {
                let first = buf[0];
                let body = buf[i..i + len].to_vec();
                buf.drain(..i + len);
                return (first, body);
            }
        }
        let mut chunk = [0u8; 4096];
        let n = r.read(&mut chunk).await.unwrap();
        assert!(n > 0, "client closed");
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn params(port: u16, transport: Transport) -> Params {
    Params {
        host: "127.0.0.1".into(),
        port,
        transport,
        ws_path: "/mqtt".into(),
        client_id: "test-client".into(),
        keep_alive: 30,
        username: Some("user".into()),
        password: Some("secret".into()),
        client_auth: None,
        trusted: Vec::new(),
        wireguard: None,
        filters: vec!["sensors/#".into()],
    }
}

fn collector() -> (Emit, mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(move |e| {
        let _ = tx.send(e);
    }), rx)
}

#[tokio::test]
async fn connect_subscribe_receive_publish() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let broker = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let (first, body) = read_packet(&mut s, &mut buf).await;
        assert_eq!(first, 0x10);
        // Client ID, then user name and password.
        assert!(body.windows(11).any(|w| w == b"test-client"));
        assert!(body.windows(6).any(|w| w == b"secret"));
        s.write_all(&[0x20, 2, 0, 0]).await.unwrap();
        let (first, body) = read_packet(&mut s, &mut buf).await;
        assert_eq!(first, 0x82);
        assert_eq!(&body[2..], &[0, 9, b's', b'e', b'n', b's', b'o', b'r', b's', b'/', b'#', 1]);
        let pkid = [body[0], body[1]];
        s.write_all(&[0x90, 3, pkid[0], pkid[1], 1]).await.unwrap();
        // A retained message at QoS 1, which the client acknowledges.
        let msg = codec::publish(&Publish { topic: "sensors/t1".into(), payload: b"21.5".to_vec(), qos: 1, retain: true, dup: false, pkid: 42 }).unwrap();
        s.write_all(&msg).await.unwrap();
        let (first, body) = read_packet(&mut s, &mut buf).await;
        assert_eq!((first, body), (0x40, vec![0, 42]));
        // The client publishes at QoS 1.
        let (first, body) = read_packet(&mut s, &mut buf).await;
        assert_eq!(first, 0x32);
        let id = [body[body.len() - 4], body[body.len() - 3]];
        assert!(body.ends_with(b"on"));
        s.write_all(&[0x40, 2, id[0], id[1]]).await.unwrap();
        // And goes.
        let (first, _) = read_packet(&mut s, &mut buf).await;
        assert_eq!(first, 0xE0);
    });

    let (emit, mut events) = collector();
    let conn = connect(params(port, Transport::Tcp), emit).await.unwrap();
    let got = match events.recv().await.unwrap() {
        Event::Messages { messages } => messages,
        Event::Closed { error } => panic!("closed: {error:?}"),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].topic, "sensors/t1");
    assert_eq!(got[0].payload, "MjEuNQ==");
    assert!(got[0].retain);
    conn.publish("lights/1".into(), b"on".to_vec(), 1, false).await.unwrap();
    conn.close().await;
    broker.await.unwrap();
    match events.recv().await.unwrap() {
        Event::Closed { error } => assert_eq!(error, None),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn bad_login_is_told_apart() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        read_packet(&mut s, &mut buf).await;
        s.write_all(&[0x20, 2, 0, 4]).await.unwrap();
    });
    let (emit, _events) = collector();
    match connect(params(port, Transport::Tcp), emit).await {
        Err(MqttError::LoginRefused(m)) => assert!(m.contains("password")),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("connected"),
    }
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn over_websocket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let callback = |req: &tokio_tungstenite::tungstenite::handshake::server::Request, mut resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
            assert_eq!(req.uri().path(), "/mqtt");
            assert_eq!(req.headers().get("Sec-WebSocket-Protocol").unwrap(), "mqtt");
            resp.headers_mut().insert("Sec-WebSocket-Protocol", "mqtt".parse().unwrap());
            Ok(resp)
        };
        let mut ws = tokio_tungstenite::accept_hdr_async(s, callback).await.unwrap();
        let _connect = ws.next().await.unwrap().unwrap();
        // CONNACK and a message in one frame, a SUBACK split over two.
        let mut out = vec![0x20, 2, 0, 0];
        out.extend(codec::publish(&Publish { topic: "a".into(), payload: b"x".to_vec(), qos: 0, retain: false, dup: false, pkid: 0 }).unwrap());
        ws.send(Message::binary(out)).await.unwrap();
        while let Some(Ok(m)) = ws.next().await {
            if m.is_close() {
                break;
            }
        }
    });
    let (emit, mut events) = collector();
    let mut p = params(port, Transport::Ws);
    p.username = None;
    let conn = connect(p, emit).await.unwrap();
    match events.recv().await.unwrap() {
        Event::Messages { messages } => assert_eq!(messages[0].topic, "a"),
        other => panic!("{other:?}"),
    }
    conn.close().await;
}

#[test]
fn options_defaults() {
    let o = MqttOptions::default();
    assert!(o.is_empty());
    assert_eq!(o.filters(), vec!["#".to_string()]);
    assert_eq!(Transport::parse(Some("WSS")), Transport::Wss);
    assert_eq!(Transport::parse(None), Transport::Tcp);
    assert!(random_client_id().starts_with("reachercm-"));
}
