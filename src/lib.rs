#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_debug_implementations,
    nonstandard_style,
    missing_copy_implementations,
    unused_qualifications
)]

#[cfg(feature = "bench")]
pub(crate) mod bench;
// Shared response-cache (memory/disk/tiered) construction for the `proxy` and
// `gateway` outbound clients.
#[cfg(any(feature = "proxy", feature = "gateway"))]
pub(crate) mod cache;
#[cfg(feature = "client")]
pub(crate) mod client;
#[cfg(all(unix, feature = "dev-server"))]
pub(crate) mod dev_server;
#[cfg(any(feature = "serve", feature = "gateway"))]
pub(crate) mod directory_listing;
// Encrypted-DNS (`--dns`) plumbing, shared by the `client`/`proxy`/`gateway`
// outbound clients. Every transport runs over tls, so it's only built when a
// tls backend is present.
#[cfg(all(
    any(feature = "client", feature = "proxy", feature = "gateway"),
    any(feature = "rustls", feature = "native-tls", feature = "openssl")
))]
pub(crate) mod dns;
#[cfg(feature = "gateway")]
pub(crate) mod gateway;
#[cfg(feature = "grpc")]
pub(crate) mod grpc;
#[cfg(feature = "proxy")]
pub(crate) mod proxy;
#[cfg(feature = "serve")]
pub(crate) mod serve;
#[cfg(any(
    feature = "proxy",
    feature = "client",
    feature = "serve",
    feature = "bench",
    feature = "gateway"
))]
pub(crate) mod tls;
use clap::Parser;

pub fn main() {
    Cli::parse().run()
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub enum Cli {
    #[cfg(feature = "serve")]
    /// Static file server and reverse proxy
    Serve(serve::StaticCli),

    #[cfg(all(unix, feature = "dev-server"))]
    /// Development server for trillium applications
    DevServer(dev_server::DevServer),

    #[cfg(feature = "client")]
    /// Make http requests using the trillium client
    Client(client::ClientCli),

    #[cfg(feature = "bench")]
    /// Generate http load and report latency/throughput statistics
    Bench(bench::BenchCli),

    #[cfg(feature = "proxy")]
    /// Run a http proxy
    Proxy(proxy::ProxyCli),

    #[cfg(feature = "gateway")]
    /// Run a config-driven server: static files + proxy across one or more listeners
    Gateway(gateway::GatewayCli),

    #[cfg(feature = "grpc")]
    /// Generate Rust modules from .proto service definitions
    Grpc(grpc::GrpcCli),
}

impl Cli {
    pub fn run(self) {
        use Cli::*;
        match self {
            #[cfg(feature = "serve")]
            Serve(s) => s.run(),
            #[cfg(all(unix, feature = "dev-server"))]
            DevServer(d) => d.run(),
            #[cfg(feature = "client")]
            Client(c) => c.run(),
            #[cfg(feature = "bench")]
            Bench(b) => b.run(),
            #[cfg(feature = "proxy")]
            Proxy(p) => p.run(),
            #[cfg(feature = "gateway")]
            Gateway(g) => g.run(),
            #[cfg(feature = "grpc")]
            Grpc(g) => g.run(),
        }
    }
}

#[cfg(any(feature = "proxy", feature = "serve", feature = "gateway"))]
mod ratelimit;
// `gateway` has its own TLS path (`gateway::sni`) and never touches `ServerTls`,
// so this module is only built for `serve`/`proxy`.
#[cfg(any(feature = "proxy", feature = "serve"))]
mod server_tls;
