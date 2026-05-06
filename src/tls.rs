use trillium_client::{Client, Url};
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

pub fn parse_url(src: &str) -> Result<Url, String> {
    if src.starts_with("http") {
        src.parse::<Url>().map_err(|e| e.to_string())
    } else {
        format!("http://{}", src)
            .parse::<Url>()
            .map_err(|e| e.to_string())
    }
}
