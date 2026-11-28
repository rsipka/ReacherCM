use super::*;

#[test]
fn connect_with_login() {
    let bytes = connect(&Connect {
        client_id: "c1".into(),
        keep_alive: 60,
        clean_session: true,
        username: Some("u".into()),
        password: Some("p".into()),
    })
    .unwrap();
    assert_eq!(
        bytes,
        vec![0x10, 20, 0, 4, b'M', b'Q', b'T', b'T', 4, 0xC2, 0, 60, 0, 2, b'c', b'1', 0, 1, b'u', 0, 1, b'p']
    );
}

#[test]
fn publish_round_trip() {
    let p = Publish { topic: "a/b".into(), payload: b"hi".to_vec(), qos: 1, retain: true, dup: false, pkid: 7 };
    let mut buf = publish(&p).unwrap();
    assert_eq!(buf[0], 0x33);
    buf.extend_from_slice(&pingreq()[..1]); // the start of the next packet stays
    assert_eq!(decode(&mut buf).unwrap(), Some(Incoming::Publish(p)));
    assert_eq!(buf, vec![0xC0]);
    assert_eq!(decode(&mut buf).unwrap(), None);
}

#[test]
fn long_remaining_length() {
    let payload = vec![0xAB; 300];
    let p = Publish { topic: "t".into(), payload: payload.clone(), qos: 0, retain: false, dup: false, pkid: 0 };
    let mut buf = publish(&p).unwrap();
    // 3 + 300 = 303 = 0xAF 0x02
    assert_eq!(&buf[1..3], &[0xAF, 0x02]);
    let half = buf.split_off(100);
    let mut partial = buf;
    assert_eq!(decode(&mut partial).unwrap(), None);
    partial.extend_from_slice(&half);
    match decode(&mut partial).unwrap() {
        Some(Incoming::Publish(got)) => assert_eq!(got.payload, payload),
        other => panic!("{other:?}"),
    }
}

#[test]
fn acks_and_others() {
    let mut buf = vec![0x20, 2, 1, 0, 0x90, 4, 0, 9, 0, 0x80, 0xD0, 0, 0xB0, 2, 0, 3];
    buf.extend(ack(PUBREL, 5));
    assert_eq!(decode(&mut buf).unwrap(), Some(Incoming::ConnAck { session_present: true, code: 0 }));
    assert_eq!(decode(&mut buf).unwrap(), Some(Incoming::SubAck { pkid: 9, codes: vec![0, 0x80] }));
    assert_eq!(decode(&mut buf).unwrap(), Some(Incoming::PingResp));
    assert_eq!(decode(&mut buf).unwrap(), Some(Incoming::UnsubAck(3)));
    assert_eq!(decode(&mut buf).unwrap(), Some(Incoming::PubRel(5)));
    assert!(buf.is_empty());
}

#[test]
fn rejects_bad_packets() {
    assert!(decode(&mut vec![0x36, 3, 0, 1, b'a']).is_err()); // QoS 3
    assert!(decode(&mut vec![0x30, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]).is_err());
}

#[test]
fn subscribe_bytes() {
    let b = subscribe(1, &[("a/#".into(), 1)]).unwrap();
    assert_eq!(b, vec![0x82, 8, 0, 1, 0, 3, b'a', b'/', b'#', 1]);
    let u = unsubscribe(2, &["a/#".into()]).unwrap();
    assert_eq!(u, vec![0xA2, 7, 0, 2, 0, 3, b'a', b'/', b'#']);
}

#[test]
fn filters() {
    assert!(matches("#", "a/b"));
    assert!(matches("a/+/c", "a/b/c"));
    assert!(!matches("a/+/c", "a/b/d"));
    assert!(matches("a/#", "a"));
    assert!(matches("a/#", "a/b/c"));
    assert!(!matches("#", "$SYS/broker"));
    assert!(!matches("+/broker", "$SYS/broker"));
    assert!(matches("$SYS/#", "$SYS/broker/uptime"));
    assert!(valid_filter("a/+/#"));
    assert!(!valid_filter("a/#/b"));
    assert!(!valid_filter("a/b#"));
    assert!(valid_topic("a/b"));
    assert!(!valid_topic("a/+"));
}
