//! The keys and the most-recently-used list of a client's connection pools
//! (see the parent module's notes on pooling).
use super::{Error, Request, State, check_pointer, milliseconds};
use crt_sys::{self as ffi, native as c};
use std::num::NonZeroU32;
use std::ptr::NonNull;

/// How many pools a client keeps, urllib3's `PoolManager` default
/// (`num_pools`): a request to an eleventh server closes the least recently
/// used pool's idle connections. botocore never sets it.
pub const POOLS: usize = 10;

/// How many connections a pool keeps unless the caller says otherwise:
/// urllib3's pool `maxsize` as botocore sets it, `max_pool_connections`,
/// whose default is `MAX_POOL_CONNECTIONS` in botocore/endpoint.py. No CLI
/// option, variable or profile setting reaches the client's `Config`, so
/// the Python route always runs with this.
#[allow(
    clippy::expect_used,
    reason = "evaluated at compile time, so a zero fails the build"
)]
pub const POOL_CONNECTIONS: NonZeroU32 = NonZeroU32::new(10).expect("a pool keeps a connection");

/// What makes two requests' connections interchangeable: urllib3 keys its
/// pools by scheme, host and port (`PoolManager._new_pool`), a forwarding
/// proxy's requests by the proxy's, and a tunnel's by the server it reaches
/// through that proxy (`ProxyManager.connection_from_host`); the rest is
/// how the connection is made, which one client keeps the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PoolKey {
    host: String,
    port: u16,
    tls: bool,
    tunnel: Option<(String, u16)>,
    keep_alive: bool,
    connect_timeout_ms: u32,
    max_connections: u32,
}

impl PoolKey {
    pub(super) fn of(request: &Request<'_>, max_connections: NonZeroU32) -> Self {
        Self {
            host: request.host.to_owned(),
            port: request.port,
            tls: request.tls,
            tunnel: request
                .tunnel
                .map(|tunnel| (tunnel.host.to_owned(), tunnel.port)),
            keep_alive: request.keep_alive,
            connect_timeout_ms: milliseconds(request.connect_timeout),
            max_connections: max_connections.get(),
        }
    }
}

impl State {
    /// The pool for `key`, made on `runtime` if there is none, and now the
    /// most recently used.
    pub(super) fn pool(
        &mut self,
        runtime: NonNull<c::Runtime>,
        key: PoolKey,
    ) -> Result<NonNull<c::HttpPool>, Error> {
        if let Some(index) = self.pools.iter().position(|(held, _)| *held == key) {
            let entry = self.pools.remove(index);
            let pool = entry.1;
            self.pools.push(entry);
            return Ok(pool);
        }
        let cursor = |text: &str| ffi::AwsByteCursor::from_bytes(text.as_bytes());
        let (tunnel_host, tunnel_port) = key
            .tunnel
            .as_ref()
            .map_or(("", 0), |(host, port)| (host.as_str(), *port));
        let options = c::HttpPoolOptions {
            host: cursor(&key.host),
            tunnel_host: cursor(tunnel_host),
            port: u32::from(key.port),
            tunnel_port: u32::from(tunnel_port),
            connect_timeout_ms: key.connect_timeout_ms,
            max_connections: key.max_connections,
            tls: key.tls,
            keep_alive: key.keep_alive,
        };
        // SAFETY: the caller holds the lock that keeps the runtime's stages
        // in place, and the shim copies the options before it returns.
        let pool = check_pointer(unsafe { c::zaws_http_pool_new(runtime.as_ptr(), &options) })
            .map_err(Error::Connect)?;
        if self.pools.len() == POOLS {
            let (_, oldest) = self.pools.remove(0);
            // SAFETY: the client's reference, which nothing reaches now; a
            // call in flight holds the pool itself.
            unsafe { c::zaws_http_pool_release(oldest.as_ptr()) };
        }
        self.pools.push((key, pool));
        Ok(pool)
    }
}

#[cfg(test)]
mod tests;
