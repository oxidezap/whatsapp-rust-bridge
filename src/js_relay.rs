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
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::Receiver;
use async_trait::async_trait;
use bytes::Bytes;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use whatsapp_rust::wacore::voip::transport::{
    RelayDisconnectReason, RelayEndpointParams, RelayTransport, RelayTransportEvent,
    RelayTransportFactory, RelayTransportProvider,
};

// ---------------------------------------------------------------------------
// TypeScript interface (documentation only — actual impl uses raw Functions)
// ---------------------------------------------------------------------------

#[wasm_bindgen(typescript_custom_section)]
const TS_RELAY: &str = r#"
/**
 * Host-owned relay media channel for one call, behind
 * `setRelayTransportProvider`. The bridge implements the core's
 * `RelayTransportProvider` over these three functions and never touches
 * WebRTC itself.
 *
 * `createRelayConnection(params, events)` builds the channel and resolves
 * with its handle once the channel is open and carrying datagrams. The
 * default implementation is `createRtcRelayTransportProvider`, which answers
 * an `RTCPeerConnection` with a synthetic SDP description of the relay and
 * opens the pre-negotiated id=0 DataChannel the relay expects
 * (`ordered: false, maxRetransmits: 0`); a host may supply its own
 * constructor instead, as long as it keeps this contract:
 *
 * - `params` carries the relay address plus the ICE credentials the call
 *   named. `icePwd` is live credential material; it goes into the
 *   connectivity checks and nowhere else.
 * - `events.onPacket(data)` gets every datagram that arrives, exactly once.
 *   VoIP is loss tolerant, so under backpressure the host drops rather than
 *   queues without bound.
 * - `events.onOpen()` fires once the channel carries datagrams. The bridge
 *   reports the relay connected on it.
 * - `events.onClose(reason?)` fires when the channel is gone, including
 *   after `close()` resolves. A string reason reports a transport-level
 *   read error; absent means a clean close.
 * - `handle.send(data)` ships one datagram. It may resolve synchronously.
 * - `handle.close()` tears the channel down and is followed by `onClose`.
 *
 * Every Promise handed back must settle, including on failure: reject rather
 * than leaving it pending. The bridge awaits through `JsFuture`, whose
 * resolve/reject pair is only released when the promise settles.
 */
export interface JsRelayConnectionParams {
    address: string;
    port: number;
    iceUfrag: string;
    icePwd: string;
}

export interface JsRelayConnectionEvents {
    onPacket(data: Uint8Array): void;
    onOpen(): void;
    onClose(reason?: string): void;
}

export interface JsRelayConnectionHandle {
    send(data: Uint8Array): void | Promise<void>;
    close(): void | Promise<void>;
}

export interface JsRelayProviderCallbacks {
    createRelayConnection(
        params: JsRelayConnectionParams,
        events: JsRelayConnectionEvents,
    ): Promise<JsRelayConnectionHandle>;
}
"#;

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
        js_sys::Reflect::set(&params_obj, &"iceUfrag".into(), &params.ice_ufrag.clone().into())
            .map_err(|e| anyhow::anyhow!("relay params iceUfrag: {e:?}"))?;
        // Live credential material, crossing because the connectivity checks
        // are built on the host side. It goes into the checks and nowhere else.
        js_sys::Reflect::set(&params_obj, &"icePwd".into(), &params.ice_pwd.clone().into())
            .map_err(|e| anyhow::anyhow!("relay params icePwd: {e:?}"))?;

        let events = relay_events_object(event_tx);
        let result = self
            .create_fn
            .call2(&JsValue::NULL, &params_obj.into(), &events)
            .map_err(|e| anyhow::anyhow!("createRelayConnection threw: {e:?}"))?;
        JsRelayConnection::from_js(result).await
    }
}

/// The events object handed to the constructor. Each closure pushes into the
/// event channel and returns; a closed or full channel is a teardown in
/// progress or a wedged consumer, and neither is answered by blocking a
/// host callback.
fn relay_events_object(
    event_tx: async_channel::Sender<RelayTransportEvent>,
) -> JsValue {
    let obj = js_sys::Object::new();

    let tx = event_tx.clone();
    let on_packet = Closure::wrap(Box::new(move |data: js_sys::Uint8Array| {
        let bytes = Bytes::from(crate::js_bytes::to_vec(&data));
        match tx.try_send(RelayTransportEvent::PacketReceived(bytes)) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Closed(_)) => {
                log::debug!("Relay channel closed, packet dropped (teardown in progress)");
            }
            // VoIP is loss tolerant: shed here and let the engine count it,
            // rather than grow a queue behind a consumer that stopped reading.
            Err(async_channel::TrySendError::Full(_)) => {
                let _ = tx.try_send(RelayTransportEvent::InboundDropped(1));
            }
        }
    }) as Box<dyn FnMut(js_sys::Uint8Array)>);
    let _ = js_sys::Reflect::set(&obj, &"onPacket".into(), &on_packet.into_js_value());

    let tx = event_tx.clone();
    let on_open = Closure::wrap(Box::new(move || {
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
        if tx.try_send(event).is_err() {
            log::debug!("Relay channel closed, close event dropped");
        }
    }) as Box<dyn FnMut(JsValue)>);
    let _ = js_sys::Reflect::set(&obj, &"onClose".into(), &on_close.into_js_value());

    obj.into()
}

/// Await a value that may or may not be a Promise, carrying a thrown
/// rejection as an error. A host `send`/`close` that never settles retains
/// its resolve/reject pair for the life of the process, so the contract
/// above requires settling.
async fn resolve_maybe(what: &str, val: JsValue) -> Result<(), anyhow::Error> {
    if val.is_instance_of::<js_sys::Promise>() {
        let promise = js_sys::Promise::unchecked_from_js(val);
        // Annotated like the signaling transport's copy: the future's output
        // is otherwise unconstrained and inference leaves it unsolved.
        let future: JsFuture = JsFuture::from(promise);
        future
            .await
            .map_err(|e| anyhow::anyhow!("relay {what} rejected: {e:?}"))?;
    }
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
        if val.is_instance_of::<js_sys::Promise>() {
            let promise = js_sys::Promise::unchecked_from_js(val);
            let future: JsFuture = JsFuture::from(promise);
            let val = future
                .await
                .map_err(|e| anyhow::anyhow!("createRelayConnection rejected: {e:?}"))?;
            return Self::from_handle(val);
        }
        // A synchronous handle is a host bug — the contract requires a
        // Promise — but refusing it here would strand a channel the host
        // already opened. Accept it and let the missing-open watchdog below
        // (the engine's own allocate deadline) bound the wait.
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
        // precedent: linear memory into a typed array the host sends.
        let uint8 = js_sys::Uint8Array::from(data);
        let result = self
            .send_fn
            .call1(&JsValue::NULL, &uint8.into())
            .map_err(|e| anyhow::anyhow!("relay send threw: {e:?}"))?;
        resolve_maybe("send", result).await
    }

    async fn call_close(&self) -> Result<(), anyhow::Error> {
        let result = self
            .close_fn
            .call0(&JsValue::NULL)
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
