//! `Host*` traits — the decoupling seam between engine-agnostic logic and the
//! engine binding. The binding (`bridge-napi`) implements these over
//! `ThreadsafeFunction`s; a future `bridge-wasm` would implement them over
//! `js_sys::Function`. All impls MUST be genuinely `Send + Sync + 'static`
//! (napi runs on a real tokio threadpool — no faking).
//!
//! The engine-agnostic wacore adapters (impl `Transport`/`HttpClient`/`Backend`/
//! `EventHandler`) live in `bridge-core` and delegate to these traits, so the
//! bounded channel, overflow policy, write-back cache, self-index, expiry and
//! camelCase serialization never touch engine types.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use wacore::net::{HttpRequest, HttpResponse};

use crate::value::BridgeValue;

// ── Transport ───────────────────────────────────────────────────────────────

/// Sink the engine hands to the host so the host can push WebSocket events back
/// into the engine. Implemented by a core-owned type wrapping the
/// `async_channel::Sender<TransportEvent>`; the host (e.g. the napi
/// `JsTransportHandle` class) calls these from the JS thread.
///
/// Methods are synchronous and MUST be non-blocking (a `try_send` into the
/// channel) — never block the JS event loop, never call back into the engine.
pub trait HostTransportSink: Send + Sync + 'static {
    fn on_connected(&self);
    fn on_data(&self, data: Bytes);
    /// Maps to `DisconnectReason::Unknown` today (the JS side carries no code).
    fn on_disconnected(&self);
}

/// A live host-provided transport (one per connection attempt).
#[async_trait]
pub trait HostTransport: Send + Sync + 'static {
    async fn send(&self, data: Bytes) -> anyhow::Result<()>;
    async fn disconnect(&self);
}

/// Host-provided transport factory. `connect` MUST resolve only once the socket
/// is OPEN — the core's `TRANSPORT_CONNECT_TIMEOUT` depends on it.
#[async_trait]
pub trait HostTransportFactory: Send + Sync + 'static {
    async fn connect(
        &self,
        sink: Arc<dyn HostTransportSink>,
    ) -> anyhow::Result<Arc<dyn HostTransport>>;
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

/// Host-provided HTTP executor. Streaming is intentionally absent (the JS
/// bridge buffers, matching `supports_streaming() == false` today).
#[async_trait]
pub trait HostHttp: Send + Sync + 'static {
    async fn execute(&self, req: HttpRequest) -> anyhow::Result<HttpResponse>;
}

// ── Store ────────────────────────────────────────────────────────────────────

/// Static store capabilities, read once at construction. Each is only honored
/// if the corresponding optional method is actually present on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostStoreCapabilities {
    pub batch: bool,
    pub enumerate: bool,
    pub prefix_delete: bool,
    pub write_back: bool,
}

/// Key-value store seam — exactly the JS callback surface. ALL higher-level
/// logic (the 6 wacore store sub-traits, write-back cache, self-index, expiry,
/// composite key formats, `meta` index lists) lives in `bridge-core` on top of
/// this. `bridge-napi` only marshals these 8 calls over `ThreadsafeFunction`s.
///
/// Values are bytes; keys are strings. Errors map into `wacore` `StoreError`
/// in the core adapter.
#[async_trait]
pub trait HostStore: Send + Sync + 'static {
    // Mandatory
    async fn get(&self, store: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    async fn set(&self, store: &str, key: &str, value: &[u8]) -> anyhow::Result<()>;
    async fn delete(&self, store: &str, key: &str) -> anyhow::Result<()>;

    // Optional batch — `Ok(false)` == "not supported, caller uses per-key fallback".
    async fn set_many(&self, _store: &str, _entries: &[(String, Vec<u8>)]) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn delete_many(&self, _store: &str, _keys: &[String]) -> anyhow::Result<bool> {
        Ok(false)
    }
    /// `Ok(None)` == "no batch handle, caller does a per-key loop".
    async fn get_many(
        &self,
        _store: &str,
        _keys: &[String],
    ) -> anyhow::Result<Option<Vec<(String, Vec<u8>)>>> {
        Ok(None)
    }

    // Optional enumeration / prefix delete (only invoked when capability set).
    async fn list_keys(&self, _store: &str, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// `Ok(None)` == "no handle".
    async fn delete_prefix(&self, _store: &str, _prefix: &str) -> anyhow::Result<Option<u32>> {
        Ok(None)
    }

    fn capabilities(&self) -> HostStoreCapabilities;
}

/// Separate cache-store seam (wacore `CacheStore`): TTL-aware KV.
#[async_trait]
pub trait HostCacheStore: Send + Sync + 'static {
    async fn get(&self, ns: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    async fn set(
        &self,
        ns: &str,
        key: &str,
        value: &[u8],
        ttl_secs: Option<f64>,
    ) -> anyhow::Result<()>;
    async fn delete(&self, ns: &str, key: &str) -> anyhow::Result<()>;
    async fn clear(&self, ns: &str) -> anyhow::Result<()>;
}

// ── Events ───────────────────────────────────────────────────────────────────

/// Backend that buffers writes (write-back) and must be flushed on shutdown.
/// Implemented by the store backend; lets the client hold a flush handle
/// without naming the concrete backend type.
#[async_trait]
pub trait FlushBackend: Send + Sync + 'static {
    async fn flush_cache(&self) -> anyhow::Result<()>;
}

/// Sink for fully-serialized client events. The core builds a `{ type, data }`
/// `BridgeValue`; the engine materializes it into a host JS value (Long objects,
/// `Uint8Array`, oversized-int→string fallback) and invokes the consumer's
/// `onEvent`. Fire-and-forget, ordered (single-sink FIFO).
pub trait HostEventSink: Send + Sync + 'static {
    fn emit(&self, event: BridgeValue);
}

// ── Media streaming ──────────────────────────────────────────────────────────

/// Sink the engine writes media chunks to (download output, encrypt ciphertext).
#[async_trait]
pub trait HostByteSink: Send + Sync + 'static {
    async fn write(&self, chunk: Vec<u8>) -> anyhow::Result<()>;
}

/// Pull source the engine reads plaintext chunks from (encrypt input). `None`
/// signals EOF.
#[async_trait]
pub trait HostPullSource: Send + Sync + 'static {
    async fn next(&self) -> anyhow::Result<Option<Vec<u8>>>;
}

/// Produces the full (buffered) upload body, called fresh per upload attempt so
/// CDN-failover/retry gets a new body. Matches the old `get_body` factory +
/// `stream_upload_via_js` (which buffered the JS stream before `execute`).
#[async_trait]
pub trait HostBodyFactory: Send + Sync + 'static {
    async fn body(&self) -> anyhow::Result<Vec<u8>>;
}
