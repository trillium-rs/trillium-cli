#[cfg(any(feature = "client", feature = "proxy"))]
use std::path::PathBuf;
#[cfg(all(feature = "rustls", any(feature = "client", feature = "proxy")))]
use std::sync::Arc;
use trillium_client::Client;
#[cfg(all(feature = "rustls", any(feature = "client", feature = "proxy")))]
use trillium_rustls::rustls::{
    self, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use trillium_smol::ClientConfig;

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

    // Backend precedence for the default is rustls > native-tls > openssl, so native-tls is the
    // default whenever it's enabled and rustls is not — even if openssl is also enabled.
    #[cfg(feature = "native-tls")]
    #[cfg_attr(all(feature = "native-tls", not(feature = "rustls")), default)]
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

/// The base tcp connector config shared by every client this crate builds.
///
/// `with_nodelay(true)` disables Nagle's algorithm so small requests aren't
/// delayed waiting to coalesce — the right default for an interactive CLI.
pub fn client_tcp_config() -> ClientConfig {
    ClientConfig::default().with_nodelay(true)
}

impl From<Tls> for Client {
    fn from(value: Tls) -> Self {
        match value {
            Tls::None => Client::new(client_tcp_config()),

            #[cfg(all(feature = "rustls", feature = "h3"))]
            Tls::Rustls => Client::new_with_quic(
                trillium_rustls::RustlsConfig::<ClientConfig>::default()
                    .with_tcp_config(client_tcp_config()),
                trillium_quinn::ClientQuicConfig::with_webpki_roots(),
            ),

            #[cfg(all(feature = "rustls", not(feature = "h3")))]
            Tls::Rustls => Client::new(
                trillium_rustls::RustlsConfig::<ClientConfig>::default()
                    .with_tcp_config(client_tcp_config()),
            ),

            #[cfg(feature = "native-tls")]
            Tls::Native => Client::new(
                trillium_native_tls::NativeTlsConfig::<ClientConfig>::default()
                    .with_tcp_config(client_tcp_config()),
            ),

            #[cfg(feature = "openssl")]
            Tls::Openssl => Client::new(
                trillium_openssl::OpenSslConfig::<ClientConfig>::default()
                    .with_tcp_config(client_tcp_config()),
            ),
        }
    }
}

/// Build a [`Client`] for the given tls backend, optionally skipping certificate
/// verification.
///
/// `insecure` is only honored for the `rustls` backend; with any other backend it logs a
/// warning and falls back to verified connections.
///
/// `unix_socket`, when set, dials that Unix domain socket for every request
/// instead of opening a tcp connection — the request url then only supplies
/// request metadata (path, query, `Host`), not the connection address.
#[cfg(any(feature = "client", feature = "proxy"))]
pub fn build_client(tls: Tls, insecure: bool, unix_socket: Option<PathBuf>) -> Client {
    #[cfg(unix)]
    if let Some(path) = unix_socket {
        return build_unix_client(tls, insecure, path);
    }
    // On non-unix targets there is no Unix-socket connector, so the option is
    // always `None`; bind it so the parameter isn't flagged as unused.
    #[cfg(not(unix))]
    let _ = unix_socket;

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

/// Build a [`Client`] that dials a fixed Unix domain socket instead of tcp,
/// composing the requested tls backend over the socket exactly as the tcp path
/// does.
///
/// QUIC/h3 is a UDP transport and has no Unix-socket equivalent (the connector's
/// `Udp` type is `()`), so the rustls arm here never wires up quic — `--tls
/// rustls` over a socket is tcp-style https only.
#[cfg(all(unix, any(feature = "client", feature = "proxy")))]
// With no tls backend the only arm is `Tls::None`, which ignores `insecure`.
#[cfg_attr(
    not(any(feature = "rustls", feature = "native-tls", feature = "openssl")),
    allow(unused_variables)
)]
fn build_unix_client(tls: Tls, insecure: bool, path: PathBuf) -> Client {
    let inner = trillium_smol::UnixClientConfig::new(path);

    match tls {
        Tls::None => Client::new(inner),

        #[cfg(feature = "rustls")]
        Tls::Rustls => {
            // `--insecure` swaps in the accept-any verifier; otherwise reuse the
            // default rustls client config the tcp path would build.
            let rustls_config: trillium_rustls::RustlsClientConfig = if insecure {
                insecure_rustls_config().into()
            } else {
                trillium_rustls::RustlsConfig::<ClientConfig>::default().rustls_config
            };
            Client::new(trillium_rustls::RustlsConfig::new(rustls_config, inner))
        }

        #[cfg(feature = "native-tls")]
        Tls::Native => {
            if insecure {
                log::warn!(
                    "--insecure is only supported with --tls rustls; verifying certificates"
                );
            }
            Client::new(trillium_native_tls::NativeTlsConfig::from(inner))
        }

        #[cfg(feature = "openssl")]
        Tls::Openssl => {
            if insecure {
                log::warn!(
                    "--insecure is only supported with --tls rustls; verifying certificates"
                );
            }
            let ssl_config = trillium_openssl::OpenSslConfig::<ClientConfig>::default().ssl_config;
            Client::new(trillium_openssl::OpenSslConfig::new(ssl_config, inner))
        }
    }
}

/// A rustls [`ServerCertVerifier`] that accepts any certificate. Deliberately not easy to reach
/// — it disables server authentication entirely and exists only for the `--insecure` CLI flag.
#[cfg(all(feature = "rustls", any(feature = "client", feature = "proxy")))]
#[derive(Debug)]
struct AcceptAnyServerCert(Arc<CryptoProvider>);

#[cfg(all(feature = "rustls", any(feature = "client", feature = "proxy")))]
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

#[cfg(all(feature = "rustls", any(feature = "client", feature = "proxy")))]
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

#[cfg(all(feature = "rustls", any(feature = "client", feature = "proxy")))]
fn insecure_rustls_client() -> Client {
    let rustls_config = insecure_rustls_config();

    #[cfg(feature = "h3")]
    let client = {
        let quic =
            trillium_quinn::ClientQuicConfig::from_rustls_client_config(rustls_config.clone());
        Client::new_with_quic(
            trillium_rustls::RustlsConfig::new(rustls_config, client_tcp_config()),
            quic,
        )
    };
    #[cfg(not(feature = "h3"))]
    let client = Client::new(trillium_rustls::RustlsConfig::new(
        rustls_config,
        client_tcp_config(),
    ));

    client
}

#[cfg(any(
    feature = "client",
    feature = "bench",
    feature = "proxy",
    feature = "serve"
))]
pub fn parse_url(src: &str) -> Result<trillium_client::Url, String> {
    use trillium_client::Url;
    if src.starts_with("http") {
        src.parse::<Url>().map_err(|e| e.to_string())
    } else {
        format!("http://{}", src)
            .parse::<Url>()
            .map_err(|e| e.to_string())
    }
}
