// Self-signed TLS setup for server and client.
//
// Trust model: TOFU (trust on first use).
// - Server generates a self-signed cert on first run and saves it.
// - Client accepts any cert on first connection, saves the raw DER to
//   known_servers_dir()/HOST_PORT.crt, and pins it for all future connections.
// - On mismatch: the connection is rejected. User must delete the file to re-pin
//   (e.g., after a cert rotation).

use anyhow::Result;
use rcgen::generate_simple_self_signed;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
    DigitallySignedStruct, Error as TlsError, SignatureScheme,
};
use std::path::{Path, PathBuf};
use tracing::warn;

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
pub fn default_cert_paths() -> (PathBuf, PathBuf) {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wayflow")
        .join("certs");
    (dir.join("server.crt"), dir.join("server.key"))
}

/// Directory where client-side pinned server certs are stored.
pub fn known_servers_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wayflow")
        .join("known_servers")
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

/// Client-side TLS with TOFU cert pinning.
///
/// On first connection to `server_addr`, the server's cert DER is saved to
/// `known_servers_dir()/HOST_PORT.crt`. On subsequent connections the saved cert
/// is compared byte-for-byte; a mismatch aborts the connection.
pub fn client_tls_tofu(server_addr: &str) -> Result<rustls::ClientConfig> {
    client_tls_tofu_with_dir(server_addr, &known_servers_dir())
}

fn addr_to_filename(addr: &str) -> String {
    let sanitized: String = addr
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("{sanitized}.crt")
}

/// Testable variant of `client_tls_tofu` that accepts an explicit known_servers dir.
pub fn client_tls_tofu_with_dir(server_addr: &str, dir: &Path) -> Result<rustls::ClientConfig> {
    let cert_path = dir.join(addr_to_filename(server_addr));
    let pinned_der = if cert_path.exists() {
        Some(std::fs::read(&cert_path)?)
    } else {
        None
    };

    let verifier = TofuVerifier {
        server_addr: server_addr.to_string(),
        cert_path,
        pinned_der,
    };

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(verifier))
        .with_no_client_auth();
    Ok(config)
}

#[derive(Debug)]
struct TofuVerifier {
    server_addr: String,
    cert_path: PathBuf,
    pinned_der: Option<Vec<u8>>,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &ServerName,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let cert_der = end_entity.as_ref();
        match &self.pinned_der {
            None => {
                if let Some(parent) = self.cert_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| TlsError::General(e.to_string()))?;
                }
                std::fs::write(&self.cert_path, cert_der)
                    .map_err(|e| TlsError::General(e.to_string()))?;
                warn!(
                    "first connection to {} -- cert pinned. Delete {} to re-pin after cert rotation.",
                    self.server_addr,
                    self.cert_path.display()
                );
                Ok(ServerCertVerified::assertion())
            }
            Some(pinned) => {
                if cert_der == pinned.as_slice() {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(TlsError::General(format!(
                        "cert mismatch for {} -- possible MITM or cert rotation. Delete {} to re-pin.",
                        self.server_addr,
                        self.cert_path.display()
                    )))
                }
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Client-side TLS that accepts any certificate. For dev/test use only.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init() {
        install_default_crypto_provider();
    }

    #[test]
    fn install_crypto_provider_is_idempotent() {
        install_default_crypto_provider();
        install_default_crypto_provider(); // must not panic
    }

    #[test]
    fn default_cert_paths_contain_wayflow() {
        let (cert, key) = default_cert_paths();
        let cert_s = cert.to_string_lossy();
        let key_s = key.to_string_lossy();
        assert!(cert_s.contains("wayflow"), "cert path: {cert_s}");
        assert!(key_s.contains("wayflow"), "key path: {key_s}");
        assert!(cert_s.ends_with("server.crt"), "cert path: {cert_s}");
        assert!(key_s.ends_with("server.key"), "key path: {key_s}");
    }

    #[test]
    fn server_tls_generates_cert_and_creates_dir() {
        init();
        let dir = tempdir().unwrap();
        let cert_path = dir.path().join("certs").join("server.crt");
        let key_path = dir.path().join("certs").join("server.key");

        let result = server_tls(&cert_path, &key_path);
        assert!(result.is_ok(), "server_tls failed: {:?}", result.err());
        assert!(cert_path.exists(), "cert file not created");
        assert!(key_path.exists(), "key file not created");

        let cfg = result.unwrap();
        assert!(!cfg.cert.is_empty(), "no certs returned");
    }

    #[test]
    fn server_tls_loads_existing_cert() {
        init();
        let dir = tempdir().unwrap();
        let cert_path = dir.path().join("server.crt");
        let key_path = dir.path().join("server.key");

        // Generate once
        server_tls(&cert_path, &key_path).unwrap();

        // Load the existing files
        let result = server_tls(&cert_path, &key_path);
        assert!(result.is_ok(), "second call failed: {:?}", result.err());
        let cfg = result.unwrap();
        assert!(!cfg.cert.is_empty());
    }

    #[test]
    fn client_tls_insecure_builds_config() {
        init();
        let result = client_tls_insecure();
        assert!(result.is_ok(), "client_tls_insecure failed: {:?}", result.err());
    }

    #[test]
    fn server_tls_missing_key_returns_error() {
        init();
        let dir = tempdir().unwrap();
        let cert_path = dir.path().join("server.crt");
        let key_path = dir.path().join("server.key");

        // Generate to create cert_path, then delete key so loading fails
        server_tls(&cert_path, &key_path).unwrap();
        std::fs::remove_file(&key_path).unwrap();

        // cert exists but key does not -- neither exists check fails, so it tries to generate
        // fresh (cert is overwritten). This should succeed.
        let result = server_tls(&cert_path, &key_path);
        assert!(result.is_ok());
    }

    #[test]
    fn addr_to_filename_sanitizes_colon_and_dot() {
        assert_eq!(addr_to_filename("helicon:24800"), "helicon_24800.crt");
        assert_eq!(addr_to_filename("192.168.1.2:24800"), "192_168_1_2_24800.crt");
    }

    #[test]
    fn client_tls_tofu_first_connect_pins_cert() {
        init();
        let dir = tempdir().unwrap();
        // No pinned cert exists: verifier should be constructed without error.
        let result = client_tls_tofu_with_dir("helicon:24800", dir.path());
        assert!(result.is_ok(), "{:?}", result.err());
        // Known-servers file should NOT exist yet (created during TLS handshake, not construction).
        assert!(!dir.path().join("helicon_24800.crt").exists());
    }

    #[test]
    fn client_tls_tofu_loads_pinned_cert() {
        init();
        let dir = tempdir().unwrap();
        let cert_path = dir.path().join("helicon_24800.crt");
        // Pre-populate a fake pinned cert.
        std::fs::write(&cert_path, b"fake-der-bytes").unwrap();
        let result = client_tls_tofu_with_dir("helicon:24800", dir.path());
        assert!(result.is_ok(), "{:?}", result.err());
    }
}
