use crate::{
    server_tls::ServerTls,
    tls::{Tls, parse_url},
};
use clap::{Parser, ValueEnum};
use std::{fmt::Debug, time::Duration};
use trillium::{Conn, Method, Status};
use trillium_cache::{InMemoryStorage, client::Cache};
use trillium_logger::{
    Logger,
    client::{ClientLogger, dev_formatter as client_dev_formatter},
    dev_formatter,
};
use trillium_proxy::{
    ForwardProxyConnect, Proxy, Url,
    upstream::{
        ConnectionCounting, ForwardProxy, IntoUpstreamSelector, RandomSelector, RoundRobin,
        UpstreamSelector,
    },
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
    client_tls: Tls,

    /// skip upstream TLS certificate verification (rustls only)
    ///
    /// dangerous: this disables authentication of the upstream server.
    #[arg(short = 'k', long, verbatim_doc_comment)]
    insecure: bool,

    /// disable response compression (gzip/brotli/zstd)
    #[arg(long)]
    no_compress: bool,

    /// disable response caching entirely
    #[arg(long, help_heading = "Cache")]
    no_cache: bool,

    /// maximum total in-memory cache size, e.g. 256MiB, 1GB
    #[arg(long, value_parser = parse_size, default_value = "256MiB", conflicts_with = "no_cache", help_heading = "Cache")]
    cache_capacity: u64,

    /// maximum cacheable response body; larger responses stream through uncached
    #[arg(long, value_parser = parse_size, default_value = "16MiB", conflicts_with = "no_cache", help_heading = "Cache")]
    cache_max_body: u64,

    /// evict cache entries not read within this duration, e.g. 5m, 1h
    #[arg(long, value_parser = humantime::parse_duration, conflicts_with = "no_cache", help_heading = "Cache")]
    cache_time_to_idle: Option<Duration>,

    /// evict cache entries this long after they are stored, e.g. 1h, 24h
    #[arg(long, value_parser = humantime::parse_duration, conflicts_with = "no_cache", help_heading = "Cache")]
    cache_time_to_live: Option<Duration>,

    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity,
}

fn parse_size(s: &str) -> Result<u64, String> {
    let size = size::Size::from_str(s).map_err(|e| e.to_string())?;
    u64::try_from(size.bytes()).map_err(|_| "size must not be negative".to_string())
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

        // `Option<T>` is itself a `ClientHandler`, so a `None` here drops caching
        // entirely instead of installing a do-nothing cache.
        let cache = (!self.no_cache).then(|| {
            let mut storage = InMemoryStorage::new().with_max_capacity_bytes(self.cache_capacity);
            if let Some(time_to_idle) = self.cache_time_to_idle {
                storage = storage.with_time_to_idle(time_to_idle);
            }
            if let Some(time_to_live) = self.cache_time_to_live {
                storage = storage.with_time_to_live(time_to_live);
            }
            Cache::new(storage)
                .with_max_cacheable_size(self.cache_max_body)
                .shared()
        });

        let client = crate::tls::build_client(self.client_tls, self.insecure).with_handler((
            ClientLogger::new().with_formatter(("-> ", client_dev_formatter)),
            cache,
        ));

        let server = (
            Logger::new().with_formatter(("<- ", dev_formatter)),
            // `Option<Handler>` is a `Handler`, so `None` skips compression entirely.
            (!self.no_compress).then(trillium_compression::compression),
            trillium_caching_headers::caching_headers(),
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
            Proxy::new(client, self.build_upstream())
                .with_via_pseudonym("trillium-proxy")
                .with_websocket_upgrades()
                .proxy_not_found(),
        );

        let config = trillium_smol::config()
            .with_port(self.port)
            .with_host(&self.host);

        #[cfg(feature = "rustls")]
        if let Some(acceptor) = self.server_tls.rustls_acceptor() {
            #[cfg(feature = "h3")]
            if let Some(quic) = self.server_tls.quic() {
                config.with_acceptor(acceptor).with_quic(quic).run(server);
                return;
            }

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
