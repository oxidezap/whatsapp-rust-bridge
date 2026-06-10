//! `WhatsAppClientCore` — engine-agnostic wrapper around `whatsapp_rust::Client`.
//! The napi `WasmWhatsAppClient` class holds an `Arc<WhatsAppClientCore>` and
//! forwards to it. All lifecycle/orchestration logic (run loop, sync worker,
//! background saver, flush-on-shutdown) lives here, ported 1:1 from the old
//! `wasm_client.rs`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_channel::Receiver;
use wacore::runtime::{AbortHandle, Runtime};
use wacore::store::traits::Backend;
use wacore::types::events::{Event, EventHandler};

use whatsapp_rust::Client;
use whatsapp_rust::store::persistence_manager::PersistenceManager;
use whatsapp_rust::sync_task::MajorSyncTask;

use crate::adapters::{CoreHttpClient, CoreTransportFactory};
use crate::host::{FlushBackend, HostEventSink, HostHttp, HostTransportFactory};
use crate::runtime::TokioRuntime;

/// Bridges wacore events to the host's event sink. `handle_event` is sync and
/// runs on a tokio worker; it serializes to a `Send` `BridgeValue` and fires
/// the sink (fire-and-forget, ordered by the sink).
struct CoreEventHandler {
    sink: Arc<dyn HostEventSink>,
}

impl EventHandler for CoreEventHandler {
    fn handle_event(&self, event: Arc<Event>) {
        if let Some(value) = crate::events::event_to_bridge(&event) {
            self.sink.emit(value);
        }
    }
}

pub struct WhatsAppClientCore {
    pub client: Arc<Client>,
    pub runtime: Arc<dyn Runtime>,
    pub persistence_manager: Arc<PersistenceManager>,
    sync_rx: Mutex<Option<Receiver<MajorSyncTask>>>,
    saver_handle: Mutex<Option<AbortHandle>>,
    run_handle: Mutex<Option<AbortHandle>>,
    sync_worker_handle: Mutex<Option<AbortHandle>>,
    /// Write-back-enabled backend handle (flush on shutdown). None for
    /// write-through / in-memory.
    backend_flush: Option<Arc<dyn FlushBackend>>,
}

impl WhatsAppClientCore {
    /// Build the full client from host-provided abstractions. `backend` is the
    /// `dyn Backend` the persistence manager uses; `backend_flush` is the same
    /// object as a concrete `CoreBackend` when write-back is active.
    pub async fn create(
        transport: Arc<dyn HostTransportFactory>,
        http: Arc<dyn HostHttp>,
        event_sink: Option<Arc<dyn HostEventSink>>,
        backend: Arc<dyn Backend>,
        backend_flush: Option<Arc<dyn FlushBackend>>,
        version: Option<(u32, u32, u32)>,
        cache_spec: crate::cache::CacheSpec,
    ) -> anyhow::Result<Self> {
        let cache_config = crate::cache::build_cache_config(cache_spec);
        let runtime: Arc<dyn Runtime> = Arc::new(TokioRuntime);

        let transport_factory: Arc<dyn wacore::net::TransportFactory> =
            Arc::new(CoreTransportFactory::new(transport));
        let http_client: Arc<dyn wacore::net::HttpClient> = Arc::new(CoreHttpClient::new(http));

        let persistence_manager = Arc::new(
            PersistenceManager::new(backend)
                .await
                .map_err(|e| anyhow::anyhow!("create persistence manager: {e}"))?,
        );

        let (client, sync_rx) = Client::new_with_cache_config(
            runtime.clone(),
            persistence_manager.clone(),
            transport_factory,
            http_client,
            version,
            cache_config,
        )
        .await;

        let saver_handle = persistence_manager.clone().run_background_saver(
            runtime.clone(),
            Duration::from_secs(5),
            client.shutdown_signal(),
        );

        if let Some(sink) = event_sink {
            let handler = Arc::new(CoreEventHandler { sink }) as Arc<dyn EventHandler>;
            client.register_handler(handler);
        }

        Ok(Self {
            client,
            runtime,
            persistence_manager,
            sync_rx: Mutex::new(Some(sync_rx)),
            saver_handle: Mutex::new(Some(saver_handle)),
            run_handle: Mutex::new(None),
            sync_worker_handle: Mutex::new(None),
            backend_flush,
        })
    }

    fn take(slot: &Mutex<Option<AbortHandle>>) -> Option<AbortHandle> {
        slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Spawn the main client loop + sync worker in the background.
    pub fn run(&self) -> anyhow::Result<()> {
        let sync_rx = self
            .sync_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let sync_rx = sync_rx.ok_or_else(|| anyhow::anyhow!("run() already called"))?;

        let worker_client = self.client.clone();
        let worker = self.runtime.spawn(Box::pin(async move {
            while let Ok(task) = sync_rx.recv().await {
                worker_client.process_sync_task(task).await;
            }
        }));
        *self
            .sync_worker_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(worker);

        let client = self.client.clone();
        let handle = self.runtime.spawn(Box::pin(async move {
            client.run().await;
        }));
        *self.run_handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(())
    }

    pub async fn connect(&self) -> anyhow::Result<()> {
        self.client.connect().await
    }

    async fn shutdown_and_flush(&self) {
        if let Some(h) = Self::take(&self.saver_handle) {
            h.abort();
        }
        if let Some(h) = Self::take(&self.run_handle) {
            h.abort();
        }
        if let Some(h) = Self::take(&self.sync_worker_handle) {
            h.abort();
        }
        if let Err(e) = self.persistence_manager.flush().await {
            log::warn!("flush on shutdown failed: {e}");
        }
        if let Some(b) = &self.backend_flush
            && let Err(e) = b.flush_cache().await
        {
            log::warn!("write-back flush on shutdown failed: {e}");
        }
    }

    pub async fn disconnect(&self) {
        self.client.disconnect().await;
        self.shutdown_and_flush().await;
    }

    pub async fn logout(&self) -> anyhow::Result<()> {
        self.client.logout().await?;
        self.shutdown_and_flush().await;
        Ok(())
    }

    pub async fn reconnect(&self) {
        self.client.reconnect_immediately().await;
    }

    pub fn set_auto_reconnect(&self, enabled: bool) {
        self.client
            .enable_auto_reconnect
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn is_logged_in(&self) -> bool {
        self.client.is_logged_in()
    }

    /// Synchronous cleanup for `free()`/`[Symbol.dispose]`: abort the background
    /// tasks so they stop polling and release any event-loop holds. Does not
    /// flush (that's the async `disconnect`/`logout` path).
    pub fn abort_tasks(&self) {
        for slot in [
            &self.saver_handle,
            &self.run_handle,
            &self.sync_worker_handle,
        ] {
            if let Some(h) = Self::take(slot) {
                h.abort();
            }
        }
    }
}

/// Bridge the store backend's inherent `flush_cache` to the `FlushBackend`
/// trait so the client can hold a flush handle without naming `CoreBackend`.
#[async_trait::async_trait]
impl FlushBackend for crate::backend::CoreBackend {
    async fn flush_cache(&self) -> anyhow::Result<()> {
        crate::backend::CoreBackend::flush_cache(self).await
    }
}
