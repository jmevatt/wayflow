// Self-signed TLS setup for server and client.
//
// Wayflow uses a TOFU (trust on first use) model: the server generates a
// self-signed cert on first run and saves it. Clients accept any cert on
// first connection and pin it afterward.
//
// TODO: implement cert pinning; currently accepts all certs (dev mode only).

use anyhow::Result;
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;

pub struct ServerTlsConfig {
    pub cert: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

/// Install the ring crypto provider as the process default.
/// Call once at startup before any rustls usage.
pub fn install_default_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok(); // already installed is fine
}

/// Default paths for the server's self-signed cert and private key.
pub fn default_cert_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("wayflow")
        .join("certs");
    (dir.join("server.crt"), dir.join("server.key"))
}

/// Load or generate a self-signed cert + key pair.
pub fn server_tls(cert_path: &Path, key_path: &Path) -> Result<ServerTlsConfig> {
    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read(cert_path)?;
        let key_pem = std::fs::read(key_path)?;
        let certs = rustls_pemfile::certs(&mut cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()?;
        let key = rustls_pemfile::private_key(&mut key_pem.as_slice())?
            .ok_or_else(|| anyhow::anyhow!("no private key in {}", key_path.display()))?;
        return Ok(ServerTlsConfig { cert: certs, key });
    }

    let cert = generate_simple_self_signed(vec!["wayflow".to_string()])?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();

    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(cert_path, &cert_pem)?;
    std::fs::write(key_path, &key_pem)?;

    let cert_der = cert.cert.der().clone();
    let key_der = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("failed to parse generated key"))?;

    Ok(ServerTlsConfig {
        cert: vec![cert_der.into_owned()],
        key: key_der,
    })
}

/// Client-side TLS config. Currently accepts any certificate.
/// TODO: TOFU pinning.
pub fn client_tls_insecure() -> Result<rustls::ClientConfig> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    struct AcceptAny;

    impl ServerCertVerifier for AcceptAny {
        fn verify_server_cert(
            &self, _: &CertificateDer, _: &[CertificateDer], _: &ServerName,
            _: &[u8], _: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer, _: &DigitallySignedStruct)
            -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer, _: &DigitallySignedStruct)
            -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAny))
        .with_no_client_auth();
    Ok(config)
}
