use crate::{
    client_tls::{parse_url, ClientTls},
    server_tls::ServerTls,
};
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use std::{fmt::Debug, io::Write};
use trillium_logger::Logger;
use trillium_proxy::{Client, Proxy, Url};
use trillium_static::StaticFileHandler;

mod root_path;
use root_path::RootPath;

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

    #[command(flatten)]
    server_tls: ServerTls,

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
    #[arg(short, long, env = "FORWARD", value_parser = parse_url)]
    forward: Option<Url>,

    #[arg(short, long, env)]
    index: Option<String>,

    #[command(flatten)]
    verbose: Verbosity,
}

impl StaticCli {
    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .format(|buf, record| writeln!(buf, "{}", record.args()))
            .init();

        let path = self.root.clone();
        let mut static_file_handler = StaticFileHandler::new(path);
        if let Some(index) = &self.index {
            static_file_handler = static_file_handler.with_index_file(index);
        }

        let server = (
            Logger::new(),
            self.forward
                .clone()
                .map(|url| Proxy::new(Client::from(ClientTls::default()).with_default_pool(), url)),
            static_file_handler,
        );

        let config = trillium_smol::config()
            .with_port(self.port)
            .with_host(&self.host);

        #[cfg(feature = "rustls")]
        if let Some(acceptor) = self.server_tls.rustls_acceptor() {
            config.with_acceptor(acceptor).run(server);
            return;
        }

        #[cfg(feature = "native-tls")]
        if let Some(acceptor) = self.server_tls.native_tls_acceptor() {
            config.with_acceptor(acceptor).run(server);
            return;
        }

        config.run(server);
    }
}
