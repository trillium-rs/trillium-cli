//! `trillium gateway` — a config-driven, multi-binding server.
//!
//! Reads a KDL config file and assembles trillium's static-file, proxy,
//! compression, and rate-limit handlers into one or more listeners. Unlike a
//! normal trillium app, the handler graph is built at runtime from the config
//! rather than composed at compile time.

mod build;
mod config;
mod host;
mod sni;
mod upstream;
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use config::Config;
use std::{fmt::Debug, path::PathBuf};
use trillium_server_common::Swansong;

#[derive(Parser, Debug)]
pub struct GatewayCli {
    /// Path to the KDL config file
    #[arg(
        short,
        long,
        env = "TRILLIUM_GATEWAY_CONFIG",
        default_value = "gateway.kdl"
    )]
    config: PathBuf,

    /// Parse and print the resolved config, then exit without serving
    #[arg(long)]
    check: bool,

    #[command(flatten)]
    verbose: Verbosity,
}

impl GatewayCli {
    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .init();

        let config = match Config::load(&self.config) {
            Ok(config) => config,
            Err(report) => {
                eprintln!("{report:?}");
                std::process::exit(1);
            }
        };

        if self.check {
            println!("{config:#?}");
            return;
        }

        if config.bindings.is_empty() {
            eprintln!("no `binding` declared in {}", self.config.display());
            std::process::exit(1);
        }

        // One Swansong shared by every binding: a single shutdown signal drains
        // them all together. Each server's own signal handling is disabled
        // (`without_signals`) so we register once, on the main thread, below.
        //
        // One client (cache + connection pool) shared by every proxy directive.
        let client = build::build_client(&config);
        let swansong = Swansong::new();
        let mut handles = Vec::with_capacity(config.bindings.len());
        for binding in &config.bindings {
            match build::spawn_binding(binding, &config, &swansong, &client) {
                Ok(handle) => handles.push(handle),
                // A bind failed (port in use, unresolvable host). Drain the
                // bindings that did come up, then exit so the operator sees the
                // problem immediately rather than a partially-serving gateway.
                Err(error) => {
                    eprintln!("failed to bind {}: {error}", binding.listen);
                    swansong.shut_down().block();
                    std::process::exit(1);
                }
            }
        }

        // Announce only once every listener is actually bound, so the green
        // banner never advertises a binding that failed to come up.
        build::print_startup(&config);

        wait_for_shutdown_signal();
        log::info!("shutting down {} binding(s)", handles.len());
        swansong.shut_down();
        swansong.block_on_shutdown_completion();
    }
}

/// Block the main thread until a shutdown signal arrives.
#[cfg(unix)]
fn wait_for_shutdown_signal() {
    use signal_hook::{
        consts::signal::{SIGINT, SIGQUIT, SIGTERM},
        iterator::Signals,
    };
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGQUIT]).expect("registering signals");
    signals.forever().next();
}

/// Non-unix fallback: park until the process is terminated. Graceful
/// signal-driven shutdown on Windows is a follow-up.
#[cfg(not(unix))]
fn wait_for_shutdown_signal() {
    loop {
        std::thread::park();
    }
}
