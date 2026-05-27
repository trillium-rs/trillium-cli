//! The config-driven [`UpstreamSelector`] for the `proxy` directive.
//!
//! Each configured `upstream` becomes a [`Base`], which builds the forwarded
//! URL explicitly from the **router-stripped** [`conn.path()`](trillium::Conn::path)
//! plus querystring — *not* [`path_and_query()`](trillium::Conn::path_and_query),
//! which the stock `impl UpstreamSelector for Url` uses and which ignores router
//! nesting. Building the path ourselves keeps proxying consistent with how
//! `files` sees the stripped path, and sidesteps `Url::join`'s relative-
//! resolution surprises. Selection across multiple upstreams reuses trillium's
//! own [`RoundRobin`]/[`RandomSelector`]/[`ConnectionCounting`], parameterized
//! with `Base`.

use super::config::ProxyDirective;
use trillium::Conn;
use trillium_proxy::{
    Url,
    upstream::{ConnectionCounting, RandomSelector, RoundRobin, UpstreamSelector},
};

/// A single upstream target plus the path-construction logic.
#[derive(Debug, Clone)]
struct Base(Url);

impl UpstreamSelector for Base {
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        let mut url = self.0.clone();
        // Concatenate the router-stripped request path onto the upstream's own
        // base path, so `http://backend/api` forwards to `/api/<rest>` while a
        // bare `http://backend` forwards to `/<rest>`. `conn.path()` is the
        // router's wildcard capture, which has no leading slash for wildcard
        // routes but is the full (slash-led) path for exact routes — so
        // normalize to exactly one separating slash.
        let base_path = url.path().trim_end_matches('/');
        let rest = conn.path().trim_start_matches('/');
        url.set_path(&format!("{base_path}/{rest}"));
        let query = conn.querystring();
        url.set_query((!query.is_empty()).then_some(query));
        Some(url)
    }
}

/// Build the upstream selector for one `proxy` directive.
pub fn build_selector(proxy: &ProxyDirective) -> Box<dyn UpstreamSelector> {
    let bases: Vec<Base> = proxy
        .upstreams
        .iter()
        .map(|u| {
            Base(
                u.url
                    .parse()
                    .unwrap_or_else(|e| panic!("invalid upstream url {:?}: {e}", u.url)),
            )
        })
        .collect();

    assert!(
        !bases.is_empty(),
        "proxy directive requires at least one `upstream`"
    );

    match proxy.strategy.as_deref().unwrap_or("round-robin") {
        "round-robin" => RoundRobin::new(bases).boxed(),
        "random" => RandomSelector::new(bases).boxed(),
        "connection-counting" | "least-conn" => ConnectionCounting::new(bases).boxed(),
        other => panic!(
            "unknown proxy strategy {other:?}; use round-robin, random, or connection-counting"
        ),
    }
}
