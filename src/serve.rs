use crate::{
    assets,
    ratelimit::RateLimit,
    server_tls::ServerTls,
    tls::{Tls, parse_url},
};
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use std::{fmt::Debug, io::Write};
use trillium_logger::Logger;
use trillium_proxy::{Client, Proxy, Url};
use trillium_static::StaticFileHandler;

#[cfg(feature = "serve-render")]
mod live;
#[cfg(feature = "serve-render")]
mod render;
mod root_path;
use crate::directory_listing::DirectoryListing;
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

    /// disable response compression (gzip/brotli/zstd)
    #[arg(long)]
    no_compress: bool,

    /// serve an HTML directory listing for directories without an index file
    ///
    /// When enabled, a request that resolves to a directory with no index file
    /// renders a listing of that directory's contents instead of returning 404
    /// Not Found. Off by default, since it exposes file names and structure.
    #[arg(short = 'l', long, env)]
    directory_listing: bool,

    /// Render recognized files in the browser and live-reload on change
    ///
    /// Adds a `?render` query param to any served file: source files are shown
    /// as a syntax-highlighted HTML page, markdown is rendered to HTML, and
    /// `?render=json` returns a JSON envelope for anything else. Directory
    /// listings link to `?render` for recognized types.
    ///
    /// Also injects a small live-reload script into HTML responses and watches
    /// the served directory, refreshing the browser whenever a file changes or
    /// is added (so directory listings stay current too).
    #[cfg(feature = "serve-render")]
    #[arg(short = 'r', long, env)]
    render: bool,

    #[command(flatten)]
    rate_limit: RateLimit,

    #[command(flatten)]
    verbose: Verbosity,
}

impl StaticCli {
    pub fn run(self) {
        env_logger::Builder::new()
            .parse_filters(&format!(
                "{},quinn=off,quinn_proto=off",
                self.verbose.log_level_filter()
            ))
            .format(|buf, record| {
                writeln!(
                    buf,
                    "[{}] {}",
                    record.module_path().unwrap_or_default().dimmed(),
                    record.args()
                )
            })
            .init();

        let path = self.root.clone();
        #[cfg(feature = "serve-render")]
        let root_dir = std::path::PathBuf::from(self.root.clone());
        let mut static_file_handler = StaticFileHandler::new(path);
        if let Some(index) = &self.index {
            static_file_handler = static_file_handler.with_index_file(index);
        }

        // Without the feature there is no `--render` flag, so nothing renders.
        // Hoisting it to a plain `bool` keeps the rest of `run` cfg-free.
        #[cfg(feature = "serve-render")]
        let render = self.render;
        #[cfg(not(feature = "serve-render"))]
        let render = false;

        // The `?render` handler, enabled by `--render`. It transforms the served
        // body in `before_send` (the static handler halts, so a later `run`
        // would never fire) and sits after the file handler so it acts on what
        // that handler produced. `Option<()>` is a no-op `Handler` when the
        // feature is compiled out.
        #[cfg(feature = "serve-render")]
        let render_handler = render.then_some(render::Render);
        #[cfg(not(feature = "serve-render"))]
        let render_handler = (); // `()` is a no-op `Handler`

        // The directory listing links `?render` for recognized files only when
        // `--render` is on.
        #[cfg(feature = "serve-render")]
        let directory_listing = if render {
            DirectoryListing::with_renderable(render::is_renderable)
        } else {
            DirectoryListing::new()
        };
        #[cfg(not(feature = "serve-render"))]
        let directory_listing = DirectoryListing::new();

        // Live reload, enabled by `--render`: injects the reload script into HTML
        // responses and serves the `/_serve_live.*` routes. Placed after
        // compression (so the rewriter runs on the uncompressed body) and ahead
        // of the file handler (so its routes win). `()` is a no-op when disabled.
        #[cfg(feature = "serve-render")]
        let live = render.then(|| live::handler(root_dir));
        #[cfg(not(feature = "serve-render"))]
        let live = ();

        // Embedded stylesheets (`/_css/`) and the fonts they reference
        // (`/_fonts/`). Both the rendered pages and the directory listing link a
        // stylesheet from here, so either flag mounts it. A hit serves-and-halts;
        // a miss falls through to the user's files.
        let assets = (render || self.directory_listing).then(assets::handler);

        let server = (
            Logger::new(),
            self.rate_limit.limiter(),
            // `Option<Handler>` is a `Handler`, so `None` skips compression entirely.
            (!self.no_compress).then(trillium_compression::compression),
            live,
            assets,
            self.forward
                .clone()
                .map(|url| Proxy::new(Client::from(Tls::default()), url)),
            static_file_handler,
            render_handler,
            // Runs only when the file handler resolved a directory it had no
            // index for; otherwise leaves the conn untouched for the 404 path.
            self.directory_listing.then_some(directory_listing),
        );

        let config = trillium_smol::config()
            .with_nodelay()
            .with_port(self.port)
            .with_host(&self.host);

        self.server_tls.run_with_tls(config, server);
    }
}
