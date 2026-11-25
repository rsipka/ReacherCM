//! Against a real Redfish service; ignored unless asked for:
//!
//! REACH_BMC_PORT=8443 REACH_BMC_USER=root REACH_BMC_PASS=calvin \
//!   cargo test --lib bmc::live_tests -- --ignored --nocapture
//!
//! The DMTF Redfish Mockup Server does: `python3 redfishMockupServer.py -s
//! --cert c.pem --key k.pem -p 8443 -D public-rackmount1`. The certificate
//! is trusted after the first refusal, as the prompt would. With
//! `REACH_BMC_ACTIONS=1` it also presses the power button (GracefulRestart),
//! sets boot once and clears nothing: only for a mockup or a spare server.

use super::*;

fn params() -> Option<Params> {
    let port = std::env::var("REACH_BMC_PORT").ok()?.parse().ok()?;
    Some(Params {
        host: std::env::var("REACH_BMC_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port,
        username: std::env::var("REACH_BMC_USER").unwrap_or_else(|_| "root".into()),
        password: std::env::var("REACH_BMC_PASS").unwrap_or_default(),
        trusted: Vec::new(),
        socks: None,
        tunnel: None,
    })
}

#[tokio::test]
#[ignore]
async fn live_redfish() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut p = params().expect("REACH_BMC_PORT not set");
    let conn = match BmcConn::open(Params { trusted: vec![], ..clone(&p) }).await {
        Ok(c) => c,
        Err(BmcError::UntrustedCert(fp)) => {
            eprintln!("trusting {fp}");
            assert_eq!(fp.len(), 95);
            p.trusted.push(fp);
            BmcConn::open(clone(&p)).await.expect("connect with the certificate trusted")
        }
        Err(e) => panic!("connect: {e}"),
    };
    let conn = Arc::new(conn);
    eprintln!("opened: {:?}", conn.opened());

    let o = conn.overview().await.expect("overview");
    eprintln!("overview: {o:#?}");
    assert!(o.model.is_some() || o.serial.is_some());

    let s = conn.sensors().await.expect("sensors");
    eprintln!("sensors: {} temps, {} fans, {} volts, {} PSUs, {:?} W", s.temperatures.len(), s.fans.len(), s.voltages.len(), s.power_supplies.len(), s.power_watts);

    let hw = conn.hardware().await.expect("hardware");
    eprintln!("hardware: {} cpus, {} dimms, {} storage, {} drives, {} nics, {} firmware", hw.processors.len(), hw.memory.len(), hw.storage.len(), hw.drives.len(), hw.network.len(), hw.firmware.len());
    assert!(!hw.processors.is_empty());

    let logs = conn.log_services().await.expect("logs");
    eprintln!("logs: {logs:?}");
    for l in &logs {
        let entries = conn.log_entries(&l.path).await.expect("entries");
        eprintln!("  {}: {} entries, newest {:?}", l.name, entries.len(), entries.first());
    }
    assert!(conn.log_entries("/redfish/v1/Systems").await.is_err(), "only paths it listed");

    let jobs = conn.jobs().await.expect("jobs");
    eprintln!("jobs: {}", jobs.len());
    let vms = conn.virtual_media().await.expect("virtual media");
    eprintln!("virtual media: {vms:?}");

    if std::env::var("REACH_BMC_ACTIONS").as_deref() == Ok("1") {
        if o.reset_types.iter().any(|r| r == "GracefulRestart") {
            conn.power("GracefulRestart").await.expect("power");
        }
        assert!(conn.power("Explode").await.is_err());
        if let Some(t) = o.boot_targets.iter().find(|t| *t == "Pxe") {
            conn.boot_once(t).await.expect("boot once");
        }
        if o.identify.is_some() {
            conn.identify(true).await.expect("identify");
        }
        if let Some(vm) = vms.first() {
            conn.media_insert(&vm.path, "http://10.0.0.1/boot.iso").await.expect("insert");
            conn.media_eject(&vm.path).await.expect("eject");
            assert!(conn.media_insert(&vm.path, "file:///etc/passwd").await.is_err());
        }
    }
    conn.client.logout().await;
}

fn clone(p: &Params) -> Params {
    Params { host: p.host.clone(), port: p.port, username: p.username.clone(), password: p.password.clone(), trusted: p.trusted.clone(), socks: None, tunnel: None }
}
