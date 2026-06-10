//! `HostTransport*` over JS transport callbacks (CalleeHandled=false TSFNs).
//! `JsTransportHandle` is the #[napi] class passed to JS `connect(handle)`; its
//! synchronous methods push WS events into the core channel from the JS thread.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;

use bridge_core::host::{HostTransport, HostTransportFactory, HostTransportSink};

#[napi(js_name = "JsTransportHandle")]
pub struct JsTransportHandle {
    sink: Arc<dyn HostTransportSink>,
}

#[napi]
impl JsTransportHandle {
    #[napi(js_name = "onConnected")]
    pub fn on_connected(&self) {
        self.sink.on_connected();
    }
    #[napi(js_name = "onData")]
    pub fn on_data(&self, data: Uint8Array) {
        self.sink.on_data(Bytes::copy_from_slice(&data));
    }
    #[napi(js_name = "onDisconnected")]
    pub fn on_disconnected(&self) {
        self.sink.on_disconnected();
    }
}

pub type ConnectTsfn =
    ThreadsafeFunction<JsTransportHandle, Promise<()>, JsTransportHandle, Status, false>;
pub type SendTsfn = ThreadsafeFunction<Uint8Array, Promise<()>, Uint8Array, Status, false>;
pub type DisconnectTsfn = ThreadsafeFunction<(), Promise<()>, (), Status, false>;

pub struct NapiTransportFactory {
    pub connect: Arc<ConnectTsfn>,
    pub send: Arc<SendTsfn>,
    pub disconnect: Arc<DisconnectTsfn>,
}

struct NapiTransport {
    send: Arc<SendTsfn>,
    disconnect: Arc<DisconnectTsfn>,
}

#[async_trait]
impl HostTransportFactory for NapiTransportFactory {
    async fn connect(
        &self,
        sink: Arc<dyn HostTransportSink>,
    ) -> anyhow::Result<Arc<dyn HostTransport>> {
        let handle = JsTransportHandle { sink };
        let promise = self
            .connect
            .call_async(handle)
            .await
            .map_err(|e| anyhow::anyhow!("transport connect dispatch: {e}"))?;
        promise
            .await
            .map_err(|e| anyhow::anyhow!("transport connect promise: {e}"))?;
        Ok(Arc::new(NapiTransport {
            send: self.send.clone(),
            disconnect: self.disconnect.clone(),
        }))
    }
}

#[async_trait]
impl HostTransport for NapiTransport {
    async fn send(&self, data: Bytes) -> anyhow::Result<()> {
        let arr = Uint8Array::from(data.to_vec());
        let promise = self
            .send
            .call_async(arr)
            .await
            .map_err(|e| anyhow::anyhow!("transport send dispatch: {e}"))?;
        promise
            .await
            .map_err(|e| anyhow::anyhow!("transport send promise: {e}"))?;
        Ok(())
    }

    async fn disconnect(&self) {
        if let Ok(promise) = self.disconnect.call_async(()).await {
            let _ = promise.await;
        }
    }
}
