#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_debug_implementations,
    nonstandard_style,
    missing_copy_implementations,
    unused_qualifications
)]

#[cfg(feature = "client")]
pub(crate) mod client;
#[cfg(any(feature = "proxy", feature = "client", feature = "serve"))]
pub(crate) mod client_tls;
#[cfg(all(unix, feature = "dev-server"))]
pub(crate) mod dev_server;
#[cfg(feature = "proxy")]
pub(crate) mod proxy;
#[cfg(feature = "serve")]
pub(crate) mod serve;
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

    #[cfg(feature = "proxy")]
    /// Run a http proxy
    Proxy(proxy::ProxyCli),
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
            #[cfg(feature = "proxy")]
            Proxy(p) => p.run(),
        }
    }
}

#[cfg(any(feature = "proxy", feature = "serve"))]
mod server_tls;
