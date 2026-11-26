//! JS WebSocket transport adapter.
//!
//! Uses raw `js_sys::Function` callbacks instead of wasm-bindgen extern types
//! to avoid reentrancy panics. wasm-bindgen's extern types use an internal
//! object slab with RefCell-like borrowing that panics on reentrant calls
//! (e.g. disconnect → ws.close → ws.onclose → reconnect → connect).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
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

const INGRESS_EVENT_LIMIT: usize = 32768;
// Room for three maximum-size Noise frames, their headers, and the next read.
const INGRESS_BYTE_LIMIT: usize = 64 * 1024 * 1024;

struct Ingress {
    tx: async_channel::Sender<TransportEvent>,
    sizes: VecDeque<usize>,
    queued_bytes: usize,
    byte_limit: usize,
    event_limit: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Admission {
    Ready,
    Closed,
    Overflow,
}

impl Admission {
    fn report(self) -> bool {
        if self == Self::Overflow {
            log::error!("Transport ingress budget exceeded; closing to force reconnect");
        }
        self == Self::Ready
    }
}

impl Ingress {
    fn fits(&self, size: usize) -> bool {
        self.sizes.len() < self.event_limit && size <= self.byte_limit - self.queued_bytes
    }

    fn admission(&mut self, size: usize) -> Admission {
        if self.tx.is_closed() {
            self.clear();
            return Admission::Closed;
        }
        // The JS handle owns all producers. They enqueue synchronously on
        // one thread, so a reduction in the channel length is a FIFO drain.
        let queued = self.tx.len();
        while self.sizes.len() > queued {
            self.queued_bytes -= self.sizes.pop_front().expect("queued event size");
        }
        if self.fits(size) {
            return Admission::Ready;
        }
        self.close_on_overflow()
    }

    fn clear(&mut self) {
        self.sizes = VecDeque::new();
        self.queued_bytes = 0;
    }

    fn close_on_overflow(&mut self) -> Admission {
        // Losing encrypted data invalidates the Noise counter for this socket.
        self.tx.close();
        self.clear();
        Admission::Overflow
    }

    fn enqueue(&mut self, size: usize, event: TransportEvent) -> Admission {
        let admission = self.admission(size);
        if admission != Admission::Ready {
            return admission;
        }
        self.push(size, event)
    }

    fn enqueue_after_copy(&mut self, bytes: Bytes) -> Admission {
        let size = bytes.len();
        let event = TransportEvent::DataReceived(bytes);
        // The JS copy may re-enter the host. The ledger can overcount queued
        // events, so refresh it when this event might exceed a limit.
        if self.fits(size) {
            self.push(size, event)
        } else {
            self.enqueue(size, event)
        }
    }

    fn push(&mut self, size: usize, event: TransportEvent) -> Admission {
        match self.tx.try_send(event) {
            Ok(()) => {
                self.sizes.push_back(size);
                self.queued_bytes += size;
                Admission::Ready
            }
            Err(async_channel::TrySendError::Closed(_)) => {
                self.clear();
                Admission::Closed
            }
            Err(async_channel::TrySendError::Full(_)) => self.close_on_overflow(),
        }
    }
}

fn create_js_handle(
    event_tx: async_channel::Sender<TransportEvent>,
    event_limit: usize,
    byte_limit: usize,
) -> JsValue {
    let obj = js_sys::Object::new();
    let ingress = Rc::new(RefCell::new(Ingress {
        tx: event_tx,
        sizes: VecDeque::new(),
        queued_bytes: 0,
        byte_limit,
        event_limit,
    }));

    let queue = ingress.clone();
    let on_data = Closure::wrap(Box::new(move |data: js_sys::Uint8Array| {
        // Logging can call a JS host callback that re-enters this handle.
        let size = data.length() as usize;
        let admission = queue.borrow_mut().admission(size);
        if !admission.report() {
            return;
        }
        let bytes = Bytes::from(crate::js_bytes::to_vec(&data));
        let admission = queue.borrow_mut().enqueue_after_copy(bytes);
        admission.report();
    }) as Box<dyn FnMut(js_sys::Uint8Array)>);
    let _ = js_sys::Reflect::set(&obj, &"onData".into(), &on_data.into_js_value());

    let queue = ingress.clone();
    let on_connected = Closure::wrap(Box::new(move || {
        let admission = queue.borrow_mut().enqueue(0, TransportEvent::Connected);
        admission.report();
    }) as Box<dyn FnMut()>);
    let _ = js_sys::Reflect::set(&obj, &"onConnected".into(), &on_connected.into_js_value());

    let on_disconnected = Closure::wrap(Box::new(move || {
        let admission = ingress
            .borrow_mut()
            .enqueue(0, TransportEvent::Disconnected(DisconnectReason::Unknown));
        admission.report();
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
    async fn send(&self, data: Bytes) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.disconnected.load(Ordering::Acquire) {
            return Err(std::io::Error::other("transport is disconnected").into());
        }
        log::trace!("Transport::send {} bytes", data.len());
        self.callbacks
            .call_send(&data)
            .await
            .map_err(anyhow::Error::into_boxed_dyn_error)
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
    ) -> Result<
        (Arc<dyn Transport>, Receiver<TransportEvent>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        log::debug!("JsTransportFactory::create_transport called");
        // The JS handle enforces both limits. Allocate queue segments as events arrive.
        let (event_tx, event_rx) = async_channel::unbounded();
        self.callbacks
            .callback_state
            .register_event_stream(&event_tx);
        let handle = create_js_handle(event_tx, INGRESS_EVENT_LIMIT, INGRESS_BYTE_LIMIT);

        log::debug!("Calling JS connect(handle)...");
        self.callbacks
            .call_connect_js(handle)
            .await
            .map_err(anyhow::Error::into_boxed_dyn_error)?;
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

#[cfg(test)]
mod ingress_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn invoke(handle: &JsValue, name: &str, data: Option<u32>) {
        let callback = js_sys::Reflect::get(handle, &name.into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        match data {
            Some(len) => callback.call1(handle, &js_sys::Uint8Array::new_with_length(len)),
            None => callback.call0(handle),
        }
        .unwrap();
    }

    #[wasm_bindgen_test]
    fn ingress_charges_only_queued_bytes_and_closes_on_overflow() {
        let (tx, rx) = async_channel::unbounded();
        let handle = create_js_handle(tx, 16, 8);
        invoke(&handle, "onConnected", None);
        invoke(&handle, "onData", Some(5));
        invoke(&handle, "onData", Some(3));
        assert!(matches!(rx.try_recv().unwrap(), TransportEvent::Connected));
        assert!(
            matches!(rx.try_recv().unwrap(), TransportEvent::DataReceived(data) if data.len() == 5)
        );
        invoke(&handle, "onData", Some(5));
        assert!(!rx.is_closed());
        invoke(&handle, "onData", Some(1));
        assert!(rx.is_closed());
        invoke(&handle, "onData", Some(1));
        for size in [3, 5] {
            assert!(
                matches!(rx.try_recv().unwrap(), TransportEvent::DataReceived(data) if data.len() == size)
            );
        }
        assert!(matches!(
            rx.try_recv(),
            Err(async_channel::TryRecvError::Closed)
        ));
    }

    #[wasm_bindgen_test]
    fn ingress_rechecks_the_budget_after_a_reentrant_copy() {
        let (tx, rx) = async_channel::unbounded();
        let handle = create_js_handle(tx, 1, 8);
        let send = js_sys::Function::new_with_args(
            "handle",
            "const data = new Uint8Array(1); let reads = 0; \
            Object.defineProperty(data, 'length', { get() { \
                if (++reads === 2) handle.onConnected(); return 1; \
            } }); handle.onData(data);",
        );
        send.call1(&JsValue::UNDEFINED, &handle).unwrap();
        assert!(rx.is_closed());
        assert!(matches!(rx.try_recv().unwrap(), TransportEvent::Connected));
        assert!(matches!(
            rx.try_recv(),
            Err(async_channel::TryRecvError::Closed)
        ));
    }

    #[wasm_bindgen_test]
    fn ingress_refresh_after_copy_reuses_a_drained_slot() {
        let (tx, rx) = async_channel::unbounded();
        let mut ingress = Ingress {
            tx,
            sizes: VecDeque::new(),
            queued_bytes: 0,
            byte_limit: 8,
            event_limit: 2,
        };
        assert!(ingress.enqueue(0, TransportEvent::Connected) == Admission::Ready);
        assert!(ingress.admission(1) == Admission::Ready);
        assert!(
            ingress.enqueue(0, TransportEvent::Disconnected(DisconnectReason::Unknown))
                == Admission::Ready
        );
        assert!(matches!(rx.try_recv().unwrap(), TransportEvent::Connected));

        assert!(ingress.enqueue_after_copy(Bytes::from_static(b"x")) == Admission::Ready);
        assert!(!rx.is_closed());
        assert!(matches!(
            rx.try_recv().unwrap(),
            TransportEvent::Disconnected(DisconnectReason::Unknown)
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            TransportEvent::DataReceived(data) if data.as_ref() == b"x"
        ));
    }

    #[wasm_bindgen_test]
    fn ingress_keeps_event_limit_and_isolates_connections() {
        let (tx, rx) = async_channel::unbounded();
        let handle = create_js_handle(tx, 1, 8);
        invoke(&handle, "onData", Some(0));
        invoke(&handle, "onDisconnected", None);
        assert!(rx.is_closed());
        assert!(
            matches!(rx.try_recv().unwrap(), TransportEvent::DataReceived(data) if data.is_empty())
        );

        let (tx, rx) = async_channel::unbounded();
        let handle = create_js_handle(tx, 1, 8);
        invoke(&handle, "onData", Some(9));
        assert!(matches!(
            rx.try_recv(),
            Err(async_channel::TryRecvError::Closed)
        ));

        let (tx, rx) = async_channel::unbounded();
        let handle = create_js_handle(tx, 1, 8);
        invoke(&handle, "onData", Some(8));
        assert!(!rx.is_closed());
        assert!(
            matches!(rx.try_recv().unwrap(), TransportEvent::DataReceived(data) if data.len() == 8)
        );
        invoke(&handle, "onData", Some(1));
        assert!(
            matches!(rx.try_recv().unwrap(), TransportEvent::DataReceived(data) if data.len() == 1)
        );
        assert!(!rx.is_closed());
    }
}
