use super::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::time_provider::TimeProvider;
use rustls::{CertificateError, ClientConnection, ServerConfig, ServerConnection};
use std::io::Cursor;

const CA: &[u8] = include_bytes!("../../../../assets/test-tls/ca.der");
const CERT: &[u8] = include_bytes!("../../../../assets/test-tls/localhost.der");
const KEY: &[u8] = include_bytes!("../../../../assets/test-tls/localhost-key.der");

#[derive(Debug)]
struct FixtureTime;

impl TimeProvider for FixtureTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            1_800_000_000,
        )))
    }
}

fn handshake(roots: rustls::RootCertStore, name: &str) -> Result<(), rustls::Error> {
    // The public synthetic test key has no authority outside this test's CA store.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let server_config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(CERT.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
        )
        .unwrap();
    let mut client_config = tls_config(roots).unwrap();
    client_config.time_provider = Arc::new(FixtureTime);
    let mut client = ClientConnection::new(
        Arc::new(client_config),
        ServerName::try_from(name.to_owned()).unwrap(),
    )?;
    let mut server = ServerConnection::new(Arc::new(server_config))?;
    for _ in 0..16 {
        let mut data = Vec::new();
        client.write_tls(&mut data).unwrap();
        if !data.is_empty() {
            server.read_tls(&mut Cursor::new(data)).unwrap();
            server.process_new_packets()?;
        }
        let mut data = Vec::new();
        server.write_tls(&mut data).unwrap();
        if !data.is_empty() {
            client.read_tls(&mut Cursor::new(data)).unwrap();
            client.process_new_packets()?;
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            return Ok(());
        }
    }
    panic!("in-memory TLS handshake made no progress");
}

#[test]
fn tls_connector_validates_ca_and_hostname_with_safe_defaults() {
    let trusted = || {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(CA.to_vec())).unwrap();
        roots
    };
    assert!(handshake(trusted(), "localhost").is_ok());
    assert!(matches!(
        handshake(mozilla_roots(), "localhost"),
        Err(rustls::Error::InvalidCertificate(
            CertificateError::UnknownIssuer
        ))
    ));
    assert!(matches!(
        handshake(trusted(), "other.invalid"),
        Err(rustls::Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
        ))
    ));
}
