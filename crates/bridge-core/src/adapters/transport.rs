//! `wacore::net::{Transport, TransportFactory}` over `HostTransport*`.
//!
//! Owns the bounded event channel and the overflow policy verbatim from the old
//! WASM bridge (`js_transport.rs`): `Full` → `close()` (force reconnect, because
//! dropping a frame desyncs the Noise counter), `Closed` → silent drop (shutdown).

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use wacore::net::{DisconnectReason, Transport, TransportEvent, TransportFactory};

use crate::host::{HostTransport, HostTransportFactory, HostTransportSink};

/// Burst-absorption sizing carried over from the WASM bridge.
const TRANSPORT_CHANNEL_CAP: usize = 32768;

/// Engine-agnostic sink: the host pushes WS events here from the JS thread.
pub struct ChannelSink {
    tx: async_channel::Sender<TransportEvent>,
}

impl ChannelSink {
    fn push(&self, ev: TransportEvent) {
        match self.tx.try_send(ev) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Closed(_)) => {
                log::debug!("transport channel closed; event dropped (shutdown in progress)");
            }
            Err(async_channel::TrySendError::Full(_)) => {
                // Dropping a frame desyncs the Noise counter; force a reconnect
                // instead — WA redelivers unacked messages on resume.
                log::error!("transport channel full; closing to force reconnect");
                self.tx.close();
            }
        }
    }
}

impl HostTransportSink for ChannelSink {
    fn on_connected(&self) {
        self.push(TransportEvent::Connected);
    }
    fn on_data(&self, data: Bytes) {
        self.push(TransportEvent::DataReceived(data));
    }
    fn on_disconnected(&self) {
        self.push(TransportEvent::Disconnected(DisconnectReason::Unknown));
    }
}

/// Wraps a `HostTransport` as a `wacore::net::Transport`.
pub struct CoreTransport {
    inner: Arc<dyn HostTransport>,
}

#[async_trait]
impl Transport for CoreTransport {
    async fn send(&self, data: Bytes) -> Result<(), anyhow::Error> {
        self.inner.send(data).await
    }
    async fn disconnect(&self) {
        self.inner.disconnect().await;
    }
}

/// Wraps a `HostTransportFactory` as a `wacore::net::TransportFactory`.
pub struct CoreTransportFactory {
    host: Arc<dyn HostTransportFactory>,
}

impl CoreTransportFactory {
    pub fn new(host: Arc<dyn HostTransportFactory>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl TransportFactory for CoreTransportFactory {
    async fn create_transport(
        &self,
    ) -> Result<(Arc<dyn Transport>, async_channel::Receiver<TransportEvent>), anyhow::Error> {
        let (tx, rx) = async_channel::bounded(TRANSPORT_CHANNEL_CAP);
        let sink: Arc<dyn HostTransportSink> = Arc::new(ChannelSink { tx });
        // Resolves only when the socket is OPEN (host contract).
        let host_transport = self.host.connect(sink).await?;
        let transport: Arc<dyn Transport> = Arc::new(CoreTransport {
            inner: host_transport,
        });
        Ok((transport, rx))
    }
}
