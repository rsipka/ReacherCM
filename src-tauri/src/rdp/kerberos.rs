//! Kerberos login for RDP sessions that ask for it.
//!
//! CredSSP then negotiates Kerberos with NTLM as the fallback, as mstsc
//! does. What Kerberos needs on top of the login is a KDC to get the ticket
//! from: the one the session names (a host or a KDC proxy URL), else one
//! found here in DNS (`_kerberos._tcp.<realm>`), else whatever the system's
//! Kerberos settings say (krb5.conf, the Windows registry, `SSPI_KDC_URL`),
//! which the Kerberos library looks at itself.
//!
//! The server must be addressed by its DNS name: the ticket is for
//! `TERMSRV/<host>`, and an IP address has no such service.

use std::time::Duration;

use hickory_resolver::proto::rr::{RData, RecordType};

use ironrdp_connector::credssp::{KdcResolution, KerberosConfig};

use super::options::{kdc_url, RdpOptions};

/// Whether the session logs in with Kerberos.
pub fn wanted(options: Option<&RdpOptions>) -> bool {
    options.is_some_and(|o| o.auth.as_deref() == Some("kerberos"))
}

/// The Kerberos realm (DNS domain) the login belongs to: from a UPN
/// (`user@corp.example.com`), a dotted domain, or the server's own name.
pub fn realm(username: &str, domain: &str, host: &str) -> Option<String> {
    if let Some((_, at)) = username.rsplit_once('@') {
        if at.contains('.') {
            return Some(at.to_ascii_lowercase());
        }
    }
    if domain.contains('.') {
        return Some(domain.trim().to_ascii_lowercase());
    }
    let host = host.trim().trim_end_matches('.');
    if host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    host.split_once('.').map(|(_, rest)| rest.to_ascii_lowercase()).filter(|r| r.contains('.'))
}

/// The KDC for a session: its own, else one DNS names for the realm. None
/// leaves it to the system's Kerberos settings.
pub async fn find_kdc(options: Option<&RdpOptions>, username: &str, domain: &str, host: &str) -> Option<String> {
    if !wanted(options) {
        return None;
    }
    if let Some(own) = options.and_then(|o| o.kdc.as_deref()).and_then(kdc_url) {
        return Some(own);
    }
    let realm = realm(username, domain, host)?;
    let found = lookup_srv(&realm).await;
    match &found {
        Some(kdc) => tracing::info!("RDP: Kerberos KDC for {realm} from DNS: {kdc}"),
        None => tracing::info!("RDP: no KDC for {realm} in DNS; the system's Kerberos settings decide"),
    }
    found
}

async fn lookup_srv(realm: &str) -> Option<String> {
    let resolver = hickory_resolver::Resolver::builder_tokio().ok()?.build().ok()?;
    let name = format!("_kerberos._tcp.{realm}.");
    let lookup = tokio::time::timeout(Duration::from_secs(5), resolver.lookup(name.as_str(), RecordType::SRV)).await.ok()?.ok()?;
    // Lowest priority first, then the heaviest.
    let best = lookup
        .answers()
        .iter()
        .filter_map(|r| match &r.data {
            RData::SRV(srv) => Some(srv),
            _ => None,
        })
        .min_by_key(|r| (r.priority, u16::MAX - r.weight))?;
    let target = best.target.to_utf8();
    Some(format!("tcp://{}:{}", target.trim_end_matches('.'), best.port))
}

/// What CredSSP is given: Kerberos with this KDC (or the system's), or
/// None for NTLM only.
pub fn config(options: Option<&RdpOptions>, kdc: Option<&str>) -> Option<KerberosConfig> {
    if !wanted(options) {
        return None;
    }
    let url = kdc.and_then(|k| url::Url::parse(k).ok());
    Some(KerberosConfig {
        kdc_resolution: KdcResolution::KdcUrl(url),
        hostname: client_hostname(),
    })
}

/// This computer's name, as Kerberos reports it to the KDC.
fn client_hostname() -> String {
    gethostname::gethostname()
        .to_str()
        .map(|h| h.split('.').next().unwrap_or(h).to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "reachercm".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realm_from_the_login_or_the_server() {
        assert_eq!(realm("ann@corp.example.com", "", "10.0.0.5").as_deref(), Some("corp.example.com"));
        assert_eq!(realm("ann", "CORP.example.com", "10.0.0.5").as_deref(), Some("corp.example.com"));
        assert_eq!(realm("ann", "CORP", "ts1.corp.example.com").as_deref(), Some("corp.example.com"));
        assert_eq!(realm("ann", "CORP", "10.0.0.5"), None);
        assert_eq!(realm("ann", "", "ts1.example"), None);
    }

    #[test]
    fn kerberos_only_when_asked() {
        assert!(config(None, None).is_none());
        let o = RdpOptions { auth: Some("kerberos".into()), ..Default::default() };
        let c = config(Some(&o), Some("tcp://dc.example.com:88")).unwrap();
        assert!(matches!(c.kdc_resolution, KdcResolution::KdcUrl(Some(_))));
    }
}
