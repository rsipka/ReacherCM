//! FTPS certificates. One the system trusts is accepted; any other only
//! when the session trusts its SHA-256 fingerprint, as web sessions do (see
//! `crate::web::certs`). An unknown one fails the connect and is kept, so
//! the error can show its fingerprint and the user can trust it.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

/// `AB:CD:...`, as the web sessions show and keep it.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

#[derive(Debug)]
struct Verifier {
    roots: Arc<WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
    trusted: Vec<String>,
    /// The fingerprint of a certificate that was refused.
    refused: Arc<Mutex<Option<String>>>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        if self.trusted.iter().any(|t| t.eq_ignore_ascii_case(&fp)) {
            return Ok(ServerCertVerified::assertion());
        }
        match self.roots.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
            Ok(ok) => Ok(ok),
            Err(e) => {
                if let Ok(mut r) = self.refused.lock() {
                    *r = Some(fp);
                }
                Err(e)
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// A TLS setup for one FTP connection: the control connection and its data
/// connections use the same one, so the data connections resume the
/// control connection's TLS session, which many servers require.
pub struct Tls {
    pub connector: tokio_rustls::TlsConnector,
    pub name: ServerName<'static>,
    pub refused: Arc<Mutex<Option<String>>>,
}

impl Tls {
    pub fn new(host: &str, trusted: &[String]) -> Result<Self, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
            .build()
            .map_err(|e| format!("TLS setup failed: {e}"))?;
        let refused = Arc::new(Mutex::new(None));
        let verifier = Verifier { roots: webpki, provider: provider.clone(), trusted: trusted.to_vec(), refused: refused.clone() };
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS setup failed: {e}"))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let name = ServerName::try_from(host.trim_matches(['[', ']']).to_string())
            .map_err(|e| format!("{host}: not a host name TLS can check ({e})"))?;
        Ok(Self { connector: tokio_rustls::TlsConnector::from(Arc::new(config)), name, refused })
    }

    /// The fingerprint of the certificate last refused, if one was.
    pub fn refused(&self) -> Option<String> {
        self.refused.lock().ok().and_then(|r| r.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_upper_hex_with_colons() {
        let fp = fingerprint(b"abc");
        assert!(fp.starts_with("BA:78:16:BF"));
        assert_eq!(fp.len(), 32 * 3 - 1);
    }
}
