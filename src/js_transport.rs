//! JS WebSocket transport adapter.
//!
//! Uses raw `js_sys::Function` callbacks instead of wasm-bindgen extern types
//! to avoid reentrancy panics. wasm-bindgen's extern types use an internal
//! object slab with RefCell-like borrowing that panics on reentrant calls
//! (e.g. disconnect → ws.close → ws.onclose → reconnect → connect).

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_channel::Receiver;
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use whatsapp_rust::wacore::net::{DisconnectReason, Transport, TransportEvent, TransportFactory};

// ---------------------------------------------------------------------------
// TypeScript interface (documentation only — actual impl uses raw Functions)
// ---------------------------------------------------------------------------

/// Coordinates a host disconnect callback with an overlapping teardown.
/// Cancelling the wait breaks the only possible callback/teardown cycle; an
/// ordinary callback is still awaited to completion.
pub(crate) struct HostCallbackState {
    next_id: AtomicUsize,
    event_streams: Mutex<Vec<async_channel::WeakSender<TransportEvent>>>,
    cancel: Mutex<Option<(usize, futures::channel::oneshot::Sender<()>)>>,
}

impl HostCallbackState {
    fn new() -> Self {
        Self {
            next_id: AtomicUsize::new(0),
            event_streams: Mutex::new(Vec::new()),
            cancel: Mutex::new(None),
        }
    }

    fn register_event_stream(&self, sender: &async_channel::Sender<TransportEvent>) {
        let mut streams = self.event_streams.lock().unwrap_or_else(|p| p.into_inner());
        streams.retain(|stream| stream.upgrade().is_some_and(|sender| !sender.is_closed()));
        streams.push(sender.downgrade());
    }

    pub(crate) fn close_event_streams(&self) {
        let streams =
            core::mem::take(&mut *self.event_streams.lock().unwrap_or_else(|p| p.into_inner()));
        for stream in streams {
            if let Some(sender) = stream.upgrade() {
                sender.close();
            }
        }
    }

    fn begin_disconnect(
        &self,
    ) -> (
        DisconnectCallbackGuard<'_>,
        futures::channel::oneshot::Receiver<()>,
    ) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = futures::channel::oneshot::channel();
        let previous = self
            .cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace((id, sender));
        if let Some((_, sender)) = previous {
            let _ = sender.send(());
        }
        (DisconnectCallbackGuard { state: self, id }, receiver)
    }

    pub(crate) fn cancel_disconnect_wait(&self) {
        let sender = self
            .cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .map(|(_, sender)| sender);
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
    }
}

struct DisconnectCallbackGuard<'a> {
    state: &'a HostCallbackState,
    id: usize,
}

impl Drop for DisconnectCallbackGuard<'_> {
    fn drop(&mut self) {
        let mut cancel = self
            .state
            .cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cancel.as_ref().is_some_and(|(id, _)| *id == self.id) {
            cancel.take();
        }
    }
}

// ---------------------------------------------------------------------------
// Transport handle — JS pushes WebSocket events through this
// ---------------------------------------------------------------------------

/// Handle given to JS `connect()` for pushing WebSocket events into Rust.
/// Create a JS handle object with onConnected/onData/onDisconnected closures.
///
/// Returns a plain JS object (not a wasm-bindgen struct) to avoid reentrancy.
/// Closures push events into the async_channel without entering WASM.
fn create_js_handle(event_tx: async_channel::Sender<TransportEvent>) -> JsValue {
    let obj = js_sys::Object::new();

    let tx = event_tx.clone();
    let on_data = Closure::wrap(Box::new(move |data: js_sys::Uint8Array| {
        let bytes = crate::js_bytes::to_vec(&data);
        match tx.try_send(TransportEvent::DataReceived(Bytes::from(bytes))) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Closed(_)) => {
                log::debug!("Transport channel closed, data event dropped (shutdown in progress)");
            }
            Err(async_channel::TrySendError::Full(_)) => {
                // Dropping a frame desyncs the noise counter; force reconnect
                // instead — WA redelivers unacked messages on resume.
                log::error!("Transport channel full; closing to force reconnect");
                tx.close();
            }
        }
    }) as Box<dyn FnMut(js_sys::Uint8Array)>);
    let _ = js_sys::Reflect::set(&obj, &"onData".into(), &on_data.into_js_value());

    let tx = event_tx.clone();
    let on_connected =
        Closure::wrap(
            Box::new(move || match tx.try_send(TransportEvent::Connected) {
                Ok(()) => {}
                Err(async_channel::TrySendError::Closed(_)) => {
                    log::debug!("Transport channel closed, Connected event dropped");
                }
                Err(async_channel::TrySendError::Full(_)) => {
                    log::error!("Transport channel full on Connected; closing");
                    tx.close();
                }
            }) as Box<dyn FnMut()>,
        );
    let _ = js_sys::Reflect::set(&obj, &"onConnected".into(), &on_connected.into_js_value());

    let tx = event_tx;
    let on_disconnected = Closure::wrap(Box::new(move || {
        match tx.try_send(TransportEvent::Disconnected(DisconnectReason::Unknown)) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Closed(_)) => {
                log::debug!("Transport channel closed, Disconnected event dropped");
            }
            Err(async_channel::TrySendError::Full(_)) => {
                // Receiver treats channel close as Disconnected (client.rs recv Err).
                log::error!("Transport channel full on Disconnected; closing");
                tx.close();
            }
        }
    }) as Box<dyn FnMut()>);
    let _ = js_sys::Reflect::set(
        &obj,
        &"onDisconnected".into(),
        &on_disconnected.into_js_value(),
    );

    obj.into()
}

// ---------------------------------------------------------------------------
// Internal: raw JS function storage (avoids wasm-bindgen reentrancy)
// ---------------------------------------------------------------------------

generated_transport_callbacks!();

crate::wasm_send_sync!(RawTransportCallbacks);

impl RawTransportCallbacks {
    async fn call_connect_js(&self, handle: JsValue) -> Result<(), anyhow::Error> {
        let result = self
            .connect_fn
            .call1(&JsValue::NULL, &handle)
            .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;
        resolve_maybe(result).await
    }

    async fn call_send(&self, data: &[u8]) -> Result<(), anyhow::Error> {
        if let Some(send_borrowed_fn) = &self.send_borrowed_fn {
            // SAFETY: this capability has an intentionally strict synchronous
            // contract (documented in JsTransportCallbacks). The slice remains
            // alive for the entire JS call; the host immediately passes it to
            // WebSocket.send and neither retains it nor re-enters WASM. A
            // Promise result is rejected because the view cannot cross an
            // asynchronous boundary or a possible WASM memory.grow.
            let uint8 = unsafe { js_sys::Uint8Array::view(data) };
            let result = send_borrowed_fn
                .call1(&JsValue::NULL, &uint8.into())
                .map_err(|e| anyhow::anyhow!("sendBorrowed: {e:?}"))?;
            if !self.send_borrowed_checked.get() {
                if result.is_instance_of::<js_sys::Promise>() {
                    anyhow::bail!("transport.sendBorrowed must return synchronously");
                }
                self.send_borrowed_checked.set(true);
            }
            return Ok(());
        }

        let uint8 = js_sys::Uint8Array::from(data);
        let result = self
            .send_fn
            .call1(&JsValue::NULL, &uint8.into())
            .map_err(|e| anyhow::anyhow!("send: {e:?}"))?;
        resolve_maybe(result).await
    }

    async fn call_disconnect(&self) -> Result<(), anyhow::Error> {
        let (guard, cancel) = self.callback_state.begin_disconnect();
        let result = self
            .disconnect_fn
            .call0(&JsValue::NULL)
            .map_err(|e| anyhow::anyhow!("disconnect: {e:?}"))?;
        if result.is_instance_of::<js_sys::Promise>() {
            let callback = resolve_maybe(result);
            futures::pin_mut!(callback);
            futures::select! {
                _ = callback.fuse() => {}
                _ = cancel.fuse() => {}
            }
        }
        drop(guard);
        Ok(())
    }
}

async fn resolve_maybe(val: JsValue) -> Result<(), anyhow::Error> {
    if val.is_instance_of::<js_sys::Promise>() {
        let promise = js_sys::Promise::unchecked_from_js(val);
        let future: JsFuture = JsFuture::from(promise);
        let _result: JsValue = future.await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Transport implementation
// ---------------------------------------------------------------------------

struct JsTransportInner {
    callbacks: Arc<RawTransportCallbacks>,
    disconnected: AtomicBool,
}

crate::wasm_send_sync!(JsTransportInner);

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Transport for JsTransportInner {
    async fn send(&self, data: Bytes) -> Result<(), anyhow::Error> {
        if self.disconnected.load(Ordering::Acquire) {
            anyhow::bail!("transport is disconnected");
        }
        log::trace!("Transport::send {} bytes", data.len());
        self.callbacks.call_send(&data).await
    }

    async fn disconnect(&self) {
        // The core may close a transport explicitly and then reach the
        // authoritative cleanup path, which closes it again. The callback is
        // shared across transport generations, so forwarding the second call
        // could close the newly-created WebSocket instead of the old one.
        if self.disconnected.swap(true, Ordering::AcqRel) {
            log::debug!("Transport::disconnect ignored (already disconnected)");
            return;
        }
        log::debug!("Transport::disconnect called");
        let _ = self.callbacks.call_disconnect().await;
        log::debug!("Transport::disconnect completed");
    }
}

// ---------------------------------------------------------------------------
// TransportFactory
// ---------------------------------------------------------------------------

pub struct JsTransportFactory {
    callbacks: Arc<RawTransportCallbacks>,
}

crate::wasm_send_sync!(JsTransportFactory);

impl JsTransportFactory {
    pub fn from_js(obj: JsValue) -> Result<Self, JsValue> {
        Ok(Self {
            callbacks: Arc::new(RawTransportCallbacks::from_js(obj)?),
        })
    }

    pub(crate) fn callback_state(&self) -> Arc<HostCallbackState> {
        self.callbacks.callback_state.clone()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl TransportFactory for JsTransportFactory {
    async fn create_transport(
        &self,
    ) -> Result<(Arc<dyn Transport>, Receiver<TransportEvent>), anyhow::Error> {
        log::debug!("JsTransportFactory::create_transport called");
        // Sized for burst absorption; overflow is a real lag signal, not noise.
        let (event_tx, event_rx) = async_channel::bounded(32768);
        self.callbacks
            .callback_state
            .register_event_stream(&event_tx);
        let handle = create_js_handle(event_tx);

        log::debug!("Calling JS connect(handle)...");
        self.callbacks.call_connect_js(handle).await?;
        log::debug!("JS connect(handle) returned successfully");

        let transport = Arc::new(JsTransportInner {
            callbacks: self.callbacks.clone(),
            disconnected: AtomicBool::new(false),
        }) as Arc<dyn Transport>;

        Ok((transport, event_rx))
    }
}

#[cfg(test)]
mod generated_host_callback_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    #[test]
    fn registration_keeps_getter_order_and_required_throw_identity() {
        let value=js_sys::eval(r#"(() => { const reads=[], obj={reads};
            for(const name of ["connect", "send", "sendBorrowed", "disconnect"]) Object.defineProperty(obj,name,{get(){reads.push(name);return ()=>{};}});
            return obj; })()"#).unwrap();
        RawTransportCallbacks::from_js(value.clone()).unwrap();
        let reads: Vec<String> =
            serde_wasm_bindgen::from_value(js_sys::Reflect::get(&value, &"reads".into()).unwrap())
                .unwrap();
        assert_eq!(reads, vec!["connect", "send", "sendBorrowed", "disconnect"]);
        let value=js_sys::eval(r#"(() => {const marker={marker:true}; return {marker,get connect(){throw marker;}};})()"#).unwrap();
        let marker = js_sys::Reflect::get(&value, &"marker".into()).unwrap();
        let error = RawTransportCallbacks::from_js(value.clone()).err().unwrap();
        assert_eq!(error, marker);
        let value = js_sys::eval(r#"({connect:42})"#).unwrap();
        let error = RawTransportCallbacks::from_js(value.clone()).err().unwrap();
        assert_eq!(
            error.as_string().unwrap(),
            "transport.connect must be a function"
        );
    }
    #[test]
    fn optional_borrowed_send_remains_tolerant() {
        for source in [
            "({connect(){},send(){},disconnect(){},get sendBorrowed(){throw 17;}})",
            "({connect(){},send(){},disconnect(){},sendBorrowed:42})",
        ] {
            let value = js_sys::eval(source).unwrap();
            let callbacks = RawTransportCallbacks::from_js(value.clone()).unwrap();
            assert!(callbacks.send_borrowed_fn.is_none());
            assert!(!callbacks.send_borrowed_checked.get());
            assert_eq!(callbacks._js_obj, value);
        }
    }
}
