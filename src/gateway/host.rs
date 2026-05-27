//! Host-header virtual hosting — the gateway pre-router.
//!
//! A [`HostRouter`] sits in front of the per-host [`Router`]s on a single
//! binding and dispatches each request to the one whose Host pattern matches,
//! falling back to an optional default (the binding's direct routes, which also
//! catches requests with no Host header, e.g. HTTP/1.0).
//!
//! Like [`trillium_router::Router`], the handler lifecycle methods
//! (`before_send`/`has_upgrade`/`upgrade`) are **stateless**: they re-resolve
//! the matching host each time rather than stashing the selection in conn
//! state. This is what makes directives such as `headers` (which act in
//! `before_send`) keep working behind the pre-router.

use trillium::{Conn, Handler, Upgrade};
use trillium_router::Router;

/// Matches a request Host (or TLS SNI) against one configured pattern. Shared
/// by request routing and per-host TLS cert selection so both agree on what a
/// pattern means.
#[derive(Debug)]
pub(crate) enum HostMatcher {
    /// `*` — any host (including a missing Host header).
    Any,
    /// `example.com` — exact match (case-insensitive, port-insensitive).
    Exact(String),
    /// `*.example.com` — stored as `.example.com`; matches any subdomain.
    Suffix(String),
}

impl HostMatcher {
    pub(crate) fn parse(pattern: &str) -> Self {
        if pattern == "*" {
            Self::Any
        } else if let Some(rest) = pattern.strip_prefix("*.") {
            Self::Suffix(format!(".{}", rest.to_ascii_lowercase()))
        } else {
            Self::Exact(pattern.to_ascii_lowercase())
        }
    }

    pub(crate) fn matches(&self, host: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(expected) => host == Some(expected.as_str()),
            Self::Suffix(suffix) => host.is_some_and(|h| h.ends_with(suffix.as_str())),
        }
    }
}

/// One virtual host: its patterns and the router for its routes.
#[derive(Debug)]
struct HostScope {
    matchers: Vec<HostMatcher>,
    router: Router,
}

impl HostScope {
    fn matches(&self, host: Option<&str>) -> bool {
        self.matchers.iter().any(|m| m.matches(host))
    }
}

/// Dispatches by Host header to a per-host [`Router`].
#[derive(Debug)]
pub struct HostRouter {
    hosts: Vec<HostScope>,
    default: Option<Router>,
}

impl HostRouter {
    /// Build from `(patterns, router)` pairs and an optional default router.
    pub fn new(hosts: Vec<(Vec<String>, Router)>, default: Option<Router>) -> Self {
        let hosts = hosts
            .into_iter()
            .map(|(patterns, router)| HostScope {
                matchers: patterns.iter().map(|p| HostMatcher::parse(p)).collect(),
                router,
            })
            .collect();
        Self { hosts, default }
    }

    fn select(&self, host: Option<&str>) -> Option<&Router> {
        self.hosts
            .iter()
            .find(|scope| scope.matches(host))
            .map(|scope| &scope.router)
            .or(self.default.as_ref())
    }
}

/// Normalize a Host/authority to its lowercased hostname, dropping any `:port`.
fn normalize(host: Option<&str>) -> Option<String> {
    host.map(|h| {
        h.rsplit_once(':')
            .map_or(h, |(name, _port)| name)
            .to_ascii_lowercase()
    })
}

fn host(conn: &Conn) -> Option<String> {
    let conn: &trillium_http::Conn<_> = conn.as_ref();
    let host = conn.host().or(conn.authority());
    normalize(host)
}

struct NormalizedHost(Option<String>);

impl Handler for HostRouter {
    async fn run(&self, conn: Conn) -> Conn {
        let host = host(&conn);
        match self.select(host.as_deref()) {
            Some(router) => router.run(conn.with_state(NormalizedHost(host))).await,
            None => conn,
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        if let Some(route) = conn
            .state()
            .and_then(|NormalizedHost(host)| self.select(host.as_deref()))
        {
            route.before_send(conn).await
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        if let Some(route) = upgrade
            .state()
            .get()
            .and_then(|NormalizedHost(host)| self.select(host.as_deref()))
        {
            route.has_upgrade(upgrade)
        } else {
            false
        }
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        if let Some(route) = upgrade
            .state()
            .get()
            .and_then(|NormalizedHost(host)| self.select(host.as_deref()))
        {
            route.upgrade(upgrade).await;
        }
    }
}
