use trillium_client::{Client, Url};
use trillium_smol::ClientConfig;

#[derive(clap::ValueEnum, Debug, Eq, PartialEq, Clone, Copy, Default)]
pub enum ClientTls {
    None,
    #[cfg(feature = "rustls")]
    #[cfg_attr(feature = "rustls", default)]
    Rustls,
    #[cfg(feature = "native-tls")]
    #[cfg_attr(all(feature = "native-tls", not(feature = "rustls")), default)]
    Native,
}

impl From<ClientTls> for Client {
    fn from(value: ClientTls) -> Self {
        match value {
            ClientTls::None => Client::new(ClientConfig::default()),
            #[cfg(feature = "rustls")]
            ClientTls::Rustls => {
                Client::new(trillium_rustls::RustlsConfig::<ClientConfig>::default())
            }
            #[cfg(feature = "native-tls")]
            ClientTls::Native => {
                Client::new(trillium_native_tls::NativeTlsConfig::<ClientConfig>::default())
            }
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
