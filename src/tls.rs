use trillium_client::{Client, Url};
use trillium_smol::ClientConfig;

#[cfg(feature = "rustls")]
use std::sync::Arc;
#[cfg(feature = "rustls")]
use trillium_rustls::rustls::{
    self, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
};

#[derive(clap::ValueEnum, Debug, Eq, PartialEq, Clone, Copy, Default)]
pub enum Tls {
    #[cfg_attr(
        all(
            not(feature = "native-tls"),
            not(feature = "rustls"),
            not(feature = "openssl")
        ),
        default
    )]
    None,

    #[cfg(feature = "rustls")]
    #[cfg_attr(feature = "rustls", default)]
    Rustls,

    #[cfg(feature = "native-tls")]
    #[cfg_attr(
        all(
            feature = "native-tls",
            not(feature = "rustls"),
            not(feature = "openssl")
        ),
        default
    )]
    Native,

    #[cfg(feature = "openssl")]
    #[cfg_attr(
        all(
            feature = "openssl",
            not(feature = "rustls"),
            not(feature = "native-tls")
        ),
        default
    )]
    Openssl,
}

impl From<Tls> for Client {
    fn from(value: Tls) -> Self {
        match value {
            Tls::None => Client::new(ClientConfig::default()),

            #[cfg(all(feature = "rustls", feature = "h3"))]
            Tls::Rustls => Client::new_with_quic(
                trillium_rustls::RustlsConfig::<ClientConfig>::default(),
                trillium_quinn::ClientQuicConfig::with_webpki_roots(),
            ),

            #[cfg(all(feature = "rustls", not(feature = "h3")))]
            Tls::Rustls => Client::new(trillium_rustls::RustlsConfig::<ClientConfig>::default()),

            #[cfg(feature = "native-tls")]
            Tls::Native => {
                Client::new(trillium_native_tls::NativeTlsConfig::<ClientConfig>::default())
            }

            #[cfg(feature = "openssl")]
            Tls::Openssl => Client::new(trillium_openssl::OpenSslConfig::<ClientConfig>::default()),
        }
    }
}

/// Build a [`Client`] for the given tls backend, optionally skipping certificate
/// verification.
///
/// `insecure` is only honored for the `rustls` backend; with any other backend it logs a
/// warning and falls back to verified connections.
pub fn build_client(tls: Tls, insecure: bool) -> Client {
    if !insecure {
        return Client::from(tls);
    }

    #[cfg(feature = "rustls")]
    if tls == Tls::Rustls {
        return insecure_rustls_client();
    }

    log::warn!("--insecure is only supported with --tls rustls; verifying certificates");
    Client::from(tls)
}

/// A rustls [`ServerCertVerifier`] that accepts any certificate. Deliberately not easy to reach
/// — it disables server authentication entirely and exists only for the `--insecure` CLI flag.
#[cfg(feature = "rustls")]
#[derive(Debug)]
struct AcceptAnyServerCert(Arc<CryptoProvider>);

#[cfg(feature = "rustls")]
impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(feature = "rustls")]
fn insecure_rustls_config() -> rustls::ClientConfig {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = Arc::new(AcceptAnyServerCert(provider.clone()));
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("crypto provider supports default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config
}

#[cfg(feature = "rustls")]
fn insecure_rustls_client() -> Client {
    let rustls_config = insecure_rustls_config();

    #[cfg(feature = "h3")]
    let client = {
        let quic =
            trillium_quinn::ClientQuicConfig::from_rustls_client_config(rustls_config.clone());
        Client::new_with_quic(
            trillium_rustls::RustlsConfig::new(rustls_config, ClientConfig::default()),
            quic,
        )
    };
    #[cfg(not(feature = "h3"))]
    let client = Client::new(trillium_rustls::RustlsConfig::new(
        rustls_config,
        ClientConfig::default(),
    ));

    client
}

pub fn parse_url(src: &str) -> Result<Url, String> {
    if src.starts_with("http") {
        src.parse::<Url>().map_err(|e| e.to_string())
    } else {
        format!("http://{}", src)
            .parse::<Url>()
            .map_err(|e| e.to_string())
    }
}
