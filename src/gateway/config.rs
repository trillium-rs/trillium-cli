//! The KDL config model for `trillium gateway`.
//!
//! These types are decoded directly from the config file via [`knus`] (KDL v1
//! syntax: bare `true`/`false`/`null`, kebab-case property names). They are a
//! faithful, lightly-validated representation of the document — turning them
//! into running handlers happens in [`super::build`]. Keeping decode and build
//! separate means parse errors are reported against the source (with `miette`
//! spans) before any listener is touched.
//!
//! Document shape:
//!
//! ```kdl
//! compression true                  // optional cross-cutting defaults
//! rate-limit "100/min" burst=200
//!
//! binding ":8080" {
//!     tls cert="./cert.pem" key="./key.pem"
//!     http { received-body-max-len "10MiB" }
//!     route "/api/*" {
//!         proxy strip-prefix="/api" strategy="round-robin" {
//!             upstream "http://127.0.0.1:9000"
//!         }
//!     }
//!     route "/*" { files root="/srv/www" index="index.html" }
//! }
//! ```

use std::path::PathBuf;

/// The whole config document. Only `child`/`children` fields, so it decodes as
/// a `knus` root document.
#[derive(knus::Decode, Debug)]
pub struct Config {
    /// Default compression setting inherited by every binding/route. `None`
    /// leaves the per-route default (on) in place; `Some(false)` turns it off
    /// globally.
    #[knus(child, unwrap(argument))]
    pub compression: Option<bool>,

    /// Default rate limit inherited by every binding.
    #[knus(child)]
    pub rate_limit: Option<RateLimitNode>,

    /// Response caching for `proxy` directives. Opt-in: absent → no caching
    /// (unlike `trillium proxy`, a gateway shouldn't silently cache dynamic
    /// upstreams). A bare `cache` node enables it with defaults.
    #[knus(child)]
    pub cache: Option<CacheNode>,

    /// One or more listeners.
    #[knus(children(name = "binding"))]
    pub bindings: Vec<Binding>,
}

/// `cache { capacity "256MiB"; max-body "16MiB"; time-to-idle "5m"; time-to-live "1h" }`.
/// All fields optional; size/duration strings are parsed in the build step.
#[derive(knus::Decode, Debug, Default)]
pub struct CacheNode {
    /// Maximum total in-memory cache size (default 256MiB).
    #[knus(child, unwrap(argument))]
    pub capacity: Option<String>,
    /// Largest cacheable response body; bigger responses stream uncached
    /// (default 16MiB).
    #[knus(child, unwrap(argument))]
    pub max_body: Option<String>,
    /// Evict entries not read within this duration, e.g. `5m`.
    #[knus(child, unwrap(argument))]
    pub time_to_idle: Option<String>,
    /// Evict entries this long after they are stored, e.g. `1h`.
    #[knus(child, unwrap(argument))]
    pub time_to_live: Option<String>,
}

/// `rate-limit "100/min" burst=200` — parsed into a real quota in the build step
/// via the shared [`crate::ratelimit`] parser.
#[derive(knus::Decode, Debug, Clone)]
pub struct RateLimitNode {
    /// `COUNT/WINDOW`, e.g. `100/min`, `10/s`, `1000/h`.
    #[knus(argument)]
    pub rate: String,
    /// Burst allowance above the sustained rate; defaults to the rate count.
    #[knus(property)]
    pub burst: Option<u64>,
}

/// A single listener: a socket address plus everything served on it.
#[derive(knus::Decode, Debug)]
pub struct Binding {
    /// Listen address: `":8080"`, `"0.0.0.0:8080"`, or `"localhost:8080"`.
    #[knus(argument)]
    pub listen: String,

    /// TLS for this binding. Absent → plaintext.
    #[knus(child)]
    pub tls: Option<TlsNode>,

    /// Per-binding `trillium_http::HttpConfig` overrides. Absent → defaults.
    #[knus(child)]
    pub http: Option<HttpConfigNode>,

    /// Host-header virtual hosts on this (shared) socket. Each matches one or
    /// more Host patterns and has its own routes. A request whose Host matches
    /// no `host` block falls back to the binding's direct `routes` (the default
    /// vhost), which also covers requests with no Host header (HTTP/1.0).
    #[knus(children(name = "host"))]
    pub hosts: Vec<HostBlock>,

    /// Ordered path routes applied when no `host` block matches (and the only
    /// routes when there are no `host` blocks). First match wins.
    #[knus(children(name = "route"))]
    pub routes: Vec<Route>,
}

/// `host "example.com" "*.api.example.com" { route ... }` — a virtual host.
#[derive(knus::Decode, Debug)]
pub struct HostBlock {
    /// One or more Host patterns: exact (`example.com`), wildcard
    /// (`*.example.com`, matches any subdomain), or `*` (any host).
    #[knus(arguments)]
    pub patterns: Vec<String>,

    /// Ordered path routes for this virtual host.
    #[knus(children(name = "route"))]
    pub routes: Vec<Route>,
}

/// `tls cert="./cert.pem" key="./key.pem"`.
#[derive(knus::Decode, Debug)]
pub struct TlsNode {
    #[knus(property)]
    pub cert: PathBuf,
    #[knus(property)]
    pub key: PathBuf,
}

/// Per-binding subset of [`trillium_http::HttpConfig`]. All optional; only the
/// keys present are applied over the defaults. Size-valued fields are strings
/// (`"10MiB"`) parsed in the build step. Expanded toward full `HttpConfig`
/// coverage in a later increment.
#[derive(knus::Decode, Debug, Default)]
pub struct HttpConfigNode {
    #[knus(child, unwrap(argument))]
    pub received_body_max_len: Option<String>,
    #[knus(child, unwrap(argument))]
    pub head_max_len: Option<String>,
    #[knus(child, unwrap(argument))]
    pub max_connections: Option<usize>,
}

/// `route "/pattern" { <directives> }`. The pattern is a path prefix/glob; the
/// directives are an ordered, heterogeneous stack compiled into one handler.
#[derive(knus::Decode, Debug)]
pub struct Route {
    #[knus(argument)]
    pub pattern: String,

    #[knus(children)]
    pub directives: Vec<Directive>,
}

/// One directive within a route. The enum variant name is the KDL node name
/// (`files`, `proxy`, `redirect`, `headers`); document order is preserved, which
/// is how the directive stack stays ordered.
#[derive(knus::Decode, Debug)]
pub enum Directive {
    Files(FilesDirective),
    Proxy(ProxyDirective),
    Redirect(RedirectDirective),
    Headers(HeadersDirective),
}

/// `files root="/srv/www" index="index.html" directory-listing=true`.
#[derive(knus::Decode, Debug)]
pub struct FilesDirective {
    #[knus(property)]
    pub root: PathBuf,
    #[knus(property)]
    pub index: Option<String>,
    #[knus(property)]
    pub directory_listing: Option<bool>,
}

/// `proxy strategy="round-robin" { upstream "..." }`.
///
/// The forwarded path is the router-stripped `conn.path()` (so a `/api/*` route
/// strips `/api`, consistently with `files`), concatenated onto each upstream
/// URL's own base path. To forward *with* the route prefix intact, give the
/// upstream a base path (`upstream "http://backend/api"`).
#[derive(knus::Decode, Debug)]
pub struct ProxyDirective {
    /// Upstream selection strategy; parsed/validated in the build step.
    /// Defaults to round-robin.
    #[knus(property)]
    pub strategy: Option<String>,
    /// One or more upstream targets.
    #[knus(children(name = "upstream"))]
    pub upstreams: Vec<UpstreamNode>,
}

/// `upstream "http://127.0.0.1:9000"`.
#[derive(knus::Decode, Debug)]
pub struct UpstreamNode {
    #[knus(argument)]
    pub url: String,
}

/// `redirect "https://example.com/new" status=308`.
#[derive(knus::Decode, Debug)]
pub struct RedirectDirective {
    #[knus(argument)]
    pub to: String,
    /// HTTP redirect status; defaults to 302 Found.
    #[knus(property)]
    pub status: Option<u16>,
}

/// `headers { add "X-Served-By" "trillium"; remove "Server" }`.
#[derive(knus::Decode, Debug)]
pub struct HeadersDirective {
    #[knus(children)]
    pub ops: Vec<HeaderOp>,
}

/// A single response-header mutation.
#[derive(knus::Decode, Debug, Clone)]
pub enum HeaderOp {
    /// `add "Name" "value"` — append, keeping any existing values.
    Add(#[knus(argument)] String, #[knus(argument)] String),
    /// `set "Name" "value"` — replace any existing values.
    Set(#[knus(argument)] String, #[knus(argument)] String),
    /// `remove "Name"`.
    Remove(#[knus(argument)] String),
}

impl Config {
    /// Parse a KDL config file, reporting errors with `miette` source spans.
    pub fn load(path: &std::path::Path) -> miette::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| miette::miette!("could not read {}: {e}", path.display()))?;
        let filename = path.display().to_string();
        Ok(knus::parse(&filename, &text)?)
    }
}
