use crate::{
    cache::{self, CacheSpec},
    ratelimit::RateLimit,
    server_tls::ServerTls,
    tls::{Tls, parse_url},
};
use clap::{Parser, ValueEnum};
use std::{fmt::Debug, path::PathBuf, time::Duration};
use trillium::{Conn, Method, Status};
use trillium_client::Client;
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

    /// route upstream DNS through an encrypted resolver instead of the system resolver
    ///
    /// the scheme selects the transport (following the dnsproxy convention):
    ///
    ///   --dns 1.1.1.1                DNS-over-HTTPS, expands to
    ///                                https://1.1.1.1/dns-query
    ///   --dns https://h/dns-query    DNS-over-HTTPS at an explicit url
    ///   --dns tls://1.1.1.1          DNS-over-TLS    (needs a tls backend)
    ///   --dns quic://1.1.1.1         DNS-over-QUIC   (needs --client-tls rustls + h3)
    ///   --dns h3://1.1.1.1           DNS-over-HTTPS forced over HTTP/3
    ///
    /// a bare host or one given with tls://, quic:// or h3:// expands to the
    /// transport's default port and path; pass a full url to override either.
    ///
    /// beyond encryption, a non-system resolver also fetches SVCB/HTTPS records
    /// (RFC 9460), so an upstream that advertises alpn=h3 in DNS is reached over
    /// HTTP/3 on the very first request — with no Alt-Svc round-trip — when the
    /// client is http/3-capable (--client-tls rustls with the h3 build).
    ///
    /// resolution is fail-closed: once set, a lookup the resolver can't answer
    /// fails the request rather than falling back to the system resolver, so a
    /// query never leaks to the local resolver.
    ///
    /// every transport runs over tls, so this needs a client tls backend — it
    /// has no effect with --client-tls none.
    #[cfg(any(feature = "rustls", feature = "native-tls", feature = "openssl"))]
    #[arg(long, value_parser = crate::dns::parse_dns, verbatim_doc_comment, help_heading = "DNS")]
    dns: Option<crate::dns::DnsResolver>,

    /// proxy upstream requests over this unix domain socket instead of tcp
    ///
    /// every upstream connection dials the socket at this path; the upstream url
    /// still supplies the request metadata (scheme, path, and `Host`), so pass a
    /// single upstream url for the socket's virtual host, e.g.
    ///
    /// trillium proxy http://app.local --unix-socket /run/app.sock
    ///
    /// combine with --client-tls to speak https over the socket.
    #[cfg(unix)]
    #[cfg_attr(
        any(feature = "rustls", feature = "native-tls", feature = "openssl"),
        arg(
            long,
            value_name = "PATH",
            conflicts_with = "dns",
            verbatim_doc_comment
        )
    )]
    #[cfg_attr(
        not(any(feature = "rustls", feature = "native-tls", feature = "openssl")),
        arg(long, value_name = "PATH", verbatim_doc_comment)
    )]
    unix_socket: Option<PathBuf>,

    /// disable response compression (gzip/brotli/zstd)
    #[arg(long)]
    no_compress: bool,

    /// disable response caching entirely
    #[arg(long, help_heading = "Cache")]
    no_cache: bool,

    /// in-memory (hot) cache tier size, e.g. 256MiB, 1GB; set to 0 to drop the
    /// in-memory tier and cache only to disk (requires --cache-disk)
    #[arg(long, alias = "cache-capacity", value_parser = parse_size, default_value = "256MiB", conflicts_with = "no_cache", help_heading = "Cache")]
    cache_memory_capacity: u64,

    /// directory for an on-disk cache tier; persists cached responses across
    /// restarts. Given alone it tiers a hot in-memory cache over durable disk;
    /// with --cache-memory-capacity 0 it caches only to disk
    #[arg(
        long,
        value_name = "DIR",
        conflicts_with = "no_cache",
        help_heading = "Cache"
    )]
    cache_disk: Option<PathBuf>,

    /// on-disk cache tier size, e.g. 10GiB (only used with --cache-disk)
    #[arg(long, value_parser = parse_size, default_value = "1GiB", conflicts_with = "no_cache", help_heading = "Cache")]
    cache_disk_capacity: u64,

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
    rate_limit: RateLimit,

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

    /// Apply the `--dns` resolver (if any) to the upstream `client`, handing the
    /// selected `--client-tls` to the shared [`crate::dns`] module so it can
    /// validate that the backend can carry the chosen transport.
    #[cfg(any(feature = "rustls", feature = "native-tls", feature = "openssl"))]
    fn apply_dns(&self, client: Client) -> Client {
        match &self.dns {
            Some(dns) => dns.apply(client, self.client_tls, "--client-tls"),
            None => client,
        }
    }

    /// In a build with no tls backend the `--dns` flag doesn't exist, so there
    /// is nothing to apply.
    #[cfg(not(any(feature = "rustls", feature = "native-tls", feature = "openssl")))]
    fn apply_dns(&self, client: Client) -> Client {
        client
    }

    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .init();

        // Resolve the cache flags into a primitive spec, then let the shared
        // `cache` module select the storage backend. `--no-cache` (or a spec
        // with no tiers) leaves the client uncached. `--cache-memory-capacity 0`
        // drops the in-memory tier, so `--cache-disk` alone tiers hot memory
        // over durable disk and pairing it with `0` caches only to disk.
        let cache_spec = (!self.no_cache).then(|| CacheSpec {
            memory: (self.cache_memory_capacity > 0).then_some(self.cache_memory_capacity),
            disk: self
                .cache_disk
                .clone()
                .map(|path| (path, self.cache_disk_capacity)),
            max_body: self.cache_max_body,
            time_to_idle: self.cache_time_to_idle,
            time_to_live: self.cache_time_to_live,
        });

        #[cfg(unix)]
        let unix_socket = self.unix_socket.clone();
        #[cfg(not(unix))]
        let unix_socket: Option<PathBuf> = None;

        let client = crate::tls::build_client(self.client_tls, self.insecure, unix_socket)
            .with_handler(ClientLogger::new().with_formatter(("-> ", client_dev_formatter)));
        let client = match cache_spec {
            Some(spec) => cache::attach(client, spec),
            None => client,
        };
        // `--dns` and `--unix-socket` are mutually exclusive (a fixed socket
        // never resolves a host), so this is a no-op whenever a socket is set.
        let client = self.apply_dns(client);

        let server = (
            Logger::new().with_formatter(("<- ", dev_formatter)),
            self.rate_limit.limiter(),
            // `Option<Handler>` is a `Handler`, so `None` skips compression entirely.
            (!self.no_compress).then(trillium_compression::compression),
            trillium_caching_headers::caching_headers(),
            if self.strategy == UpstreamSelectorStrategy::Forward {
                Some((
                    ForwardProxyConnect::new(crate::tls::client_tcp_config()),
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
            .with_nodelay()
            .with_port(self.port)
            .with_host(&self.host);

        self.server_tls.run_with_tls(config, server);
    }
}
