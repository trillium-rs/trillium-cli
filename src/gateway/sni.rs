//! Per-host TLS via SNI for gateway bindings.
//!
//! rustls already serves multiple certificates from one socket: it consults a
//! [`ResolvesServerCert`] with each TLS `ClientHello`, which carries the SNI
//! hostname. So a "vhost-aware acceptor" needs no new trillium machinery — we
//! build one resolver from the per-`host` certs (reusing the same
//! [`HostMatcher`] as request routing, so cert selection and routing agree on
//! what a pattern means) and hand it to both the rustls acceptor
//! ([`From<ServerConfig>`]) and the QUIC config
//! ([`QuicConfig::from_cert_resolver`]).
//!
//! Assumes the aws-lc-rs crypto provider, which is `trillium-rustls`'s default.

use super::{
    config::{Binding, TlsNode},
    host::HostMatcher,
};
use std::{io::Cursor, sync::Arc};
use trillium_rustls::{
    RustlsAcceptor,
    rustls::{
        ServerConfig,
        crypto::aws_lc_rs,
        server::{ClientHello, ResolvesServerCert},
        sign::CertifiedKey,
    },
};

/// Picks a certificate by SNI, falling back to an optional default cert (for
/// unmatched SNI and for clients that send none).
#[derive(Debug)]
struct SniResolver {
    certs: Vec<(HostMatcher, Arc<CertifiedKey>)>,
    default: Option<Arc<CertifiedKey>>,
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = hello.server_name();
        self.certs
            .iter()
            .find(|(matcher, _)| matcher.matches(sni))
            .map(|(_, ck)| Arc::clone(ck))
            .or_else(|| self.default.clone())
    }
}

/// Parse a PEM cert chain + private key into a rustls [`CertifiedKey`].
fn load_certified_key(tls: &TlsNode) -> Arc<CertifiedKey> {
    let cert_pem = std::fs::read(&tls.cert)
        .unwrap_or_else(|e| panic!("could not read cert {}: {e}", tls.cert.display()));
    let key_pem = std::fs::read(&tls.key)
        .unwrap_or_else(|e| panic!("could not read key {}: {e}", tls.key.display()));

    let cert_chain = rustls_pemfile::certs(&mut Cursor::new(&cert_pem))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("invalid certificate {}: {e}", tls.cert.display()));
    let key_der = rustls_pemfile::private_key(&mut Cursor::new(&key_pem))
        .unwrap_or_else(|e| panic!("invalid key {}: {e}", tls.key.display()))
        .unwrap_or_else(|| panic!("no private key found in {}", tls.key.display()));

    let signing_key = aws_lc_rs::default_provider()
        .key_provider
        .load_private_key(key_der)
        .unwrap_or_else(|e| panic!("unusable private key {}: {e}", tls.key.display()));

    Arc::new(CertifiedKey::new(cert_chain, signing_key))
}

/// TLS for one binding, ready to apply to its server `Config`.
pub struct TlsBundle {
    pub acceptor: RustlsAcceptor,
    #[cfg(feature = "h3")]
    pub quic: trillium_quinn::QuicConfig,
}

/// Build TLS for a binding from its per-host and binding-level certs, or `None`
/// if no certificate is configured anywhere on it (plaintext binding).
pub fn build(binding: &Binding) -> Option<TlsBundle> {
    let mut certs = Vec::new();
    for host in &binding.hosts {
        if let Some(tls) = &host.tls {
            let certified = load_certified_key(tls);
            for pattern in &host.patterns {
                certs.push((HostMatcher::parse(pattern), Arc::clone(&certified)));
            }
        }
    }
    let default = binding.tls.as_ref().map(load_certified_key);

    if certs.is_empty() && default.is_none() {
        return None;
    }

    let resolver: Arc<dyn ResolvesServerCert> = Arc::new(SniResolver { certs, default });

    // Mirror `RustlsAcceptor::from_single_cert`'s setup, swapping the single
    // cert for the SNI resolver.
    let mut config = ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("crypto provider supports safe default protocol versions")
        .with_no_client_auth()
        .with_cert_resolver(Arc::clone(&resolver));
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Some(TlsBundle {
        acceptor: RustlsAcceptor::from(config),
        #[cfg(feature = "h3")]
        quic: trillium_quinn::QuicConfig::from_cert_resolver(resolver),
    })
}
