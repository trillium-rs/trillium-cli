use clap::Args;
use std::{fs, path::PathBuf};

#[derive(Args, Debug, Clone, Default)]
pub struct ServerTls {
    /// Path to a tls certificate for trillium_rustls
    ///
    /// This will panic unless rustls_key is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem`
    /// For development, try using mkcert
    #[cfg(feature = "rustls")]
    #[arg(long, env, requires = "rustls_key")]
    #[cfg_attr(feature = "native-tls", arg(conflicts_with_all(["native_tls_identity", "native_tls_password"])))]
    rustls_cert: Option<PathBuf>,

    /// The path to a tls key file for trillium_rustls
    ///
    /// This will panic unless rustls_cert is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem`
    /// For development, try using mkcert
    #[cfg(feature = "rustls")]
    #[arg(long, env, requires = "rustls_cert")]
    #[cfg_attr(feature = "native-tls", arg(conflicts_with_all(["native_tls_identity", "native_tls_password"])))]
    rustls_key: Option<PathBuf>,

    #[cfg(feature = "native-tls")]
    #[arg(long, env, requires = "native_tls_password")]
    #[cfg_attr(feature = "rustls", arg(conflicts_with_all(["rustls_cert", "rustls_key"])))]
    native_tls_identity: Option<PathBuf>,

    #[cfg(feature = "native-tls")]
    #[arg(long, env, requires = "native_tls_identity")]
    #[cfg_attr(feature = "rustls", arg(conflicts_with_all(["rustls_cert", "rustls_key"])))]
    native_tls_password: Option<String>,
}

impl ServerTls {
    #[cfg(feature = "rustls")]
    pub fn rustls_acceptor(&self) -> Option<trillium_rustls::RustlsAcceptor> {
        if let (Some(cert), Some(key)) = (&self.rustls_cert, &self.rustls_key) {
            Some(trillium_rustls::RustlsAcceptor::from_single_cert(
                &fs::read(cert).unwrap(),
                &fs::read(key).unwrap(),
            ))
        } else {
            None
        }
    }
    #[cfg(feature = "native-tls")]
    pub fn native_tls_acceptor(&self) -> Option<trillium_native_tls::NativeTlsAcceptor> {
        if let (Some(id), Some(pass)) = (&self.native_tls_identity, &self.native_tls_password) {
            Some(trillium_native_tls::NativeTlsAcceptor::from_pkcs12(
                &fs::read(id).unwrap(),
                pass,
            ))
        } else {
            None
        }
    }
}
