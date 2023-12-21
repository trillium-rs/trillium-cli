#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_debug_implementations,
    nonstandard_style,
    missing_copy_implementations,
    unused_qualifications
)]

mod cli_options;
mod client;
#[cfg(unix)]
mod dev_server;
mod proxy;
mod root_path;
mod static_cli_options;

use clap::Parser;
pub(crate) use cli_options::*;
pub(crate) use client::ClientCli;
#[cfg(unix)]
pub(crate) use dev_server::DevServer;
pub(crate) use proxy::*;
pub(crate) use root_path::*;
pub(crate) use static_cli_options::*;

pub fn main() {
    Cli::parse().run()
}
