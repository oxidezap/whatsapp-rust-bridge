//! JS relay-transport adapter for call media.
//!
//! Implements the core's `RelayTransportProvider` over a media channel the
//! host builds and owns. The bridge never sees WebRTC: it hands the relay
//! address plus the ICE credentials the call named to one JS constructor,
//! and ships opaque datagrams through the handle that comes back. Inbound
//! datagrams arrive on Rust closures the host invokes, the same shape as the
//! signaling transport's `JsTransportHandle`.
//!
//! Uses raw `js_sys::Function` callbacks instead of wasm-bindgen extern types,
//! for the reason `js_transport.rs` documents: a host callback that
//! re-enters WASM must not meet the extern-type object slab.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_channel::Receiver;
use async_trait::async_trait;
use bytes::Bytes;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use whatsapp_rust::wacore::voip::transport::{
    RelayDisconnectReason, RelayEndpointParams, RelayTransport, RelayTransportEvent,
    RelayTransportFactory, RelayTransportProvider,
};

// The provider callback interfaces live in `wasm_client.rs` as an ungated
// TypeScript section: hosts implement them against any feature set, so the
// declarations must survive `client-calls-audio` being off. The installer
// method itself stays gated with the domain.
const CREATE_METHOD: &str = "createRelayConnection";
const SEND_METHOD: &str = "send";
const CLOSE_METHOD: &str = "close";

/// Inbound relay events per connection. Sized for burst absorption; voice
/// packets arrive at tens per second and the engine drains promptly, so a
/// full channel is a wedged consumer, not jitter.
const RELAY_EVENT_CHANNEL_CAPACITY: usize = 128;

// ---------------------------------------------------------------------------
// Internal: raw JS function storage (avoids wasm-bindgen reentrancy)
// ---------------------------------------------------------------------------

/// Stores the provider callbacks as raw JS functions. `Send + Sync` by the
/// same single-threaded-WASM reasoning as `RawTransportCallbacks`: the
/// functions are only ever invoked on this thread.
struct RawRelayCallbacks {
    create_fn: js_sys::Function,
    /// The original JS object — kept alive to prevent GC.
    _js_obj: JsValue,
}

crate::wasm_send_sync!(RawRelayCallbacks);

impl RawRelayCallbacks {
    /// Extract the constructor from a JS object carrying it.
    fn from_js(obj: JsValue) -> Result<Self, String> {
        let create_fn = js_sys::Reflect::get(&obj, &CREATE_METHOD.into())
            .map_err(|_| "could not read the relay provider callbacks".to_string())?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| "relay provider.createRelayConnection must be a function".to_string())?;
        Ok(Self {
            create_fn,
            _js_obj: obj,
        })
    }

    /// Call the constructor: one channel for `params`, inbound events flowing
    /// into `event_tx` through the closures below.
    async fn call_create(
        &self,
        params: &RelayEndpointParams,
        event_tx: async_channel::Sender<RelayTransportEvent>,
    ) -> Result<JsRelayConnection, anyhow::Error> {
        let params_obj = js_sys::Object::new();
        let address = params.addr.ip().to_string();
        let port = params.addr.port();
        js_sys::Reflect::set(&params_obj, &"address".into(), &address.into())
            .map_err(|e| anyhow::anyhow!("relay params address: {e:?}"))?;
        js_sys::Reflect::set(&params_obj, &"port".into(), &port.into())
            .map_err(|e| anyhow::anyhow!("relay params port: {e:?}"))?;
        js_sys::Reflect::set(
            &params_obj,
            &"iceUfrag".into(),
            &params.ice_ufrag.clone().into(),
        )
        .map_err(|e| anyhow::anyhow!("relay params iceUfrag: {e:?}"))?;
        // Live credential material, crossing because the connectivity checks
        // are built on the host side. It goes into the checks and nowhere else.
        js_sys::Reflect::set(
            &params_obj,
            &"icePwd".into(),
            &params.ice_pwd.clone().into(),
        )
        .map_err(|e| anyhow::anyhow!("relay params icePwd: {e:?}"))?;

        let events = relay_events_object(event_tx);
        // The owning object stays the receiver: a class-based provider
        // reads its state off `this`, and a NULL receiver would detach it.
        let result = self
            .create_fn
            .call2(&self._js_obj, &params_obj.into(), &events)
            .map_err(|e| anyhow::anyhow!("createRelayConnection threw: {e:?}"))?;
        JsRelayConnection::from_js(result).await
    }
}

/// Push one inbound packet, accounting sheds the engine never sees.
///
/// Drops accumulate in `drops` and ride ahead of the next delivered packet
/// — ahead because they happened earlier — instead of being fired into the
/// same full channel, which could never succeed. Reports ride only when
/// they cost no packet slot: with exactly one slot free the packet goes
/// and the count waits, because a report that spends the last slot starves
/// the packet behind it, and a consumer draining one slot per arrival
/// would then watch every packet shed forever — transient saturation
/// turned into a sustained blackout. Single-threaded, so nothing slips
/// between the length check and the sends.
fn push_packet(tx: &async_channel::Sender<RelayTransportEvent>, drops: &AtomicU32, bytes: Bytes) {
    let free = tx
        .capacity()
        .map_or(usize::MAX, |cap| cap.saturating_sub(tx.len()));
    if free >= 2 {
        let pending = drops.swap(0, Ordering::AcqRel);
        if pending > 0
            && tx
                .try_send(RelayTransportEvent::InboundDropped(pending))
                .is_err()
        {
            drops.fetch_add(pending, Ordering::AcqRel);
        }
    }
    match tx.try_send(RelayTransportEvent::PacketReceived(bytes)) {
        Ok(()) => {}
        Err(async_channel::TrySendError::Closed(_)) => {
            log::debug!("Relay channel closed, packet dropped (teardown in progress)");
        }
        // VoIP is loss tolerant: shed here and count it above, rather than
        // grow a queue behind a consumer that stopped reading.
        Err(async_channel::TrySendError::Full(_)) => {
            drops.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// Push the close event, flushing drops first. A close the queue cannot
/// carry still terminates the call: closing the sender ends the stream,
/// and the drive loop breaks on stream end the same way it breaks on the
/// event — so a saturated queue can delay the news, never lose it.
fn push_close(
    tx: &async_channel::Sender<RelayTransportEvent>,
    drops: &AtomicU32,
    event: RelayTransportEvent,
) {
    let pending = drops.swap(0, Ordering::AcqRel);
    if pending > 0
        && tx
            .try_send(RelayTransportEvent::InboundDropped(pending))
            .is_err()
    {
        let tx = tx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(RelayTransportEvent::InboundDropped(pending)).await;
            let _ = tx.send(event).await;
            tx.close();
        });
        return;
    }
    match tx.try_send(event) {
        Ok(()) => {}
        Err(async_channel::TrySendError::Full(event)) => {
            let tx = tx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let _ = tx.send(event).await;
                tx.close();
            });
        }
        Err(async_channel::TrySendError::Closed(_)) => {}
    }
}

/// The events object handed to the constructor. Each closure pushes into the
/// event channel and returns; a closed channel is a teardown in progress,
/// and neither case is answered by blocking a host callback.
fn relay_events_object(event_tx: async_channel::Sender<RelayTransportEvent>) -> JsValue {
    let obj = js_sys::Object::new();
    let drops = Arc::new(AtomicU32::new(0));

    let tx = event_tx.clone();
    let drops_packets = drops.clone();
    let on_packet = Closure::wrap(Box::new(move |data: js_sys::Uint8Array| {
        push_packet(
            &tx,
            &drops_packets,
            Bytes::from(crate::js_bytes::to_vec(&data)),
        );
    }) as Box<dyn FnMut(js_sys::Uint8Array)>);
    let _ = js_sys::Reflect::set(&obj, &"onPacket".into(), &on_packet.into_js_value());

    let tx = event_tx.clone();
    let on_open = Closure::wrap(Box::new(move || {
        // Informational: the drive loop ignores it, so a drop here costs
        // nothing and must not close a healthy call behind it.
        if tx.try_send(RelayTransportEvent::Connected).is_err() {
            log::debug!("Relay channel closed, open event dropped");
        }
    }) as Box<dyn FnMut()>);
    let _ = js_sys::Reflect::set(&obj, &"onOpen".into(), &on_open.into_js_value());

    let tx = event_tx;
    let on_close = Closure::wrap(Box::new(move |reason: JsValue| {
        let event = if let Some(reason) = reason.as_string().filter(|r| !r.is_empty()) {
            RelayTransportEvent::Disconnected(RelayDisconnectReason::ReadError(reason))
        } else {
            RelayTransportEvent::Disconnected(RelayDisconnectReason::Closed)
        };
        push_close(&tx, &drops, event);
    }) as Box<dyn FnMut(JsValue)>);
    let _ = js_sys::Reflect::set(&obj, &"onClose".into(), &on_close.into_js_value());

    obj.into()
}

/// Await a value that may or may not be a Promise, carrying a thrown
/// rejection as an error. A host `send`/`close` that never settles retains
/// its resolve/reject pair for the life of the process, so the contract
/// above requires settling.
/// Await a host return that may or may not be a Promise.
///
/// Normalized through this realm's `Promise.resolve`, which adopts
/// cross-realm promises and bare thenables alike: a realm-local
/// `instanceof` would misread both as resolved values (and their
/// rejections as successes). A synchronous return resolves on the next
/// microtask, which costs nothing next to the packet copy. A value that
/// never settles retains its resolve/reject pair for the life of the
/// process, so the contract requires settling.
async fn resolve_maybe(what: &str, val: JsValue) -> Result<(), anyhow::Error> {
    // Annotated: the future's output is otherwise unconstrained and
    // inference leaves it unsolved.
    let future: JsFuture = JsFuture::from(js_sys::Promise::resolve(&val));
    future
        .await
        .map_err(|e| anyhow::anyhow!("relay {what} rejected: {e:?}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Connection handle — the constructed channel
// ---------------------------------------------------------------------------

/// The constructed channel: its `send`/`close` functions plus the original
/// object, kept alive to prevent GC.
struct JsRelayConnection {
    send_fn: js_sys::Function,
    close_fn: js_sys::Function,
    /// The original JS handle — kept alive to prevent GC.
    _js_obj: JsValue,
}

impl JsRelayConnection {
    async fn from_js(val: JsValue) -> Result<Self, anyhow::Error> {
        // Normalized like every other host return (see `resolve_maybe`): a
        // cross-realm promise or a bare thenable adopts here instead of
        // being misread as the handle itself.
        //
        // A synchronous handle is a host bug — the contract requires a
        // Promise — but refusing it here would strand a channel the host
        // already opened. Accept it and let the missing-open watchdog below
        // (the engine's own allocate deadline) bound the wait.
        let future: JsFuture = JsFuture::from(js_sys::Promise::resolve(&val));
        let val = future
            .await
            .map_err(|e| anyhow::anyhow!("createRelayConnection rejected: {e:?}"))?;
        Self::from_handle(val)
    }

    fn from_handle(val: JsValue) -> Result<Self, anyhow::Error> {
        let send_fn = js_sys::Reflect::get(&val, &SEND_METHOD.into())
            .map_err(|_| anyhow::anyhow!("relay handle is missing send"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| anyhow::anyhow!("relay handle.send must be a function"))?;
        let close_fn = js_sys::Reflect::get(&val, &CLOSE_METHOD.into())
            .map_err(|_| anyhow::anyhow!("relay handle is missing close"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| anyhow::anyhow!("relay handle.close must be a function"))?;
        Ok(Self {
            send_fn,
            close_fn,
            _js_obj: val,
        })
    }

    async fn call_send(&self, data: &[u8]) -> Result<(), anyhow::Error> {
        // One copy on the way out, matching the bridge's `Vec<u8>`-by-value
        // precedent: linear memory into a typed array the host sends. The
        // handle stays the receiver, for the reason `call_create` names.
        let uint8 = js_sys::Uint8Array::from(data);
        let result = self
            .send_fn
            .call1(&self._js_obj, &uint8.into())
            .map_err(|e| anyhow::anyhow!("relay send threw: {e:?}"))?;
        resolve_maybe("send", result).await
    }

    async fn call_close(&self) -> Result<(), anyhow::Error> {
        let result = self
            .close_fn
            .call0(&self._js_obj)
            .map_err(|e| anyhow::anyhow!("relay close threw: {e:?}"))?;
        resolve_maybe("close", result).await
    }
}

// ---------------------------------------------------------------------------
// Provider + factory + transport
// ---------------------------------------------------------------------------

/// The installed constructor, shared by every call on the client.
pub struct JsRelayTransportProvider {
    callbacks: Arc<RawRelayCallbacks>,
}

crate::wasm_send_sync!(JsRelayTransportProvider);

impl JsRelayTransportProvider {
    pub fn from_js(obj: JsValue) -> Result<Self, String> {
        Ok(Self {
            callbacks: Arc::new(RawRelayCallbacks::from_js(obj)?),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RelayTransportProvider for JsRelayTransportProvider {
    async fn factory(
        &self,
        relay: &RelayEndpointParams,
    ) -> Result<Arc<dyn RelayTransportFactory>, anyhow::Error> {
        Ok(Arc::new(JsRelayTransportFactory {
            callbacks: self.callbacks.clone(),
            params: relay.clone(),
        }))
    }
}

/// Bound to one relay endpoint: carries the ICE credentials the call named.
struct JsRelayTransportFactory {
    callbacks: Arc<RawRelayCallbacks>,
    params: RelayEndpointParams,
}

crate::wasm_send_sync!(JsRelayTransportFactory);

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RelayTransportFactory for JsRelayTransportFactory {
    async fn connect(
        &self,
    ) -> Result<(Arc<dyn RelayTransport>, Receiver<RelayTransportEvent>), anyhow::Error> {
        let (event_tx, event_rx) = async_channel::bounded(RELAY_EVENT_CHANNEL_CAPACITY);
        let connection = self.callbacks.call_create(&self.params, event_tx).await?;
        Ok((
            Arc::new(JsRelayTransport {
                callbacks: self.callbacks.clone(),
                connection,
                params: self.params.clone(),
                disconnected: AtomicBool::new(false),
            }),
            event_rx,
        ))
    }
}

/// One open channel. `Send + Sync` by the single-threaded-WASM reasoning
/// above; every JS invocation happens on this thread.
struct JsRelayTransport {
    callbacks: Arc<RawRelayCallbacks>,
    connection: JsRelayConnection,
    /// The credentials this channel was built with, reused when the relay
    /// migrates to a new address. The core asks for a redial carrying an
    /// address alone, so these may be the retired relay's — refusing would
    /// end a call over a routine migration, which is the worse failure.
    params: RelayEndpointParams,
    disconnected: AtomicBool,
}

crate::wasm_send_sync!(JsRelayTransport);

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RelayTransport for JsRelayTransport {
    async fn send(&self, data: Bytes) -> Result<(), anyhow::Error> {
        if self.disconnected.load(Ordering::Acquire) {
            anyhow::bail!("relay transport is disconnected");
        }
        self.connection.call_send(&data).await
    }

    async fn disconnect(&self) {
        // Double close would tear down the channel a redial already replaced,
        // so only the first call crosses into JS. The `onClose` the contract
        // requires after `close()` is what tells the engine.
        if self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(e) = self.connection.call_close().await {
            log::warn!("Relay close failed: {e}");
        }
    }

    async fn reconnect(
        &self,
        endpoint: std::net::SocketAddr,
    ) -> Result<(Arc<dyn RelayTransport>, Receiver<RelayTransportEvent>), anyhow::Error> {
        let params = RelayEndpointParams {
            addr: endpoint,
            ..self.params.clone()
        };
        let (event_tx, event_rx) = async_channel::bounded(RELAY_EVENT_CHANNEL_CAPACITY);
        let connection = self.callbacks.call_create(&params, event_tx).await?;
        Ok((
            Arc::new(Self {
                callbacks: self.callbacks.clone(),
                connection,
                params,
                disconnected: AtomicBool::new(false),
            }),
            event_rx,
        ))
    }
}

#[cfg(test)]
mod relay_event_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn packet(n: u8) -> Bytes {
        Bytes::from(vec![n])
    }

    /// Sheds accumulate and ride ahead of the next delivered packet, in the
    /// order they happened — never fired into the full channel that just
    /// refused them.
    #[test]
    fn sheds_are_counted_and_packets_keep_flowing() {
        let (tx, rx) = async_channel::bounded(3);
        let drops = AtomicU32::new(0);

        push_packet(&tx, &drops, packet(1));
        push_packet(&tx, &drops, packet(2));
        push_packet(&tx, &drops, packet(3));
        // Full: this shed reaches only the counter.
        push_packet(&tx, &drops, packet(4));
        assert_eq!(drops.load(Ordering::Acquire), 1);

        // The consumer drains one slot per arrival from here on. The count
        // waits while only one slot is free, and every packet still gets
        // through — no report ever starves one.
        for expected in [packet(1), packet(2), packet(3)] {
            match rx.try_recv().expect("a queued packet reads back") {
                RelayTransportEvent::PacketReceived(data) => assert_eq!(data, expected),
                other => panic!("expected the queued packet, got {other:?}"),
            }
            push_packet(&tx, &drops, packet(9));
        }
        assert_eq!(drops.load(Ordering::Acquire), 1);
        // Drained faster than arrivals now: with two slots free the waiting
        // count flushes ahead of the next packet, in the order the two
        // happened — older packet, shed count, new packet.
        for _ in 0..2 {
            match rx.try_recv().expect("a queued packet reads back") {
                RelayTransportEvent::PacketReceived(data) => assert_eq!(data, packet(9)),
                other => panic!("expected the queued packet, got {other:?}"),
            }
        }
        push_packet(&tx, &drops, packet(9));
        match rx.try_recv().expect("a queued packet reads back") {
            RelayTransportEvent::PacketReceived(data) => assert_eq!(data, packet(9)),
            other => panic!("expected the queued packet, got {other:?}"),
        }
        match rx.try_recv().expect("the shed count reads back") {
            RelayTransportEvent::InboundDropped(1) => {}
            other => panic!("expected the shed count, got {other:?}"),
        }
        match rx.try_recv().expect("the new packet follows its count") {
            RelayTransportEvent::PacketReceived(data) => assert_eq!(data, packet(9)),
            other => panic!("expected the queued packet, got {other:?}"),
        }
        assert_eq!(drops.load(Ordering::Acquire), 0);
    }

    /// A close the queue cannot carry still terminates the call: closing
    /// the sender ends the stream, which the drive loop breaks on like the
    /// event itself.
    #[test]
    async fn an_undeliverable_close_still_ends_the_stream() {
        let (tx, rx) = async_channel::bounded(1);
        let drops = AtomicU32::new(3);

        push_packet(&tx, &drops, packet(1));
        push_close(
            &tx,
            &drops,
            RelayTransportEvent::Disconnected(RelayDisconnectReason::Closed),
        );
        // The queued packet still reads back; then the stream ends instead
        // of hanging on a close event that never fit.
        assert!(rx.try_recv().is_ok());
        assert!(matches!(
            rx.recv().await.unwrap(),
            RelayTransportEvent::InboundDropped(3)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RelayTransportEvent::Disconnected(RelayDisconnectReason::Closed)
        ));
        assert!(rx.recv().await.is_err());
    }
}
