//! Shared encrypted-DNS plumbing for the `client`, `proxy`, and `gateway`
//! clients.
//!
//! `client` and `proxy` expose this directly as `--dns` (via the [`parse_dns`]
//! value parser); `gateway` reads the same syntax from a `dns` config node. All
//! three route a [`Client`]'s lookups — including SVCB/HTTPS records — through
//! the resolver by calling [`DnsResolver::apply`].
//!
//! The whole module only exists when a tls backend is available, because every
//! encrypted-DNS transport runs over tls.

use crate::tls::Tls;
use colored::Colorize;
use trillium_client::Client;

/// An encrypted-DNS resolver: the transport plus the string handed to the
/// matching `Client::with_*` builder.
#[derive(Clone, Debug)]
pub struct DnsResolver {
    transport: DnsTransport,
    resolver: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DnsTransport {
    /// DNS-over-HTTPS (`https://`, or a bare host).
    Doh,
    /// DNS-over-HTTPS pinned to HTTP/3 (`h3://`).
    #[cfg(feature = "h3")]
    DohH3,
    /// DNS-over-TLS (`tls://`).
    Dot,
    /// DNS-over-QUIC (`quic://`).
    #[cfg(feature = "h3")]
    Doq,
}

/// Parse a resolver of the form `[scheme://]host[:port][/path]`, where the
/// scheme picks the transport per the dnsproxy convention (`https`/`h3`/`tls`/
/// `quic`); a missing scheme means DoH. `h3`/`quic` exist only in an h3 build.
/// Backend compatibility (a tls provider, or rustls for h3/quic) is checked
/// later in [`DnsResolver::apply`], where the selected `--tls` is known.
pub fn parse_dns(src: &str) -> Result<DnsResolver, String> {
    let (scheme, rest) = match src.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, src),
    };

    if rest.is_empty() {
        return Err("missing resolver host".into());
    }

    let (transport, resolver) = match scheme {
        // DoH accepts a bare host (expands to https://host/dns-query) or a full
        // url, so the original string passes through untouched.
        None | Some("https" | "doh") => (DnsTransport::Doh, src.to_string()),

        // DoH-over-h3 wants the same expansion as DoH: a bare host (no path) is
        // left bare so the builder appends /dns-query; an explicit path becomes
        // an https url used as-is.
        #[cfg(feature = "h3")]
        Some("h3") => {
            let resolver = if rest.contains('/') {
                format!("https://{rest}")
            } else {
                rest.to_string()
            };
            (DnsTransport::DohH3, resolver)
        }

        // DoT/DoQ carry no path; pass a full https url so an explicit :port is
        // honored rather than re-suffixed onto a bare host.
        Some("tls" | "dot") => (DnsTransport::Dot, format!("https://{rest}")),

        #[cfg(feature = "h3")]
        Some("quic" | "doq") => (DnsTransport::Doq, format!("https://{rest}")),

        #[cfg(not(feature = "h3"))]
        Some(s @ ("h3" | "quic" | "doq")) => {
            return Err(format!(
                "the `{s}` scheme needs an http/3 client; rebuild with the h3 feature"
            ));
        }

        Some(other) => {
            return Err(format!(
                "unknown dns scheme `{other}` (use https, h3, tls, or quic)"
            ));
        }
    };

    Ok(DnsResolver {
        transport,
        resolver,
    })
}

impl DnsResolver {
    /// Route `client`'s DNS through this resolver, validating that the `tls`
    /// backend can actually carry the chosen transport. The transport's own URL
    /// parsing already happened in [`parse_dns`]; what's checked here — the
    /// presence of a tls provider, and rustls for the h3 transports — needs the
    /// resolved backend, which the value parser doesn't see. `tls_flag` is the
    /// caller's flag name (`--tls` for `client`, `--client-tls` for `proxy`) so
    /// a misconfiguration points at the right knob.
    pub fn apply(&self, client: Client, tls: Tls, tls_flag: &str) -> Client {
        // Every encrypted-DNS transport runs over tls — DoH/h3 are HTTPS, DoT
        // is tls, DoQ is QUIC (tls 1.3) — so a plaintext connector can carry
        // none of them. Reject up front rather than failing every lookup at
        // request time.
        if matches!(tls, Tls::None) {
            dns_error(&format!(
                "--dns needs a tls backend; pass {tls_flag} rustls (or native/openssl)"
            ));
        }

        match self.transport {
            DnsTransport::Doh => client.with_doh(&self.resolver),

            DnsTransport::Dot => client.with_dot(&self.resolver),

            #[cfg(feature = "h3")]
            DnsTransport::DohH3 => {
                require_h3_client(tls, tls_flag);
                client.with_doh3(&self.resolver)
            }

            #[cfg(feature = "h3")]
            DnsTransport::Doq => {
                require_h3_client(tls, tls_flag);
                client.with_doq(&self.resolver)
            }
        }
    }
}

/// Bail unless the rustls backend was selected. Only rustls builds the quic
/// adapter (see `tls.rs`); on any other backend the client has no HTTP/3
/// endpoint, and `with_doq` would panic outright.
#[cfg(feature = "h3")]
fn require_h3_client(tls: Tls, tls_flag: &str) {
    if tls != Tls::Rustls {
        dns_error(&format!(
            "quic:// and h3:// need an http/3 client; pass {tls_flag} rustls"
        ));
    }
}

/// Print a DNS misconfiguration to stderr and exit. These are user input errors
/// caught before any request is sent, so a terse message and a nonzero exit
/// beat a panic backtrace or a confusing per-request failure.
fn dns_error(msg: &str) -> ! {
    eprintln!("{}: {msg}", "error".bright_red().bold());
    std::process::exit(1);
}
