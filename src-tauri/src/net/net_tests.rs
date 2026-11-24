use super::icmp::{checksum, echo_request, parse_reply};
use super::scan::{port_state, probe, scan, PortState, ScanEvent};
use super::wol::{magic_packet, parse_mac};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

#[test]
fn macs_are_read_in_every_usual_spelling() {
    let want = [0x00, 0x11, 0x22, 0xaa, 0xbb, 0xcc];
    for text in ["00:11:22:aa:bb:cc", "00-11-22-AA-BB-CC", "0011.22aa.bbcc", "001122aabbcc", " 00:11:22:aa:bb:cc "] {
        assert_eq!(parse_mac(text).unwrap(), want, "{text}");
    }
    for bad in ["", "00:11:22:aa:bb", "00:11:22:aa:bb:cc:dd", "zz:11:22:aa:bb:cc"] {
        assert!(parse_mac(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_magic_packet_is_ff_then_sixteen_macs() {
    let mac = [1, 2, 3, 4, 5, 6];
    let p = magic_packet(mac);
    assert_eq!(&p[..6], &[0xff; 6]);
    for i in 0..16 {
        assert_eq!(&p[6 + i * 6..12 + i * 6], &mac);
    }
}

#[test]
fn an_echo_request_checks_out() {
    let p = echo_request(false, 0x1234, 7);
    assert_eq!(p[0], 8);
    assert_eq!(&p[4..8], &[0x12, 0x34, 0, 7]);
    // A packet with its checksum in sums to zero.
    assert_eq!(checksum(&p), 0);
    // IPv6: type 128, the kernel fills the checksum.
    let p6 = echo_request(true, 1, 2);
    assert_eq!(p6[0], 128);
    assert_eq!(&p6[2..4], &[0, 0]);
}

#[test]
fn replies_are_read_with_or_without_the_ip_header() {
    let mut reply = echo_request(false, 0x0102, 9);
    reply[0] = 0;
    assert_eq!(parse_reply(false, &reply), Some((0x0102, 9, None)));
    // macOS: a 20-byte IPv4 header first, TTL in byte 8.
    let mut with_ip = vec![0x45, 0, 0, 0, 0, 0, 0, 0, 57, 1, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2];
    with_ip.extend_from_slice(&reply);
    assert_eq!(parse_reply(false, &with_ip), Some((0x0102, 9, Some(57))));
    // A request (type 8) is not a reply.
    assert_eq!(parse_reply(false, &echo_request(false, 1, 1)), None);
    let mut r6 = echo_request(true, 3, 4);
    r6[0] = 129;
    assert_eq!(parse_reply(true, &r6), Some((3, 4, None)));
    assert_eq!(parse_reply(true, &[129, 0, 0]), None);
}

#[tokio::test]
async fn a_listening_port_is_open_and_reported_by_a_scan() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let open = listener.local_addr().unwrap().port();
    let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
    assert_eq!(probe(lo, open, Duration::from_secs(2)).await.0, PortState::Open);
    assert!(super::tcp_ping(lo, open, Duration::from_secs(2)).await.ok);

    let seen = std::sync::Mutex::new(Vec::new());
    let summary = scan("t1", lo, vec![open], Duration::from_secs(2), 8, |e| {
        if let ScanEvent::Open { port, .. } = e {
            seen.lock().unwrap().push(port);
        }
    })
    .await;
    assert_eq!(summary.open, vec![open]);
    assert_eq!(summary.closed, 0);
    assert_eq!(summary.scanned, 1);
    assert!(!summary.cancelled);
    assert_eq!(*seen.lock().unwrap(), vec![open]);
}

#[test]
fn refusal_and_timeout_have_distinct_tcp_results() {
    use std::io::ErrorKind;
    // A runner is free to drop closed-port loopback probes instead of
    // returning a refusal before our deadline. Test both outcomes directly.
    let refused = || Some(Err(ErrorKind::ConnectionRefused.into()));
    assert_eq!(port_state(refused()), PortState::Closed);
    assert_eq!(port_state(None), PortState::Filtered);
    let elapsed = Duration::from_millis(12);
    let reply = super::tcp_ping_reply(refused(), elapsed);
    assert!(reply.ok);
    assert_eq!(reply.ms, Some(12.0));
    assert_eq!(reply.method, "tcp");
    let timeout = super::tcp_ping_reply(None, elapsed);
    assert!(!timeout.ok);
    assert_eq!(timeout.ms, None);
    assert_eq!(timeout.error, None);
}

#[tokio::test]
async fn names_and_addresses_resolve() {
    assert_eq!(super::resolve("10.0.0.5").await.unwrap(), vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))]);
    assert_eq!(super::resolve("[::1]").await.unwrap(), vec!["::1".parse::<IpAddr>().unwrap()]);
    assert!(super::resolve("  ").await.is_err());
}

#[test]
fn icmp_reaches_this_machine_where_allowed() {
    // CI machines may forbid ICMP sockets; then the caller falls back to TCP.
    match super::icmp::echo(IpAddr::V4(Ipv4Addr::LOCALHOST), Duration::from_secs(2), 1) {
        Ok(reply) => assert!(reply.is_some(), "no echo reply from 127.0.0.1"),
        Err(super::icmp::IcmpError::Unavailable(why)) => eprintln!("ICMP not available here: {why}"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn the_system_ping_output_is_read() {
    use super::icmp::parse_ping_output;
    let iputils = "PING 10.0.0.5 (10.0.0.5) 56(84) bytes of data.\n64 bytes from 10.0.0.5: icmp_seq=1 ttl=63 time=0.412 ms\n";
    let r = parse_ping_output(iputils).unwrap();
    assert!((r.ms - 0.412).abs() < 1e-9 && r.ttl == Some(63));
    let mac6 = "16 bytes from ::1, icmp_seq=0 hlim=64 time=0.077 ms";
    assert_eq!(parse_ping_output(mac6).unwrap().ttl, Some(64));
    let lost = "PING 10.0.0.9 (10.0.0.9) 56(84) bytes of data.\n\n--- 10.0.0.9 ping statistics ---\n1 packets transmitted, 0 received, 100% packet loss\nrtt min/avg/max/mdev = 0.1/0.1/0.1/0.0 ms";
    assert!(parse_ping_output(lost).is_none());
}
