//! Turns the decoded [`config`](super::config) into running trillium handlers.
//!
//! Each [`Binding`] becomes one listener. Per-binding cross-cutting handlers
//! (logger, rate limit, compression) wrap a [`trillium_router::Router`] in which
//! every [`Route`] registers its ordered directive stack — a `Vec<BoxedHandler>`,
//! the runtime-assembled equivalent of the `Option`-in-a-tuple idiom used by
//! `serve`/`proxy` — for *all* HTTP methods, so routing is by path regardless of
//! method. Config patterns (`/api/*`, `/*`) are routefinder patterns verbatim,
//! and the matched prefix is stripped for the inner handlers.

use super::{
    config::{
        Binding, CacheNode, Config, Directive, ElementOp, FilesDirective, HeaderOp,
        HeadersDirective, HttpConfigNode, ProxyDirective, RedirectDirective, RewriteHtmlDirective,
        Route, SelectBlock,
    },
    upstream,
};
use crate::{directory_listing::DirectoryListing, tls::Tls};
use std::io;
use trillium::{BoxedHandler, Conn, Handler, HttpConfig, KnownHeaderName, Method, Status};
use trillium_cache::{InMemoryStorage, client::Cache};
use trillium_client::Client;
use trillium_html_rewriter::{
    HtmlRewriter, Settings,
    html::{element, html_content::ContentType},
};
use trillium_logger::Logger;
use trillium_proxy::Proxy;
use trillium_router::Router;
use trillium_server_common::{ServerHandle, Swansong};
use trillium_static::StaticFileHandler;

/// Default cache knobs, matching `trillium proxy`.
const DEFAULT_CACHE_CAPACITY: u64 = 256 * 1024 * 1024;
const DEFAULT_CACHE_MAX_BODY: u64 = 16 * 1024 * 1024;

/// Build the shared proxy client, attaching a response cache if the config
/// opts in. One client (and one cache + connection pool) serves every `proxy`
/// directive across all bindings.
pub fn build_client(config: &Config) -> Client {
    let client = Client::from(Tls::default());
    match &config.cache {
        None => client,
        Some(cache) => client.with_handler(build_cache(cache)),
    }
}

fn build_cache(cache: &CacheNode) -> impl trillium_client::ClientHandler {
    let capacity = cache
        .capacity
        .as_deref()
        .map_or(DEFAULT_CACHE_CAPACITY, parse_size);
    let max_body = cache
        .max_body
        .as_deref()
        .map_or(DEFAULT_CACHE_MAX_BODY, parse_size);

    let mut storage = InMemoryStorage::new().with_max_capacity_bytes(capacity);
    if let Some(tti) = &cache.time_to_idle {
        storage = storage.with_time_to_idle(parse_duration(tti));
    }
    if let Some(ttl) = &cache.time_to_live {
        storage = storage.with_time_to_live(parse_duration(ttl));
    }
    Cache::new(storage)
        .with_max_cacheable_size(max_body)
        .shared()
}

/// Every HTTP method a route stack is registered for, so a route matches on
/// path alone. (`Router::all` covers only GET/POST/PUT/DELETE/PATCH; a gateway
/// must also pass HEAD, OPTIONS, etc.)
const ROUTE_METHODS: &[Method] = &[
    Method::Get,
    Method::Head,
    Method::Post,
    Method::Put,
    Method::Delete,
    Method::Patch,
    Method::Options,
    Method::Connect,
    Method::Trace,
];

/// Print a colored summary of every binding and its routes at startup. The
/// output is part of the product: it shows, at a glance, what each listener
/// serves.
pub fn print_startup(config: &Config) {
    use colored::Colorize;

    for binding in &config.bindings {
        let (host, port) = parse_listen(&binding.listen);
        let scheme = if binding.tls.is_some() {
            "https"
        } else {
            "http"
        };
        println!("{}", format!("{scheme}://{host}:{port}").bold().green());

        for hostblock in &binding.hosts {
            println!("  {}", hostblock.patterns.join(" ").yellow());
            print_routes(&hostblock.routes, 4);
        }
        if !binding.routes.is_empty() {
            if !binding.hosts.is_empty() {
                println!("  {}", "(default)".yellow().dimmed());
            }
            print_routes(
                &binding.routes,
                if binding.hosts.is_empty() { 2 } else { 4 },
            );
        }
    }
}

/// Print one indented `pattern → directives` line per route.
fn print_routes(routes: &[Route], indent: usize) {
    use colored::Colorize;
    let width = routes.iter().map(|r| r.pattern.len()).max().unwrap_or(0);
    for route in routes {
        let directives = route
            .directives
            .iter()
            .map(describe_directive)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:indent$}{:<width$}  {} {directives}",
            "",
            route.pattern.cyan(),
            "→".dimmed(),
        );
    }
}

/// One-line human description of a directive for the startup summary.
fn describe_directive(directive: &Directive) -> String {
    match directive {
        Directive::Files(f) => format!("files {}", f.root.display()),
        Directive::Proxy(p) => format!(
            "proxy {}",
            p.upstreams
                .iter()
                .map(|u| u.url.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Directive::Redirect(r) => format!("redirect {}", r.to),
        Directive::Headers(_) => "headers".to_string(),
        Directive::RewriteHtml(r) => format!("rewrite-html ({} selectors)", r.selects.len()),
    }
}

/// Build one binding's listeners (shared swansong, per-binding `HttpConfig`, TLS)
/// and spawn them, returning its [`ServerHandle`]. Each address is claimed
/// eagerly via the [`ListenerConfig`] builder, so a bind failure (port in use,
/// unresolvable host) surfaces here as an `Err` — fail-fast — instead of as a
/// silently dead listener after the server task spawns.
///
/// [`ListenerConfig`]: trillium_server_common::ListenerConfig
pub fn spawn_binding(
    binding: &Binding,
    config: &Config,
    swansong: &Swansong,
    client: &Client,
) -> io::Result<ServerHandle> {
    let (host, port) = parse_listen(&binding.listen);
    let addr = (host.as_str(), port);

    let mut server = trillium_smol::config()
        .with_swansong(swansong.clone())
        .without_signals();

    if let Some(http) = &binding.http {
        server = server.with_http_config(http_config(http));
        if let Some(max) = http.max_connections {
            server = server.with_max_connections(Some(max));
        }
    }

    let handler = binding_handler(binding, config, client);

    // The global server config (swansong, HTTP config, …) carries over to the
    // multi-listener builder; we add the binding's listener topology to it. TLS
    // (with per-host SNI cert selection) is built from the binding's and its
    // hosts' cert configs; `gateway` currently implies `rustls`, so the `tls{}`
    // block is always actionable.
    let listeners = server.listeners();
    let listeners = match super::sni::build(binding) {
        Some(tls) => {
            let listeners = listeners.bind_tls(addr, tls.acceptor)?;
            // On h3 builds, a QUIC listener shares the binding's port and is
            // advertised to clients via an `alt-svc` header on the TLS listener.
            #[cfg(feature = "h3")]
            let listeners = listeners.bind_quic(addr, tls.quic)?;

            listeners
        }
        None => listeners.bind_tcp(addr)?,
    };

    Ok(listeners.spawn(handler))
}

/// Build a `trillium_http::HttpConfig` from the `http {}` block, applying only
/// the keys present. Size-valued fields accept human units (`"10MiB"`).
fn http_config(node: &HttpConfigNode) -> HttpConfig {
    let mut cfg = HttpConfig::default();
    if let Some(s) = &node.received_body_max_len {
        cfg = cfg.with_received_body_max_len(parse_size(s));
    }
    if let Some(s) = &node.head_max_len {
        cfg = cfg.with_head_max_len(parse_size(s) as usize);
    }
    cfg
}

/// Parse a human-readable byte size like `10MiB` or `1GB` into bytes.
fn parse_size(s: &str) -> u64 {
    let size = size::Size::from_str(s).unwrap_or_else(|e| panic!("invalid size {s:?}: {e}"));
    u64::try_from(size.bytes()).unwrap_or_else(|_| panic!("size {s:?} must not be negative"))
}

/// Parse a human-readable duration like `5m` or `1h`.
fn parse_duration(s: &str) -> std::time::Duration {
    humantime::parse_duration(s).unwrap_or_else(|e| panic!("invalid duration {s:?}: {e}"))
}

/// Build the top-level handler for one binding, applying the config-wide
/// cross-cutting defaults (compression on unless disabled; rate limit if set).
pub fn binding_handler(binding: &Binding, config: &Config, client: &Client) -> impl Handler {
    // With no `host` blocks, the binding is a single router over its routes (v1
    // behavior). Otherwise a host pre-router dispatches by Host header, with the
    // binding's direct routes as the default vhost. `BoxedHandler` unifies the
    // two shapes into one handler type.
    let dispatcher = if binding.hosts.is_empty() {
        BoxedHandler::new(build_router(&binding.routes, client))
    } else {
        let hosts = binding
            .hosts
            .iter()
            .map(|h| (h.patterns.clone(), build_router(&h.routes, client)))
            .collect();
        let default = (!binding.routes.is_empty()).then(|| build_router(&binding.routes, client));
        BoxedHandler::new(super::host::HostRouter::new(hosts, default))
    };

    // `Option<Handler>` is a `Handler`, so `None` drops straight out of the tuple.
    let compression = config
        .compression
        .unwrap_or(true)
        .then(trillium_compression::compression);
    let rate_limit = config.rate_limit.as_ref().map(|rl| {
        crate::ratelimit::limiter_for(&rl.rate, rl.burst)
            .unwrap_or_else(|e| panic!("invalid rate-limit {:?}: {e}", rl.rate))
    });
    // Pairs with the client-side response cache: adds ETag/Cache-Control
    // handling to our responses. Only present when caching is enabled.
    let caching_headers = config
        .cache
        .is_some()
        .then(trillium_caching_headers::caching_headers);

    (
        // Suppress the per-binding "Trillium started …" banner; our own
        // `print_startup` summary covers all bindings once, up front.
        Logger::new().without_init_message(),
        rate_limit,
        caching_headers,
        compression,
        dispatcher,
    )
}

/// Build a router over a set of routes, registering each route's directive
/// stack for all HTTP methods.
fn build_router(routes: &[Route], client: &Client) -> Router {
    let mut router = Router::new();
    for route in routes {
        router = router.any(
            ROUTE_METHODS,
            route.pattern.as_str(),
            route_stack(route, client),
        );
    }
    router
}

/// Assemble one route's ordered directive stack into a single handler.
fn route_stack(route: &Route, client: &Client) -> Vec<BoxedHandler> {
    let mut stack = Vec::new();
    for directive in &route.directives {
        push_directive(&mut stack, directive, client);
    }
    stack
}

fn push_directive(stack: &mut Vec<BoxedHandler>, directive: &Directive, client: &Client) {
    match directive {
        Directive::Files(files) => push_files(stack, files),
        Directive::Proxy(proxy) => push_proxy(stack, proxy, client),
        Directive::Redirect(redirect) => stack.push(BoxedHandler::new(Redirect::new(redirect))),
        Directive::Headers(headers) => stack.push(BoxedHandler::new(Headers::new(headers))),
        Directive::RewriteHtml(rewrite) => push_rewrite_html(stack, rewrite),
    }
}

/// `rewrite-html` → an [`HtmlRewriter`] that replays the configured per-selector
/// element mutations over the response body. Selectors are validated at load
/// time (see [`Config::validate_selectors`](super::config::Config)), so the
/// `element!` macro's internal parse never fails here. The handler self-gates on
/// the response `Content-Type`, so it's safe regardless of what the route serves.
fn push_rewrite_html(stack: &mut Vec<BoxedHandler>, rewrite: &RewriteHtmlDirective) {
    let selects = rewrite.selects.clone();
    // `HtmlRewriter::new` wants a `Fn() -> Settings`: lol-html's handlers are
    // single-use, so fresh ones are built per rewritten response. Borrow (don't
    // consume) `selects` so the closure stays `Fn`.
    let handler = HtmlRewriter::new(move || Settings {
        element_content_handlers: selects
            .iter()
            .cloned()
            .map(|SelectBlock { selector, ops }| {
                element!(selector, move |el| {
                    for op in &ops {
                        match op {
                            ElementOp::SetAttribute(name, value) => {
                                let _ = el.set_attribute(name, value);
                            }
                            ElementOp::RemoveAttribute(name) => el.remove_attribute(name),
                            ElementOp::Before(html) => el.before(html, ContentType::Html),
                            ElementOp::After(html) => el.after(html, ContentType::Html),
                            ElementOp::Prepend(html) => el.prepend(html, ContentType::Html),
                            ElementOp::Append(html) => el.append(html, ContentType::Html),
                            ElementOp::SetInner(html) => {
                                el.set_inner_content(html, ContentType::Html)
                            }
                            ElementOp::SetText(text) => {
                                el.set_inner_content(text, ContentType::Text)
                            }
                            ElementOp::Replace(html) => el.replace(html, ContentType::Html),
                            ElementOp::SetTag(name) => {
                                let _ = el.set_tag_name(name);
                            }
                            ElementOp::Remove => el.remove(),
                            ElementOp::Unwrap => el.remove_and_keep_content(),
                        }
                    }
                    Ok(())
                })
            })
            .collect(),
        ..Settings::new_send()
    });
    stack.push(BoxedHandler::new(handler));
}

/// `files` → a static file handler, optionally followed by a directory listing.
fn push_files(stack: &mut Vec<BoxedHandler>, files: &FilesDirective) {
    let mut handler = StaticFileHandler::new(&files.root);
    if let Some(index) = &files.index {
        handler = handler.with_index_file(index);
    }
    stack.push(BoxedHandler::new(handler));

    if files.directory_listing.unwrap_or(false) {
        // Runs only when the file handler resolved a directory it had no index
        // for; otherwise leaves the conn untouched. Same pattern as `serve`.
        stack.push(BoxedHandler::new(DirectoryListing));
    }
}

/// `proxy` → a reverse proxy over the configured upstream selector. Upstream
/// 404s are forwarded to the client (`proxy_not_found`), since a proxy route is
/// terminal.
fn push_proxy(stack: &mut Vec<BoxedHandler>, proxy: &ProxyDirective, client: &Client) {
    let handler = Proxy::new(client.clone(), upstream::build_selector(proxy))
        .with_via_pseudonym("trillium-gateway")
        .with_websocket_upgrades()
        .proxy_not_found();
    stack.push(BoxedHandler::new(handler));
}

/// Parse a binding's `listen` address into `(host, port)`. An empty host
/// (`":8080"`) binds all interfaces, matching the nginx `listen :80` convention.
pub fn parse_listen(listen: &str) -> (String, u16) {
    let (host, port) = listen
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("listen must be host:port or :port (got {listen:?})"));
    let port = port
        .parse()
        .unwrap_or_else(|_| panic!("invalid port in listen {listen:?}"));
    let host = if host.is_empty() {
        "0.0.0.0".to_string()
    } else {
        host.to_string()
    };
    (host, port)
}

/// `redirect "url" status=NNN` — respond with a `Location` redirect and halt.
#[derive(Debug, Clone)]
struct Redirect {
    to: String,
    status: Status,
}

impl Redirect {
    fn new(redirect: &RedirectDirective) -> Self {
        let status = match redirect.status {
            Some(code) => {
                Status::try_from(code).unwrap_or_else(|_| panic!("invalid redirect status {code}"))
            }
            None => Status::Found,
        };
        Self {
            to: redirect.to.clone(),
            status,
        }
    }
}

impl Handler for Redirect {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_response_header(KnownHeaderName::Location, self.to.clone())
            .with_status(self.status)
            .halt()
    }
}

/// `headers { add/set/remove ... }` — mutate response headers. Applied in
/// `before_send` so it overrides headers set by the terminal handler (and can
/// remove headers added late, like `Server`).
#[derive(Debug, Clone)]
struct Headers {
    ops: Vec<HeaderOp>,
}

impl Headers {
    fn new(headers: &HeadersDirective) -> Self {
        Self {
            ops: headers.ops.clone(),
        }
    }
}

impl Handler for Headers {
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        let headers = conn.response_headers_mut();
        for op in &self.ops {
            match op {
                HeaderOp::Add(name, value) => {
                    headers.append(name.clone(), value.clone());
                }
                HeaderOp::Set(name, value) => {
                    headers.insert(name.clone(), value.clone());
                }
                HeaderOp::Remove(name) => {
                    headers.remove(name.clone());
                }
            }
        }
        conn
    }
}
