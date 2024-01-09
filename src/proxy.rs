use crate::{
    client_tls::{parse_url, ClientTls},
    server_tls::ServerTls,
};
use clap::{Parser, ValueEnum};
use std::fmt::Debug;
use trillium::{Conn, Method, Status};
use trillium_logger::Logger;
use trillium_proxy::{
    upstream::{
        ConnectionCounting, ForwardProxy, IntoUpstreamSelector, RandomSelector, RoundRobin,
        UpstreamSelector,
    },
    Client, ForwardProxyConnect, Proxy, Url,
};
use trillium_smol::ClientConfig;

#[derive(Clone, Copy, Debug, ValueEnum, Default, PartialEq, Eq)]
enum UpstreamSelectorStrategy {
    #[default]
    RoundRobin,
    ConnectionCounting,
    Random,
    Forward,
}

#[derive(Parser, Debug)]
pub struct ProxyCli {
    #[arg(env, value_parser = parse_url)]
    upstream: Vec<Url>,

    #[arg(short, long, env, default_value_t, value_enum)]
    strategy: UpstreamSelectorStrategy,

    /// Local host or ip to listen on
    #[arg(short = 'o', long, env, default_value = "localhost")]
    host: String,

    /// Local port to listen on
    #[arg(short, long, env, default_value = "8080")]
    port: u16,

    #[command(flatten)]
    server_tls: ServerTls,

    /// tls implementation
    ///
    /// required if the upstream url is https.
    #[arg(short, long, verbatim_doc_comment, default_value_t, value_enum)]
    client_tls: ClientTls,

    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

impl ProxyCli {
    pub fn build_upstream(&self) -> Box<dyn UpstreamSelector> {
        if self.strategy == UpstreamSelectorStrategy::Forward {
            if !self.upstream.is_empty() {
                panic!("forward proxy does not take upstreams");
            } else {
                println!("Running in forward proxy mode");
            }
        } else if self.upstream.is_empty() {
            panic!("upstream required unless --strategy forward is provided");
        } else if self.upstream.len() == 1 {
            let upstream = self.upstream[0].clone().into_upstream();
            println!("Proxying to {upstream}");
            return upstream.boxed();
        } else {
            println!(
                "Forwarding to {} with strategy {}",
                self.upstream
                    .iter()
                    .map(|u| u.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                self.strategy.to_possible_value().unwrap().get_name()
            );
        }

        match self.strategy {
            UpstreamSelectorStrategy::RoundRobin => RoundRobin::new(self.upstream.clone()).boxed(),
            UpstreamSelectorStrategy::ConnectionCounting => {
                ConnectionCounting::new(self.upstream.clone()).boxed()
            }
            UpstreamSelectorStrategy::Random => RandomSelector::new(self.upstream.clone()).boxed(),
            UpstreamSelectorStrategy::Forward => ForwardProxy.boxed(),
        }
    }

    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .init();

        let server = (
            Logger::new(),
            if self.strategy == UpstreamSelectorStrategy::Forward {
                Some((
                    ForwardProxyConnect::new(ClientConfig::default()),
                    |conn: Conn| async move {
                        if conn.status() == Some(Status::Ok) && conn.method() == Method::Connect {
                            conn.halt()
                        } else {
                            conn
                        }
                    },
                ))
            } else {
                None
            },
            Proxy::new(
                Client::from(self.client_tls).with_default_pool(),
                self.build_upstream(),
            )
            .with_via_pseudonym("trillium-proxy")
            .with_websocket_upgrades()
            .proxy_not_found(),
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
