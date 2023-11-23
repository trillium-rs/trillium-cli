use crate::RootPath;
use clap::Parser;
use std::{fmt::Debug, fs, io::Write, path::PathBuf};
use trillium_logger::Logger;
use trillium_native_tls::NativeTlsAcceptor;
use trillium_proxy::Proxy;
use trillium_rustls::{RustlsAcceptor, RustlsConfig};
use trillium_smol::ClientConfig;
use trillium_static::StaticFileHandler;

#[derive(Parser, Debug)]
pub struct StaticCli {
    /// Filesystem path to serve
    ///
    /// Defaults to the current working directory
    #[arg(default_value_t)]
    root: RootPath,

    /// Local host or ip to listen on
    #[arg(short = 'o', long, env, default_value = "localhost")]
    host: String,

    /// Local port to listen on
    #[arg(short, long, env, default_value = "8080")]
    port: u16,

    /// Path to a tls certificate for tide_rustls
    ///
    /// This will panic unless rustls_key is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls_cert ./cert.pem --rustls_key ./key.pem`
    /// For development, try using mkcert
    #[arg(long, env)]
    rustls_cert: Option<PathBuf>,

    /// The path to a tls key file for tide_rustls
    ///
    /// This will panic unless rustls_cert is also provided. Providing
    /// both rustls_key and rustls_cert enables tls.
    ///
    /// Example: `--rustls_cert ./cert.pem --rustls_key ./key.pem`
    /// For development, try using mkcert
    #[arg(long, env)]
    rustls_key: Option<PathBuf>,

    #[arg(long, env)]
    native_tls_identity: Option<PathBuf>,

    #[arg(long, env)]
    native_tls_password: Option<String>,

    /// Host to forward (reverse proxy) not-found requests to
    ///
    /// This forwards any request that would otherwise be a 404 Not
    /// Found to the specified listener spec.
    ///
    /// Examples:
    ///    `--forward localhost:8081`
    ///    `--forward http://localhost:8081`
    ///    `--forward https://localhost:8081`
    ///
    /// Note: http+unix:// schemes are not yet supported
    #[arg(short, long, env = "FORWARD")]
    forward: Option<String>,

    #[arg(short, long, env)]
    index: Option<String>,

    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

impl StaticCli {
    pub fn root(&self) -> &RootPath {
        &self.root
    }

    pub fn forward(&self) -> Option<&str> {
        self.forward.as_deref()
    }

    pub fn index(&self) -> Option<&str> {
        self.index.as_deref()
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn rustls_acceptor(&self) -> Option<RustlsAcceptor> {
        match &self {
            StaticCli {
                rustls_cert: Some(_),
                rustls_key: None,
                ..
            }
            | StaticCli {
                rustls_cert: None,
                rustls_key: Some(_),
                ..
            } => {
                panic!("rustls_cert_path must be combined with rustls_key_path");
            }

            StaticCli {
                rustls_cert: Some(cert),
                rustls_key: Some(key),
                native_tls_identity: None,
                ..
            } => Some(RustlsAcceptor::from_single_cert(
                &fs::read(cert).unwrap(),
                &fs::read(key).unwrap(),
            )),

            StaticCli {
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
            StaticCli {
                native_tls_identity: Some(_),
                native_tls_password: None,
                ..
            }
            | StaticCli {
                native_tls_identity: None,
                native_tls_password: Some(_),
                ..
            } => {
                panic!("native_tls_identity_path and native_tls_identity_password must be used together");
            }

            StaticCli {
                rustls_cert: None,
                rustls_key: None,
                native_tls_identity: Some(x),
                native_tls_password: Some(y),
                ..
            } => Some(NativeTlsAcceptor::from_pkcs12(&fs::read(x).unwrap(), y)),

            StaticCli {
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
            .format(|buf, record| writeln!(buf, "{}", record.args()))
            .init();

        let path = self.root().clone();
        let mut static_file_handler = StaticFileHandler::new(path);
        if let Some(index) = self.index() {
            static_file_handler = static_file_handler.with_index_file(index);
        }

        let server = (
            Logger::new(),
            self.forward().map(|url| {
                Proxy::new(
                    RustlsConfig::default().with_tcp_config(ClientConfig::default()),
                    url,
                )
            }),
            static_file_handler,
        );

        let config = trillium_smol::config()
            .with_port(self.port())
            .with_host(self.host());

        if let Some(acceptor) = self.rustls_acceptor() {
            config.with_acceptor(acceptor).run(server);
        } else if let Some(acceptor) = self.native_tls_acceptor() {
            config.with_acceptor(acceptor).run(server);
        } else {
            config.run(server);
        }
    }
}
