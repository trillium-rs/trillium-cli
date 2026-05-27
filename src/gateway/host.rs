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

/// Matches a request Host against one configured pattern.
#[derive(Debug)]
enum HostMatcher {
    /// `*` — any host (including a missing Host header).
    Any,
    /// `example.com` — exact match (case-insensitive, port-insensitive).
    Exact(String),
    /// `*.example.com` — stored as `.example.com`; matches any subdomain.
    Suffix(String),
}

impl HostMatcher {
    fn parse(pattern: &str) -> Self {
        if pattern == "*" {
            Self::Any
        } else if let Some(rest) = pattern.strip_prefix("*.") {
            Self::Suffix(format!(".{}", rest.to_ascii_lowercase()))
        } else {
            Self::Exact(pattern.to_ascii_lowercase())
        }
    }

    fn matches(&self, host: Option<&str>) -> bool {
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

impl Handler for HostRouter {
    async fn run(&self, conn: Conn) -> Conn {
        let host = normalize(conn.host());
        match self.select(host.as_deref()) {
            Some(router) => router.run(conn).await,
            None => conn,
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        let host = normalize(conn.host());
        match self.select(host.as_deref()) {
            Some(router) => router.before_send(conn).await,
            None => conn,
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        let host = normalize(upgrade.authority());
        self.select(host.as_deref())
            .is_some_and(|router| router.has_upgrade(upgrade))
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        let host = normalize(upgrade.authority());
        if let Some(router) = self.select(host.as_deref()) {
            router.upgrade(upgrade).await;
        }
    }
}
