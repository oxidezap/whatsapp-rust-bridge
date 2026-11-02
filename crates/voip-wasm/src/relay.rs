//! The plugin-side relay pipe: a `wacore` `RelayTransport` over JS.
//!
//! The host installs one `VoipRelayTransport` (see `ts/voip-relay-transport.ts`)
//! through [`set_relay_transport`]; each call then `connect`s an endpoint and
//! gets back a live `VoipRelayConnection` plus the push stream `run_call`
//! consumes. Outbound `send` crosses as a `Uint8Array`; inbound `onPacket`
//! crosses back into the same event queue. The engine never sees the JS
//! objects — only this adapter's trait impl.

use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use async_trait::async_trait;
use bytes::Bytes;
use wacore::voip::transport::{
    RelayDisconnectReason, RelayEndpointParams, RelayTransport, RelayTransportEvent,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Bounded ingress shared by connection events and datagrams.
pub(crate) const RELAY_EVENT_CAPACITY: usize = 64;

/// The installed plugin transport: `connect(endpoint, events)` returning a
/// `Promise<connection>`, called once per call.
static TRANSPORT: OnceLock<Mutex<Option<JsTransport>>> = OnceLock::new();

fn transport_slot() -> &'static Mutex<Option<JsTransport>> {
    TRANSPORT.get_or_init(|| Mutex::new(None))
}

#[derive(Clone)]
struct JsTransport {
    connect: js_sys::Function,
    receiver: JsValue,
}

impl JsTransport {
    fn from_value(value: &JsValue) -> Result<Self, JsValue> {
        let connect = js_sys::Reflect::get(value, &JsValue::from_str("connect"))
            .map_err(|_| JsValue::from_str("relay transport has no connect"))?;
        let connect = connect
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("relay transport connect is not a function"))?;
        Ok(JsTransport {
            connect,
            receiver: value.clone(),
        })
    }
}

/// Installs the plugin's relay transport factory. The bridge's `createClient`
/// never calls this: only the plugin host does, when it passes `voipBackend`.
#[wasm_bindgen]
pub fn set_relay_transport(transport: JsValue) -> Result<(), JsValue> {
    let parsed = JsTransport::from_value(&transport)?;
    if let Ok(mut slot) = transport_slot().lock() {
        *slot = Some(parsed);
    }
    Ok(())
}

/// Dials one endpoint through the installed JS transport and returns the
/// engine-side halves `run_call` needs: the `Arc<dyn RelayTransport>` and
/// the push stream of [`RelayTransportEvent`].
pub async fn dial(endpoint: RelayEndpointParams) -> Result<Dialed, String> {
    let transport = transport_slot()
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or(None)
        .ok_or_else(|| "no relay transport installed".to_owned())?;
    let events = JsEvents::new();
    let (tx, rx) = async_channel::bounded(RELAY_EVENT_CAPACITY);
    events.install(tx.clone());
    let endpoint_js = js_endpoint(&endpoint);
    let promise = transport
        .connect
        .call2(&transport.receiver, &endpoint_js, events.handler_object())
        .map_err(|_| "relay connect threw".to_owned())?;
    let promise = js_sys::Promise::resolve(&promise);
    let mut pending = PendingConnect(Some(promise.clone()));
    let result = JsFuture::from(promise).await;
    pending.0.take();
    let connection = result.map_err(|_| "relay connect rejected".to_owned())?;
    let pipe = JsPipe::from_value(&connection)?;
    // The channel is open when `connect` resolved: report it so the driver
    // starts its allocate instead of waiting on an `onOpen` that already
    // happened.
    tx.try_send(RelayTransportEvent::Connected).ok();
    let closing = pipe.clone();
    let close: wacore::voip_control::VideoTeardownHook = Box::new(move || {
        closing.close_once_detached();
    });
    Ok(Dialed {
        close,
        transport: Arc::new(ConnectedPipe {
            pipe,
            events,
            endpoint,
        }) as Arc<dyn RelayTransport>,
        relay_events: rx,
    })
}

/// The two halves `run_call` takes: the packet pipe and its push stream.
pub struct Dialed {
    pub close: wacore::voip_control::VideoTeardownHook,
    pub transport: Arc<dyn RelayTransport>,
    pub relay_events: async_channel::Receiver<RelayTransportEvent>,
}

fn js_endpoint(endpoint: &RelayEndpointParams) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("address"),
        &JsValue::from_str(&endpoint.addr.ip().to_string()),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("port"),
        &JsValue::from(endpoint.addr.port()),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("iceUfrag"),
        &JsValue::from_str(&endpoint.ice_ufrag),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &JsValue::from_str("icePwd"),
        &JsValue::from_str(&endpoint.ice_pwd),
    );
    obj.into()
}

fn add_drops(dropped: &AtomicU32, count: u32) {
    let _ = dropped.try_update(Ordering::Relaxed, Ordering::Relaxed, |pending| {
        Some(pending.saturating_add(count))
    });
}

fn push_packet(
    sender: &async_channel::Sender<RelayTransportEvent>,
    dropped: &AtomicU32,
    packet: Bytes,
) {
    let free = sender
        .capacity()
        .map_or(usize::MAX, |capacity| capacity.saturating_sub(sender.len()));
    // Leave room for the packet as well as the accumulated loss notice.
    if free >= 2 {
        let pending = dropped.swap(0, Ordering::AcqRel);
        if pending > 0
            && sender
                .try_send(RelayTransportEvent::InboundDropped(pending))
                .is_err()
        {
            add_drops(dropped, pending);
        }
    }
    if matches!(
        sender.try_send(RelayTransportEvent::PacketReceived(packet)),
        Err(async_channel::TrySendError::Full(_))
    ) {
        add_drops(dropped, 1);
    }
}

fn push_close(
    sender: &async_channel::Sender<RelayTransportEvent>,
    dropped: &AtomicU32,
    event: RelayTransportEvent,
) {
    let pending = dropped.swap(0, Ordering::AcqRel);
    if pending > 0
        && sender
            .try_send(RelayTransportEvent::InboundDropped(pending))
            .is_err()
    {
        let sender = sender.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = sender
                .send(RelayTransportEvent::InboundDropped(pending))
                .await;
            let _ = sender.send(event).await;
            sender.close();
        });
        return;
    }
    match sender.try_send(event) {
        Ok(()) => {
            sender.close();
        }
        Err(async_channel::TrySendError::Full(event)) => {
            let sender = sender.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let _ = sender.send(event).await;
                sender.close();
            });
        }
        Err(async_channel::TrySendError::Closed(_)) => {}
    }
}

/// The push callbacks handed to `connect`: `onPacket`, `onOpen`, `onClose`.
/// One `Arc` per dial owns the sender; the JS closures hold a weak backref
/// and go quiet when the call drops it.
struct JsEvents {
    obj: js_sys::Object,
    inner: Mutex<Inner>,
    dropped: AtomicU32,
}

struct Inner {
    sender: Option<async_channel::Sender<RelayTransportEvent>>,
}

impl JsEvents {
    fn new() -> Arc<Self> {
        let events = Arc::new(JsEvents {
            obj: js_sys::Object::new(),
            inner: Mutex::new(Inner { sender: None }),
            dropped: AtomicU32::new(0),
        });
        let on_packet = {
            let weak = Arc::downgrade(&events);
            Closure::wrap(Box::new(move |data: js_sys::Uint8Array| {
                let Some(events) = weak.upgrade() else { return };
                let sender = events
                    .inner
                    .lock()
                    .map(|g| g.sender.clone())
                    .unwrap_or(None);
                if let Some(sender) = sender {
                    let mut bytes = vec![0u8; data.length() as usize];
                    data.copy_to(&mut bytes);
                    push_packet(&sender, &events.dropped, Bytes::from(bytes));
                }
            }) as Box<dyn FnMut(js_sys::Uint8Array)>)
        };
        let on_open = {
            let weak = Arc::downgrade(&events);
            Closure::wrap(Box::new(move || {
                let Some(events) = weak.upgrade() else { return };
                let sender = events
                    .inner
                    .lock()
                    .map(|g| g.sender.clone())
                    .unwrap_or(None);
                if let Some(sender) = sender {
                    let _ = sender.try_send(RelayTransportEvent::Connected);
                }
            }) as Box<dyn FnMut()>)
        };
        let on_close = {
            let weak = Arc::downgrade(&events);
            Closure::wrap(Box::new(move |reason: JsValue| {
                let Some(events) = weak.upgrade() else { return };
                let sender = events
                    .inner
                    .lock()
                    .map(|mut g| g.sender.take())
                    .unwrap_or(None);
                if let Some(sender) = sender {
                    let reason = reason.as_string().map(RelayDisconnectReason::ReadError);
                    push_close(
                        &sender,
                        &events.dropped,
                        RelayTransportEvent::Disconnected(
                            reason.unwrap_or(RelayDisconnectReason::Closed),
                        ),
                    );
                }
            }) as Box<dyn FnMut(JsValue)>)
        };
        let _ = js_sys::Reflect::set(
            &events.obj,
            &JsValue::from_str("onPacket"),
            &on_packet.into_js_value(),
        );
        let _ = js_sys::Reflect::set(
            &events.obj,
            &JsValue::from_str("onOpen"),
            &on_open.into_js_value(),
        );
        let _ = js_sys::Reflect::set(
            &events.obj,
            &JsValue::from_str("onClose"),
            &on_close.into_js_value(),
        );
        events
    }

    fn handler_object(&self) -> &JsValue {
        // The object reference the `connect` call receives.
        self.obj.as_ref()
    }

    fn install(self: &Arc<Self>, sender: async_channel::Sender<RelayTransportEvent>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.sender = Some(sender);
        }
    }
}

/// The live JS connection plus its event callbacks, behind the trait.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct ConnectedPipe {
    pipe: JsPipe,
    endpoint: RelayEndpointParams,
    /// Held so the `onPacket`/`onOpen`/`onClose` closures stay able to reach
    /// the sender: they hold only a weak backref, which this keeps alive.
    #[allow(dead_code)]
    events: Arc<JsEvents>,
}

#[derive(Clone)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct JsPipe {
    send: js_sys::Function,
    close: js_sys::Function,
    this: JsValue,
    closed: Arc<AtomicBool>,
}

fn observe_close(result: JsValue) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let _ = JsFuture::from(js_sys::Promise::resolve(&result)).await;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = result;
}

fn close_detached(close: &js_sys::Function, receiver: &JsValue) {
    if let Ok(result) = close.call0(receiver) {
        observe_close(result);
    }
}

// Cancellation must release the engine immediately, not wait for a host Promise.
// Only a JS-owned, capture-free cleanup remains to close a late connection.
struct PendingConnect(Option<js_sys::Promise>);

impl Drop for PendingConnect {
    fn drop(&mut self) {
        let Some(promise) = self.0.take() else { return };
        let close = Closure::wrap(Box::new(|value: JsValue| {
            if let Ok(close) = js_sys::Reflect::get(&value, &"close".into())
                && let Ok(close) = close.dyn_into::<js_sys::Function>()
            {
                close_detached(&close, &value);
            }
        }) as Box<dyn FnMut(JsValue)>)
        .into_js_value();
        let ignore =
            Closure::wrap(Box::new(|_: JsValue| {}) as Box<dyn FnMut(JsValue)>).into_js_value();
        if let Ok(then) = js_sys::Reflect::get(promise.as_ref(), &"then".into())
            && let Ok(then) = then.dyn_into::<js_sys::Function>()
        {
            let _ = then.call2(promise.as_ref(), &close, &ignore);
        }
    }
}

impl Drop for ConnectedPipe {
    fn drop(&mut self) {
        self.pipe.close_once_detached();
    }
}

impl JsPipe {
    fn begin_close(&self) -> Option<JsValue> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.close.call0(&self.this).ok()
    }

    fn close_once_detached(&self) {
        if let Some(result) = self.begin_close() {
            observe_close(result);
        }
    }
    fn from_value(value: &JsValue) -> Result<Self, String> {
        let get = |name: &str| {
            js_sys::Reflect::get(value, &JsValue::from_str(name))
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                .ok_or_else(|| format!("relay connection has no {name}"))
        };
        let close = get("close")?;
        let send = match get("send") {
            Ok(send) => send,
            Err(error) => {
                close_detached(&close, value);
                return Err(error);
            }
        };
        Ok(JsPipe {
            send,
            close,
            this: value.clone(),
            closed: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl RelayTransport for ConnectedPipe {
    async fn send(&self, data: Bytes) -> anyhow::Result<()> {
        if self.pipe.closed.load(Ordering::Acquire) {
            anyhow::bail!("relay transport is disconnected");
        }
        let bytes = js_sys::Uint8Array::from(data.as_ref());
        let result = self
            .pipe
            .send
            .call1(&self.pipe.this, &bytes.into())
            .map_err(|_| anyhow::anyhow!("relay send threw"))?;
        JsFuture::from(js_sys::Promise::resolve(&result))
            .await
            .map_err(|_| anyhow::anyhow!("relay send rejected"))?;
        Ok(())
    }

    async fn reconnect(
        &self,
        address: std::net::SocketAddr,
    ) -> anyhow::Result<(
        Arc<dyn RelayTransport>,
        async_channel::Receiver<RelayTransportEvent>,
    )> {
        let mut endpoint = self.endpoint.clone();
        endpoint.addr = address;
        let fresh = dial(endpoint).await.map_err(anyhow::Error::msg)?;
        Ok((fresh.transport, fresh.relay_events))
    }

    async fn disconnect(&self) {
        if let Some(result) = self.pipe.begin_close() {
            let _ = JsFuture::from(js_sys::Promise::resolve(&result)).await;
        }
    }
}

// Native tests exercise the sans-IO engine; JS transports run only on WASM.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl RelayTransport for ConnectedPipe {
    async fn send(&self, _: Bytes) -> anyhow::Result<()> {
        anyhow::bail!("JS relay requires wasm32")
    }
    async fn disconnect(&self) {}
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test(async)]
    async fn reconnect_has_fresh_ingress_and_independent_close_ownership() {
        let provider = js_sys::Function::new_no_args(r#"
          return {connections:[],events:[],connect(endpoint,events) {
            const connection={closed:0,sends:0,send(){this.sends++},close(){this.closed++;events.onClose()}};
            this.connections.push(connection);this.events.push(events);
            return Promise.resolve(connection);
          }};
        "#).call0(&JsValue::UNDEFINED).unwrap();
        set_relay_transport(provider.clone()).unwrap();
        let first = dial(RelayEndpointParams {
            addr: "127.0.0.1:1234".parse().unwrap(),
            ice_ufrag: "ufrag".into(),
            ice_pwd: "password".into(),
        })
        .await
        .unwrap();
        let events =
            js_sys::Array::from(&js_sys::Reflect::get(&provider, &"events".into()).unwrap());
        let old = events.get(0);
        let closed: js_sys::Function = js_sys::Reflect::get(&old, &"onClose".into())
            .unwrap()
            .unchecked_into();
        closed.call0(&old).unwrap();
        let (replacement, received) = first
            .transport
            .reconnect("127.0.0.1:5678".parse().unwrap())
            .await
            .unwrap();
        first.transport.disconnect().await;
        (first.close)();
        let connections =
            js_sys::Array::from(&js_sys::Reflect::get(&provider, &"connections".into()).unwrap());
        let count = |index: u32| {
            js_sys::Reflect::get(&connections.get(index), &"closed".into())
                .unwrap()
                .as_f64()
                .unwrap()
        };
        assert_eq!(count(0), 1.0);
        assert_eq!(
            count(1),
            0.0,
            "retiring the old pipe must not close its replacement"
        );
        let events =
            js_sys::Array::from(&js_sys::Reflect::get(&provider, &"events".into()).unwrap());
        let fresh = events.get(1);
        assert!(!js_sys::Object::is(&old, &fresh));
        let packet: js_sys::Function = js_sys::Reflect::get(&fresh, &"onPacket".into())
            .unwrap()
            .unchecked_into();
        packet
            .call1(&fresh, &js_sys::Uint8Array::from(&[42][..]))
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            RelayTransportEvent::Connected
        ));
        assert!(
            matches!(received.try_recv().unwrap(),RelayTransportEvent::PacketReceived(data) if data.as_ref()==[42])
        );
        replacement.send(Bytes::from_static(&[7])).await.unwrap();
        drop(replacement);
        assert_eq!(
            count(1),
            1.0,
            "aborting/dropping the current transport must close the replacement"
        );
        drop(first.transport);
        assert_eq!(count(0), 1.0, "Drop must share the close-once guard");
    }

    #[wasm_bindgen_test]
    fn overflow_is_reported_once_before_subsequent_packets() {
        let events = JsEvents::new();
        let (sender, receiver) = async_channel::bounded(2);
        events.install(sender);
        let packet: js_sys::Function =
            js_sys::Reflect::get(events.handler_object(), &"onPacket".into())
                .unwrap()
                .unchecked_into();
        for value in 0..5u8 {
            packet
                .call1(
                    events.handler_object(),
                    &js_sys::Uint8Array::from(&[value][..]),
                )
                .unwrap();
        }
        assert_eq!(receiver.len(), 2);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            RelayTransportEvent::PacketReceived(_)
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            RelayTransportEvent::PacketReceived(_)
        ));
        packet
            .call1(events.handler_object(), &js_sys::Uint8Array::from(&[9][..]))
            .unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            RelayTransportEvent::InboundDropped(3)
        ));
        assert!(
            matches!(receiver.try_recv().unwrap(), RelayTransportEvent::PacketReceived(packet) if packet.as_ref() == [9])
        );
        packet
            .call1(
                events.handler_object(),
                &js_sys::Uint8Array::from(&[10][..]),
            )
            .unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            RelayTransportEvent::PacketReceived(_)
        ));
        assert!(receiver.is_empty());
    }

    #[wasm_bindgen_test(async)]
    async fn closing_a_full_pipe_flushes_drops_and_fences_late_packets() {
        let events = JsEvents::new();
        let (sender, receiver) = async_channel::bounded(2);
        events.install(sender);
        let packet: js_sys::Function =
            js_sys::Reflect::get(events.handler_object(), &"onPacket".into())
                .unwrap()
                .unchecked_into();
        for _ in 0..3 {
            packet
                .call1(
                    events.handler_object(),
                    &js_sys::Uint8Array::from(&[0xff][..]),
                )
                .unwrap();
        }
        let close: js_sys::Function =
            js_sys::Reflect::get(events.handler_object(), &"onClose".into())
                .unwrap()
                .unchecked_into();
        close
            .call1(events.handler_object(), &JsValue::UNDEFINED)
            .unwrap();
        packet
            .call1(
                events.handler_object(),
                &js_sys::Uint8Array::from(&[0xff][..]),
            )
            .unwrap();
        assert_eq!(
            events.dropped.load(Ordering::Relaxed),
            0,
            "late teardown packets are fenced"
        );
        for _ in 0..2 {
            assert!(matches!(
                receiver.recv().await.unwrap(),
                RelayTransportEvent::PacketReceived(_)
            ));
        }
        assert!(matches!(
            receiver.recv().await.unwrap(),
            RelayTransportEvent::InboundDropped(1)
        ));
        assert!(matches!(
            receiver.recv().await.unwrap(),
            RelayTransportEvent::Disconnected(_)
        ));
        assert!(receiver.recv().await.is_err());
    }

    #[wasm_bindgen_test]
    fn invalid_opened_connections_close_once_with_their_receiver() {
        let make = js_sys::Function::new_no_args(
            "return {closed:0,send(){},reconnect(){},close(){this.closed++;return Promise.resolve();}};",
        );
        let method = "send";
        for kind in 0..3 {
            let handle: js_sys::Object = make.call0(&JsValue::UNDEFINED).unwrap().unchecked_into();
            if kind == 2 {
                let descriptor = js_sys::Object::new();
                js_sys::Reflect::set(
                    &descriptor,
                    &"get".into(),
                    &js_sys::Function::new_no_args("throw new Error('bad getter');"),
                )
                .unwrap();
                js_sys::Object::define_property(&handle, &method.into(), &descriptor);
            } else {
                let value = if kind == 0 {
                    JsValue::UNDEFINED
                } else {
                    42.into()
                };
                js_sys::Reflect::set(&handle, &method.into(), &value).unwrap();
            }
            let error = JsPipe::from_value(&handle)
                .err()
                .expect("invalid method refused");
            assert!(error.contains(method));
            assert_eq!(
                js_sys::Reflect::get(&handle, &"closed".into())
                    .unwrap()
                    .as_f64(),
                Some(1.0)
            );
        }
        let valid = make.call0(&JsValue::UNDEFINED).unwrap();
        assert!(JsPipe::from_value(&valid).is_ok());
        assert_eq!(
            js_sys::Reflect::get(&valid, &"closed".into())
                .unwrap()
                .as_f64(),
            Some(0.0)
        );
    }
}
