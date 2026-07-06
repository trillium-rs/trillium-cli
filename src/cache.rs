//! Shared response-cache construction for the `proxy` and `gateway` outbound
//! clients.
//!
//! Both subcommands build the same CDN-style shared cache in front of their
//! upstreams, differing only in where the knobs come from (`proxy` reads clap
//! flags; `gateway` reads a KDL `cache` node). This module owns the part that is
//! identical and easy to get subtly wrong — selecting a storage backend from the
//! declared tiers and attaching it to a [`Client`] — behind a primitive-typed
//! [`CacheSpec`]. Each call site resolves its own configuration into a
//! `CacheSpec` and calls [`attach`].
//!
//! Tier selection mirrors the two knobs a `CacheSpec` carries:
//!
//! - `memory` only → in-memory ([`InMemoryStorage`]).
//! - `disk` only → on-disk ([`FileSystemStorage`]); persists across restarts.
//! - both → a hot in-memory tier over the durable on-disk one ([`TieredStorage`]).
//! - neither → no cache; `attach` returns the client unchanged.

use std::{path::PathBuf, time::Duration};
use trillium_cache::{
    CacheStorage, FileSystemStorage, InMemoryStorage, TieredStorage, client::Cache,
};
use trillium_client::Client;
use trillium_smol::SmolRuntime;

/// A resolved cache configuration in primitive form, produced by each
/// subcommand from its own flags/config and consumed by [`attach`].
#[derive(Debug, Clone, Default)]
pub struct CacheSpec {
    /// In-memory tier capacity in bytes. `None` ⇒ no in-memory tier.
    pub memory: Option<u64>,
    /// On-disk tier: root directory plus its byte cap. `None` ⇒ no on-disk tier.
    pub disk: Option<(PathBuf, u64)>,
    /// Largest cacheable response body in bytes; bigger responses stream uncached.
    pub max_body: u64,
    /// Evict entries not read within this duration.
    pub time_to_idle: Option<Duration>,
    /// Evict entries this long after they are stored.
    pub time_to_live: Option<Duration>,
}

/// Attach a shared response cache to `client`, selecting the storage backend
/// from the tiers `spec` declares. Returns the client unchanged when neither
/// tier is present, so callers can pass a "no cache" spec without a special
/// case.
pub fn attach(client: Client, spec: CacheSpec) -> Client {
    let CacheSpec {
        memory,
        disk,
        max_body,
        time_to_idle: tti,
        time_to_live: ttl,
    } = spec;

    match (memory, disk) {
        // No tiers declared: leave the client uncached.
        (None, None) => client,
        // In-memory only.
        (Some(capacity), None) => {
            client.with_handler(shared_cache(memory_storage(capacity, tti, ttl), max_body))
        }
        // On-disk only: persist everything, no in-memory tier.
        (None, Some((path, capacity))) => client.with_handler(shared_cache(
            disk_storage(path, capacity, tti, ttl),
            max_body,
        )),
        // Both: a hot in-memory tier over the durable on-disk cold tier. The
        // tiered write-back is spawned on the process's smol runtime.
        (Some(mem_capacity), Some((path, disk_capacity))) => {
            let hot = memory_storage(mem_capacity, tti, ttl);
            let cold = disk_storage(path, disk_capacity, tti, ttl);
            let tiered = TieredStorage::new(hot, cold, SmolRuntime::default());
            client.with_handler(shared_cache(tiered, max_body))
        }
    }
}

/// In-memory storage with the shared eviction knobs applied.
fn memory_storage(capacity: u64, tti: Option<Duration>, ttl: Option<Duration>) -> InMemoryStorage {
    let mut storage = InMemoryStorage::new().with_max_capacity_bytes(capacity);
    if let Some(tti) = tti {
        storage = storage.with_time_to_idle(tti);
    }
    if let Some(ttl) = ttl {
        storage = storage.with_time_to_live(ttl);
    }
    storage
}

/// On-disk storage rooted at `path`, with the shared eviction knobs applied.
fn disk_storage(
    path: PathBuf,
    capacity: u64,
    tti: Option<Duration>,
    ttl: Option<Duration>,
) -> FileSystemStorage {
    let mut storage = FileSystemStorage::new(path).with_max_capacity_bytes(capacity);
    if let Some(tti) = tti {
        storage = storage.with_time_to_idle(tti);
    }
    if let Some(ttl) = ttl {
        storage = storage.with_time_to_live(ttl);
    }
    storage
}

/// Wrap any storage backend as a shared (CDN-style) client cache handler.
fn shared_cache<S>(storage: S, max_body: u64) -> Cache<S>
where
    S: CacheStorage + Clone + Send + Sync + 'static,
{
    Cache::new(storage)
        .with_max_cacheable_size(max_body)
        .shared()
}
