use crate::tls::Tls;
use clap::Args;
#[cfg(all(unix, any(feature = "serve", feature = "proxy")))]
use std::os::fd::AsFd;
#[cfg(all(windows, any(feature = "serve", feature = "proxy")))]
use std::os::windows::io::AsSocket;
#[cfg(any(
    feature = "native-tls",
    feature = "openssl",
    feature = "rustls",
    feature = "h3"
))]
use std::{fs, path::PathBuf};
#[cfg(any(feature = "serve", feature = "proxy"))]
use trillium::Handler;
#[cfg(any(feature = "serve", feature = "proxy"))]
use trillium_server_common::UdpTransport;

#[derive(Args, Debug, Clone, Default)]
pub struct ServerTls {
    /// Path to a tls certificate file
    ///
    /// This will fail unless key is also provided. Providing
    /// both cert and key enables tls.
    ///
    /// Example: `--cert ./cert.pem --key ./key.pem`
    /// For development, try using mkcert or rcgen
    #[cfg(any(
        feature = "native-tls",
        feature = "openssl",
        feature = "rustls",
        feature = "h3"
    ))]
    #[arg(long, env, requires = "key")]
    cert: Option<PathBuf>,

    /// The path to a tls key file
    ///
    /// This will fail unless cert is also provided. Providing
    /// both cert and key enables tls.
    ///
    /// Example: `--cert ./cert.pem --key ./key.pem`
    /// For development, try using mkcert or rcgen
    #[cfg(any(
        feature = "native-tls",
        feature = "openssl",
        feature = "rustls",
        feature = "h3"
    ))]
    #[arg(long, env, requires = "cert")]
    key: Option<PathBuf>,

    #[arg(long, env, value_enum, default_value_t)]
    tls: Tls,
}

#[cfg(all(unix, any(feature = "serve", feature = "proxy")))]
pub(crate) trait SocketTransport: UdpTransport + AsFd {}
#[cfg(all(unix, any(feature = "serve", feature = "proxy")))]
impl<T: UdpTransport + AsFd> SocketTransport for T {}

#[cfg(all(windows, any(feature = "serve", feature = "proxy")))]
pub(crate) trait SocketTransport: UdpTransport + AsSocket {}
#[cfg(all(windows, any(feature = "serve", feature = "proxy")))]
impl<T: UdpTransport + AsSocket> SocketTransport for T {}

impl ServerTls {
    #[cfg(any(
        feature = "native-tls",
        feature = "openssl",
        feature = "rustls",
        feature = "h3"
    ))]
    fn cert_and_key(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        Some((
            fs::read(self.cert.as_deref()?).ok()?,
            fs::read(self.key.as_deref()?).ok()?,
        ))
    }

    #[cfg(any(feature = "serve", feature = "proxy"))]
    pub(crate) fn run_with_tls<S: trillium_server_common::Server>(
        &self,
        config: trillium_server_common::Config<S, ()>,
        handler: impl Handler,
    ) where
        S::Runtime: Unpin,
        S::UdpTransport: SocketTransport,
    {
        match self.tls {
            Tls::None => {}

            #[cfg(feature = "rustls")]
            Tls::Rustls => {
                if let Some(acceptor) = self.rustls_acceptor() {
                    #[cfg(feature = "h3")]
                    if let Some(quic) = self.quic() {
                        config.with_acceptor(acceptor).with_quic(quic).run(handler);
                        return;
                    }

                    config.with_acceptor(acceptor).run(handler);
                    return;
                }
            }

            #[cfg(feature = "openssl")]
            Tls::Openssl => {
                if let Some(acceptor) = self.openssl_acceptor() {
                    #[cfg(feature = "h3")]
                    if let Some(quic) = self.quic() {
                        config.with_acceptor(acceptor).with_quic(quic).run(handler);
                        return;
                    }

                    config.with_acceptor(acceptor).run(handler);
                    return;
                }
            }

            #[cfg(feature = "native-tls")]
            Tls::Native => {
                if let Some(acceptor) = self.native_tls_acceptor() {
                    #[cfg(feature = "h3")]
                    if let Some(quic) = self.quic() {
                        config.with_acceptor(acceptor).with_quic(quic).run(handler);
                        return;
                    }

                    config.with_acceptor(acceptor).run(handler);
                    return;
                }
            }
        }

        config.run(handler);
    }

    #[cfg(feature = "rustls")]
    pub fn rustls_acceptor(&self) -> Option<trillium_rustls::RustlsAcceptor> {
        let (cert, key) = self.cert_and_key()?;
        Some(trillium_rustls::RustlsAcceptor::from_single_cert(
            &cert, &key,
        ))
    }

    #[cfg(feature = "h3")]
    pub fn quic(&self) -> Option<trillium_quinn::QuicConfig> {
        let (cert, key) = self.cert_and_key()?;
        Some(trillium_quinn::QuicConfig::from_single_cert(&cert, &key))
    }

    #[cfg(feature = "openssl")]
    pub fn openssl_acceptor(&self) -> Option<trillium_openssl::OpenSslAcceptor> {
        let (cert, key) = self.cert_and_key()?;
        Some(trillium_openssl::OpenSslAcceptor::from_single_cert(
            &cert, &key,
        ))
    }

    #[cfg(feature = "native-tls")]
    pub fn native_tls_acceptor(&self) -> Option<trillium_native_tls::NativeTlsAcceptor> {
        let (cert, key) = self.cert_and_key()?;
        Some(trillium_native_tls::NativeTlsAcceptor::from_cert_and_key(
            &cert, &key,
        ))
    }
}
