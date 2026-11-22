//! TLS for direct Selkies servers (`ARCH-036`): the Mac's trusted roots, or one leaf
//! certificate the person trusted by its SHA-256 fingerprint.
//!
//! The pin applies only to the direct clients a `SelkiesServer` builds; SealSkin's clients
//! never see it. Every config is built with the ring provider explicitly: the build has no
//! second provider (`cargo tree -i aws-lc-rs` is empty), so the default connectors elsewhere
//! keep working.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{ring, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// A leaf certificate's SHA-256, over its DER bytes.
pub type Pin = [u8; 32];

/// The leaf the last failed verification rejected, so the gate can show it on the
/// certificate sheet without a second handshake (which a round-robin host could answer
/// with another certificate).
#[derive(Clone, Default)]
pub struct Rejected(Arc<Mutex<Option<Pin>>>);

impl Rejected {
    pub fn take(&self) -> Option<Pin> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn record(&self, pin: Pin) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(pin);
    }
}

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// Installs ring as the process-wide provider, as a guard for any code that builds a config
/// without naming one. Safe to call more than once.
pub fn install_default() {
    let _ = ring::default_provider().install_default();
}

pub fn fingerprint(der: &[u8]) -> Pin {
    let digest = ::ring::digest::digest(&::ring::digest::SHA256, der);
    let mut pin = [0u8; 32];
    pin.copy_from_slice(digest.as_ref());
    pin
}

/// `AB:CD:…`, 32 groups.
pub fn fingerprint_text(pin: &Pin) -> String {
    pin.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// `AB:CD:…` or 64 hex digits, either case; spaces are ignored.
pub fn parse_fingerprint(text: &str) -> Option<Pin> {
    let hex: String = text
        .chars()
        .filter(|c| !matches!(c, ':' | ' '))
        .collect();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut pin = [0u8; 32];
    for (index, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(pin)
}

fn same(a: &Pin, b: &Pin) -> bool {
    // Constant time: every byte is compared whatever the first difference.
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn native_roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            let loaded = rustls_native_certs::load_native_certs();
            let _ = roots.add_parsable_certificates(loaded.certs);
            Arc::new(roots)
        })
        .clone()
}

/// The config for a direct server's clients (HTTP and WebSocket), and where its verifier
/// records a rejected leaf.
pub fn client_config(pin: Option<Pin>) -> (Arc<ClientConfig>, Rejected) {
    client_config_with_roots(native_roots(), pin)
}

pub fn client_config_with_roots(
    roots: Arc<RootCertStore>,
    pin: Option<Pin>,
) -> (Arc<ClientConfig>, Rejected) {
    let provider = provider();
    let rejected = Rejected::default();
    let inner = WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
        .build()
        .ok();
    let verifier = PinnedVerifier {
        inner,
        algorithms: provider.signature_verification_algorithms,
        pin,
        rejected: rejected.clone(),
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    (Arc::new(config), rejected)
}

/// The system roots first; only when they reject the leaf, its exact SHA-256 against the pin
/// (ignoring name and expiry, which is what trusting a self-signed certificate means). The
/// handshake signature is always checked against the leaf's key.
#[derive(Debug)]
struct PinnedVerifier {
    /// `None` when the Mac has no usable roots: only a pinned leaf passes.
    inner: Option<Arc<WebPkiServerVerifier>>,
    algorithms: WebPkiSupportedAlgorithms,
    pin: Option<Pin>,
    rejected: Rejected,
}

impl std::fmt::Debug for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rejected")
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let error = match &self.inner {
            Some(inner) => match inner.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ) {
                Ok(verified) => return Ok(verified),
                Err(error) => error,
            },
            None => rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        };
        let leaf = fingerprint(end_entity.as_ref());
        if self.pin.as_ref().is_some_and(|pin| same(pin, &leaf)) {
            return Ok(ServerCertVerified::assertion());
        }
        self.rejected.record(leaf);
        Err(error)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        match &self.inner {
            Some(inner) => inner.verify_tls12_signature(message, cert, dss),
            None => rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms),
        }
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        match &self.inner {
            Some(inner) => inner.verify_tls13_signature(message, cert, dss),
            None => rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms),
        }
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        match &self.inner {
            Some(inner) => inner.supported_verify_schemes(),
            None => self.algorithms.supported_schemes(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_server {
    //! A TLS server on 127.0.0.1 for the pinning tests.
    use super::*;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::server::{ClientHello, ResolvesServerCert};
    use rustls::sign::CertifiedKey;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    pub struct Leaf {
        pub der: CertificateDer<'static>,
        pub key: rcgen::KeyPair,
        pub cert: rcgen::Certificate,
    }

    pub fn self_signed(names: &[&str]) -> Leaf {
        let key = rcgen::KeyPair::generate().unwrap();
        let params =
            rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .unwrap();
        let cert = params.self_signed(&key).unwrap();
        Leaf {
            der: cert.der().clone(),
            key,
            cert,
        }
    }

    pub fn ca() -> Leaf {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).unwrap();
        Leaf {
            der: cert.der().clone(),
            key,
            cert,
        }
    }

    pub fn signed_by(names: &[&str], ca: &Leaf) -> Leaf {
        let key = rcgen::KeyPair::generate().unwrap();
        let params =
            rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .unwrap();
        let cert = params.signed_by(&key, &ca.cert, &ca.key).unwrap();
        Leaf {
            der: cert.der().clone(),
            key,
            cert,
        }
    }

    #[derive(Debug)]
    struct Fixed(Arc<CertifiedKey>);

    impl ResolvesServerCert for Fixed {
        fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    /// Serves `chain` signed with `key` (which may not match the leaf, on purpose), answering
    /// every request with `answer(head)`.
    pub async fn serve(
        chain: Vec<CertificateDer<'static>>,
        key: &rcgen::KeyPair,
        answer: impl Fn(&str) -> String + Send + Sync + 'static,
    ) -> std::net::SocketAddr {
        let provider = provider();
        let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signing = provider.key_provider.load_private_key(der).unwrap();
        let certified = Arc::new(CertifiedKey::new(chain, signing));
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Fixed(certified)));
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, answer) = (acceptor.clone(), answer.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => request.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_string();
                    let _ = stream.write_all(answer(&head).as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        address
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::*;
    use super::*;

    fn ok(head: &str) -> String {
        let _ = head;
        "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok".into()
    }

    async fn handshake(config: Arc<ClientConfig>, port: u16, name: &str) -> Result<(), String> {
        let connector = tokio_rustls::TlsConnector::from(config);
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let name = ServerName::try_from(name.to_string()).unwrap();
        let mut stream = connector
            .connect(name, tcp)
            .await
            .map_err(|e| e.to_string())?;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        stream
            .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
            .await
            .map_err(|e| e.to_string())?;
        let mut body = Vec::new();
        let _ = stream.read_to_end(&mut body).await;
        if body.ends_with(b"ok") {
            Ok(())
        } else {
            Err("no answer".into())
        }
    }

    fn roots_of(ca: Option<&Leaf>) -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        if let Some(ca) = ca {
            roots.add(ca.der.clone()).unwrap();
        }
        Arc::new(roots)
    }

    #[test]
    fn fingerprint_text_round_trips() {
        let pin: Pin = std::array::from_fn(|i| (i * 7) as u8);
        let text = fingerprint_text(&pin);
        assert_eq!(text.split(':').count(), 32);
        assert!(text.starts_with("00:07:0E"));
        assert_eq!(parse_fingerprint(&text), Some(pin));
        assert_eq!(parse_fingerprint(&text.to_lowercase().replace(':', "")), Some(pin));
        assert_eq!(parse_fingerprint("AB:CD"), None);
        assert_eq!(parse_fingerprint(&"zz".repeat(32)), None);
    }

    #[tokio::test]
    async fn pinned_leaf_is_accepted_when_roots_reject_it() {
        let leaf = self_signed(&["localhost"]);
        let address = serve(vec![leaf.der.clone()], &leaf.key, ok).await;
        let (unpinned, rejected) = client_config_with_roots(roots_of(None), None);
        assert!(handshake(unpinned, address.port(), "localhost").await.is_err());
        assert_eq!(rejected.take(), Some(fingerprint(&leaf.der)));
        let (pinned, rejected) =
            client_config_with_roots(roots_of(None), Some(fingerprint(&leaf.der)));
        // The name doesn't matter once the exact leaf is pinned.
        handshake(pinned, address.port(), "desktop.example").await.unwrap();
        assert_eq!(rejected.take(), None);
    }

    #[tokio::test]
    async fn a_different_leaf_is_refused_despite_a_pin() {
        let leaf = self_signed(&["localhost"]);
        let other = self_signed(&["localhost"]);
        let address = serve(vec![leaf.der.clone()], &leaf.key, ok).await;
        let (config, rejected) =
            client_config_with_roots(roots_of(None), Some(fingerprint(&other.der)));
        assert!(handshake(config, address.port(), "localhost").await.is_err());
        assert_eq!(rejected.take(), Some(fingerprint(&leaf.der)));
    }

    #[tokio::test]
    async fn system_trusted_certificates_still_pass_without_a_pin() {
        let ca = ca();
        let leaf = signed_by(&["localhost"], &ca);
        let address = serve(vec![leaf.der.clone()], &leaf.key, ok).await;
        let (config, rejected) = client_config_with_roots(roots_of(Some(&ca)), None);
        handshake(config, address.port(), "localhost").await.unwrap();
        assert_eq!(rejected.take(), None);
        // A CA-valid certificate passes even when it differs from a pin.
        let other = self_signed(&["localhost"]);
        let (config, _) =
            client_config_with_roots(roots_of(Some(&ca)), Some(fingerprint(&other.der)));
        handshake(config, address.port(), "localhost").await.unwrap();
    }

    #[tokio::test]
    async fn ca_valid_certificate_with_wrong_hostname_is_refused_even_with_a_pin_for_another_leaf()
    {
        let ca = ca();
        let leaf = signed_by(&["other.example"], &ca);
        let other = self_signed(&["localhost"]);
        let address = serve(vec![leaf.der.clone()], &leaf.key, ok).await;
        let (config, _) =
            client_config_with_roots(roots_of(Some(&ca)), Some(fingerprint(&other.der)));
        assert!(handshake(config, address.port(), "localhost").await.is_err());
    }

    #[tokio::test]
    async fn pinned_verifier_still_checks_the_handshake_signature() {
        let leaf = self_signed(&["localhost"]);
        let impostor = rcgen::KeyPair::generate().unwrap();
        // The pinned certificate, presented by a server holding a different key.
        let address = serve(vec![leaf.der.clone()], &impostor, ok).await;
        let (config, _) =
            client_config_with_roots(roots_of(None), Some(fingerprint(&leaf.der)));
        assert!(handshake(config, address.port(), "localhost").await.is_err());
    }

    #[test]
    fn pins_never_reach_the_sealskin_client() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sealskin = std::fs::read_to_string(src.join("sealskin.rs")).unwrap();
        assert!(!sealskin.contains("crate::tls"), "sealskin.rs must not use pins");
        assert!(!sealskin.contains("use_preconfigured_tls"));
    }

    #[test]
    fn the_default_connectors_still_build_with_one_provider() {
        // What tungstenite's and reqwest's default (SealSkin) paths do: no explicit
        // provider. With a second provider compiled in, this panics.
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        reqwest::Client::builder().build().unwrap();
    }
}
