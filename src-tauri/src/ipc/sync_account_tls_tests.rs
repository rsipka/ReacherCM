use std::{error::Error, io, sync::Arc, time::Duration};

use rustls::{
    CertificateError, ServerConfig,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

use super::{endpoint, http_builder, origin};

// Disposable certificates for a reserved hostname. Only the trusted-fixture
// cases install this root, on their own client and never in a system store.
const HOST: &str = "sync-fixture.invalid";
const CA: &[u8] = include_bytes!("testdata/sync_tls/ca.der");
const CERT: &[u8] = include_bytes!("testdata/sync_tls/server.der");
const TEST_KEY: &[u8] = include_bytes!("testdata/sync_tls/server-key.der");
const DUMMY_TOKEN: &str = "synthetic-sync-authorization";
const DUMMY_BUNDLE: &str = "synthetic-vault-upload";

async fn exchange(
    host: &str,
    trust_fixture: bool,
) -> (
    Result<reqwest::Response, reqwest::Error>,
    io::Result<String>,
) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(CERT)],
            PrivatePkcs8KeyDer::from(TEST_KEY).into(),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = origin(&format!("https://{host}:{}", address.port())).unwrap();
    let mut builder = http_builder(&origin).no_proxy().resolve(host, address);
    if trust_fixture {
        builder = builder.tls_certs_only([reqwest::Certificate::from_der(CA).unwrap()]);
    }
    let client = builder.build().unwrap();
    let upload = client
        .put(endpoint(&origin, "/api/v1/vaults/test-vault/bundle"))
        .bearer_auth(DUMMY_TOKEN)
        .body(DUMMY_BUNDLE);

    let server = async {
        let (tcp, _) = listener.accept().await?;
        // Rejected handshakes never produce a stream from which HTTP
        // headers or the request body can be read.
        let mut stream = acceptor.accept(tcp).await?;
        let marker = DUMMY_BUNDLE.as_bytes();
        let mut received = Vec::new();
        let mut buffer = [0; 512];
        while !received.ends_with(marker) {
            let count = stream.read(&mut buffer).await?;
            assert!(
                count > 0 && received.len() < 4096,
                "incomplete fixture upload"
            );
            received.extend_from_slice(&buffer[..count]);
        }
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await?;
        Ok(String::from_utf8(received).unwrap())
    };

    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(upload.send(), server)
    })
    .await
    .expect("TLS test must finish on the loopback server")
}

fn certificate_error(error: &reqwest::Error) -> &CertificateError {
    let mut cause: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(rustls::Error::InvalidCertificate(cert)) =
            current.downcast_ref::<rustls::Error>()
        {
            return cert;
        }
        // I/O errors can wrap rustls errors without returning the wrapped
        // error itself from Error::source().
        cause = current
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &dyn Error)
            .or_else(|| current.source());
    }
    panic!("expected a certificate validation error: {error:?}");
}

fn assert_rejected_before_http(server: io::Result<String>) {
    let error = server.expect_err("the server received an application request");
    assert!(
        matches!(
            error
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<rustls::Error>()),
            Some(rustls::Error::AlertReceived(_))
        ),
        "expected the client to abort the TLS handshake with an alert: {error:?}"
    );
}

#[tokio::test]
async fn sync_tls_rejects_an_untrusted_certificate_before_upload() {
    let (client, server) = exchange(HOST, false).await;
    certificate_error(&client.expect_err("an untrusted certificate was accepted"));
    assert_rejected_before_http(server);
}

#[tokio::test]
async fn sync_tls_rejects_a_trusted_certificate_for_the_wrong_hostname() {
    let (client, server) = exchange("wrong-sync-fixture.invalid", true).await;
    let error = client.expect_err("a certificate for another hostname was accepted");
    assert!(matches!(
        certificate_error(&error),
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
    ));
    assert_rejected_before_http(server);
}

#[tokio::test]
async fn sync_tls_uploads_to_the_trusted_matching_hostname() {
    let (client, server) = exchange(HOST, true).await;
    assert_eq!(client.unwrap().status(), reqwest::StatusCode::NO_CONTENT);
    let request = server.unwrap();
    assert!(request.starts_with("PUT /api/v1/vaults/test-vault/bundle HTTP/1.1\r\n"));
    assert!(request.contains(&format!("authorization: Bearer {DUMMY_TOKEN}\r\n")));
    assert!(request.ends_with(DUMMY_BUNDLE));
}
