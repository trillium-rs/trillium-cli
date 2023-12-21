use crate::client::{parse_url, TlsType};

use clap::Parser;
use std::{fmt::Debug, fs, path::PathBuf};
use trillium_logger::Logger;
use trillium_native_tls::NativeTlsAcceptor;
use trillium_proxy::Proxy;
use trillium_rustls::RustlsAcceptor;

use url::Url;

#[derive(Parser, Debug)]
pub struct ProxyCli {
    /// Host to forward (reverse proxy) requests to
    ///
    /// This forwards any request that would otherwise be a 404 Not
    /// Found to the specified listener spec.
    ///
    /// Note: http+unix:// schemes are not yet supported
    #[arg(value_parser = parse_url)]
    upstream: Url,

    /// Local host or ip to listen on
    #[arg(short = 'o', long, env, default_value = "localhost")]
    host: String,

    /// Local port to listen on
    #[arg(short, long, env, default_value = "8080")]
    port: u16,

    /// Path to a tls certificate for trillium_rustls
    ///
    /// This will panic unless rustls_key is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem`
    /// For development, try using mkcert
    #[arg(long, env)]
    rustls_cert: Option<PathBuf>,

    /// The path to a tls key file for trillium_rustls
    ///
    /// This will panic unless rustls_cert is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem`
    /// For development, try using mkcert
    #[arg(long, env)]
    rustls_key: Option<PathBuf>,

    #[arg(long, env)]
    native_tls_identity: Option<PathBuf>,

    #[arg(long, env)]
    native_tls_password: Option<String>,

    /// tls implementation. options: rustls, native-tls, none
    ///
    /// required if the upstream url is https.
    #[arg(short, long, default_value = "rustls", verbatim_doc_comment)]
    client_tls: TlsType,

    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

impl ProxyCli {
    pub fn rustls_acceptor(&self) -> Option<RustlsAcceptor> {
        match &self {
            Self {
                rustls_cert: Some(_),
                rustls_key: None,
                ..
            }
            | Self {
                rustls_cert: None,
                rustls_key: Some(_),
                ..
            } => {
                panic!("rustls_cert_path must be combined with rustls_key_path");
            }

            Self {
                rustls_cert: Some(cert),
                rustls_key: Some(key),
                native_tls_identity: None,
                ..
            } => Some(RustlsAcceptor::from_single_cert(
                &fs::read(cert).unwrap(),
                &fs::read(key).unwrap(),
            )),

            Self {
                rustls_cert: Some(_),
                rustls_key: Some(_),
                native_tls_identity: Some(_),
                ..
            } => {
                panic!("sorry, i'm not sure what to do when provided with both native tls and rustls info. please pick one or the other")
            }

            _ => None,
        }
    }

    pub fn native_tls_acceptor(&self) -> Option<NativeTlsAcceptor> {
        match &self {
            Self {
                native_tls_identity: Some(_),
                native_tls_password: None,
                ..
            }
            | Self {
                native_tls_identity: None,
                native_tls_password: Some(_),
                ..
            } => {
                panic!("native_tls_identity_path and native_tls_identity_password must be used together");
            }

            Self {
                rustls_cert: None,
                rustls_key: None,
                native_tls_identity: Some(x),
                native_tls_password: Some(y),
                ..
            } => Some(NativeTlsAcceptor::from_pkcs12(&fs::read(x).unwrap(), y)),

            Self {
                rustls_cert: Some(_),
                rustls_key: Some(_),
                native_tls_identity: Some(_),
                ..
            } => {
                panic!("sorry, i'm not sure what to do when provided with both native tls and rustls info. please pick one or the other")
            }

            _ => None,
        }
    }

    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .init();

        if self.client_tls == TlsType::None && self.upstream.scheme() == "https" {
            eprintln!("cannot use tls type none with an upstream scheme of https");
            std::process::exit(-1);
        }
        println!("Proxying to {}", &self.upstream);

        let server = (
            Logger::new(),
            Proxy::new(self.client_tls, self.upstream.clone())
                .with_via_pseudonym("trillium-proxy")
                .proxy_not_found(),
        );

        let config = trillium_smol::config()
            .with_port(self.port)
            .with_host(&self.host);

        if let Some(acceptor) = self.rustls_acceptor() {
            config.with_acceptor(acceptor).run(server);
        } else if let Some(acceptor) = self.native_tls_acceptor() {
            config.with_acceptor(acceptor).run(server);
        } else {
            config.run(server);
        }
    }
}
