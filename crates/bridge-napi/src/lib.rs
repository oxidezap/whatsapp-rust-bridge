//! napi-rs v3 binding for the WhatsApp bridge — a thin shell over `bridge-core`.
//! See `docs/napi-migration/DESIGN.md`.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use bridge_core::client::WhatsAppClientCore;
use bridge_core::host::{FlushBackend, HostEventSink, HostHttp, HostStore, HostTransportFactory};

mod host_cache;
mod host_event;
mod host_http;
mod host_media;
mod host_store;
mod host_transport;
mod logger;
mod m_groups;
mod m_identity;
mod m_media;
mod m_messaging;
mod m_newsletter_signal_media;
mod m_profile;
mod methods;
mod value_js;

use host_event::NapiEventSink;
use host_http::NapiHttp;
use host_transport::{JsTransportHandle, NapiTransportFactory};
use value_js::JsBridgeValue;

// ── Module-level free functions ──────────────────────────────────────────────

#[napi(object)]
pub struct EnabledFeatures {
    pub audio: bool,
    pub image: bool,
    pub sticker: bool,
}

#[napi(js_name = "getEnabledFeatures")]
pub fn get_enabled_features() -> EnabledFeatures {
    EnabledFeatures {
        audio: cfg!(feature = "audio"),
        image: cfg!(feature = "image"),
        sticker: cfg!(feature = "sticker"),
    }
}

#[napi(js_name = "getWasmMemoryBytes")]
pub fn get_wasm_memory_bytes() -> f64 {
    0.0
}

/// Engine init. Native crypto/time providers are wacore defaults (no `crypto`
/// arg needed). The TS façade adapts the pino `ILogger` into `log_dispatch`
/// (`(level, target, msg) => void`) + the level string; here we wrap it in a
/// weak, non-blocking TSFN and route Rust `log` through it.
#[napi(js_name = "initWasmEngine")]
pub fn init_wasm_engine(
    env: &Env,
    log_dispatch: Option<Function<FnArgs<(u32, String, String)>, ()>>,
    level: Option<String>,
) -> Result<()> {
    if let Some(f) = log_dispatch {
        let mut tsfn = f.build_threadsafe_function().weak::<true>().build()?;
        tsfn.unref(env)?;
        logger::install(
            Arc::new(tsfn),
            logger::parse_level_filter(level.as_deref().unwrap_or("info")),
        );
    }
    Ok(())
}

// ── createWhatsAppClient ─────────────────────────────────────────────────────

#[napi(js_name = "createWhatsAppClient")]
pub fn create_whatsapp_client<'env>(
    env: &'env Env,
    transport: Object<'_>,
    http_client: Object<'_>,
    on_event: Option<Function<'_, JsBridgeValue, ()>>,
    store: Option<Object<'_>>,
    cache: Option<Object<'_>>,
    version: Option<Vec<u32>>,
) -> Result<PromiseRaw<'env, WasmWhatsAppClient>> {
    // --- SYNC extraction of callbacks → TSFNs (Functions are !Send; used only
    //     here, before the async boundary) ---
    let connect: Function<JsTransportHandle, Promise<()>> =
        transport.get_named_property("connect")?;
    let send: Function<Uint8Array, Promise<()>> = transport.get_named_property("send")?;
    let disconnect: Function<(), Promise<()>> = transport.get_named_property("disconnect")?;

    let execute: Function<host_http::ExecArgsJs, Promise<host_http::HttpExecResult>> =
        http_client.get_named_property("execute")?;

    // Build the TSFNs, then `unref` each so the bridge's callbacks don't
    // independently pin the Node event loop — the consumer's own WebSocket /
    // timers keep the loop alive while active; the process can exit once they
    // (and the client) are released.
    let mut connect_tsfn = connect.build_threadsafe_function().build()?;
    let mut send_tsfn = send.build_threadsafe_function().build()?;
    let mut disconnect_tsfn = disconnect.build_threadsafe_function().build()?;
    let mut execute_tsfn = execute.build_threadsafe_function().build()?;
    connect_tsfn.unref(env)?;
    send_tsfn.unref(env)?;
    disconnect_tsfn.unref(env)?;
    execute_tsfn.unref(env)?;

    let tf: Arc<dyn HostTransportFactory> = Arc::new(NapiTransportFactory {
        connect: Arc::new(connect_tsfn),
        send: Arc::new(send_tsfn),
        disconnect: Arc::new(disconnect_tsfn),
    });
    let http: Arc<dyn HostHttp> = Arc::new(NapiHttp {
        execute: Arc::new(execute_tsfn),
    });

    let sink: Option<Arc<dyn HostEventSink>> = match on_event {
        Some(f) => {
            let mut tsfn = f.build_threadsafe_function().build()?;
            tsfn.unref(env)?;
            Some(Arc::new(NapiEventSink {
                tsfn: Arc::new(tsfn),
            }))
        }
        None => None,
    };

    let version3 = version.and_then(|v| match v.as_slice() {
        [a, b, c] => Some((*a, *b, *c)),
        _ => None,
    });

    // JS-backed store via the HostStore seam (write-back/self-index/expiry live
    // in bridge-core's CoreBackend); in-memory fallback when no store is passed.
    let (backend, backend_flush): (
        Arc<dyn wacore::store::traits::Backend>,
        Option<Arc<dyn FlushBackend>>,
    ) = match store {
        Some(store_obj) => {
            let napi_store: Arc<dyn HostStore> =
                Arc::new(host_store::build_napi_store(env, &store_obj)?);
            let core_backend = bridge_core::backend::new_core_backend(napi_store);
            (core_backend.clone(), Some(core_backend))
        }
        None => (
            Arc::new(wacore::store::in_memory::InMemoryBackend::new()),
            None,
        ),
    };

    // Parse the JS CacheConfig (TTLs/capacities + pluggable stores); default
    // cache when absent. CacheSpec is `Send` so it moves into the async build.
    let cache_spec = match cache {
        Some(cache_obj) => host_cache::build_cache_spec(env, &cache_obj)?,
        None => bridge_core::cache::CacheSpec::default(),
    };

    env.spawn_future(async move {
        let core = WhatsAppClientCore::create(
            tf,
            http,
            sink,
            backend,
            backend_flush,
            version3,
            cache_spec,
        )
        .await
        .map_err(|e| Error::from_reason(format!("createWhatsAppClient: {e}")))?;
        Ok(WasmWhatsAppClient {
            core: Arc::new(core),
        })
    })
}

// ── WasmWhatsAppClient ───────────────────────────────────────────────────────

#[napi(js_name = "WasmWhatsAppClient")]
pub struct WasmWhatsAppClient {
    pub(crate) core: Arc<WhatsAppClientCore>,
}

#[napi]
impl WasmWhatsAppClient {
    #[napi]
    pub fn run(&self) -> Result<()> {
        self.core
            .run()
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    #[napi]
    pub async fn connect(&self) -> Result<()> {
        self.core
            .connect()
            .await
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    #[napi]
    pub async fn disconnect(&self) {
        self.core.disconnect().await;
    }

    #[napi]
    pub async fn logout(&self) -> Result<()> {
        self.core
            .logout()
            .await
            .map_err(|e| Error::from_reason(e.to_string()))
    }

    #[napi(js_name = "reconnect")]
    pub async fn reconnect(&self) {
        self.core.reconnect().await;
    }

    #[napi(js_name = "isConnected")]
    pub fn is_connected(&self) -> bool {
        self.core.is_connected()
    }

    #[napi(js_name = "isLoggedIn")]
    pub fn is_logged_in(&self) -> bool {
        self.core.is_logged_in()
    }

    #[napi(js_name = "setAutoReconnect")]
    pub fn set_auto_reconnect(&self, enabled: bool) {
        self.core.set_auto_reconnect(enabled);
    }

    /// Explicit cleanup (parity with the WASM `free()`/`[Symbol.dispose]`).
    /// Aborts background tasks; the TSFNs are unref'd already, so the process
    /// can exit once the consumer drops its references.
    #[napi]
    pub fn free(&self) {
        self.core.abort_tasks();
    }
}
