//! The plugin-side relay pipe: a `wacore` `RelayTransport` over JS.
//!
//! The host installs one `VoipRelayTransport` (see `ts/voip-relay-transport.ts`)
//! through [`set_relay_transport`]; each call then `connect`s an endpoint and
//! gets back a live `VoipRelayConnection` plus the push stream `run_call`
//! consumes. Outbound `send` crosses as a `Uint8Array`; inbound `onPacket`
//! crosses back into the same event queue. The engine never sees the JS
//! objects — only this adapter's trait impl.

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use wacore::voip::transport::{
    RelayDisconnectReason, RelayEndpointParams, RelayTransport, RelayTransportEvent,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// The installed plugin transport: `connect(endpoint, events)` returning a
/// `Promise<connection>`, called once per call.
static TRANSPORT: OnceLock<Mutex<Option<JsTransport>>> = OnceLock::new();

fn transport_slot() -> &'static Mutex<Option<JsTransport>> {
    TRANSPORT.get_or_init(|| Mutex::new(None))
}

#[derive(Clone)]
struct JsTransport {
    connect: js_sys::Function,
}

impl JsTransport {
    fn from_value(value: &JsValue) -> Result<Self, JsValue> {
        let connect = js_sys::Reflect::get(value, &JsValue::from_str("connect"))
            .map_err(|_| JsValue::from_str("relay transport has no connect"))?;
        let connect = connect
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("relay transport connect is not a function"))?;
        Ok(JsTransport { connect })
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
    let endpoint_js = js_endpoint(&endpoint);
    let promise = transport
        .connect
        .call2(&JsValue::UNDEFINED, &endpoint_js, events.handler_object())
        .map_err(|_| "relay connect threw".to_owned())?;
    let connection = JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(|_| "relay connect rejected".to_owned())?;
    let pipe = JsPipe::from_value(&connection)?;
    let (tx, rx) = async_channel::unbounded();
    events.install(tx.clone());
    // The channel is open when `connect` resolved: report it so the driver
    // starts its allocate instead of waiting on an `onOpen` that already
    // happened.
    tx.try_send(RelayTransportEvent::Connected).ok();
    Ok(Dialed {
        transport: Arc::new(ConnectedPipe { pipe, events }) as Arc<dyn RelayTransport>,
        relay_events: rx,
    })
}

/// The two halves `run_call` takes: the packet pipe and its push stream.
pub struct Dialed {
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

/// The push callbacks handed to `connect`: `onPacket`, `onOpen`, `onClose`.
/// One `Arc` per dial owns the sender; the JS closures hold a weak backref
/// and go quiet when the call drops it.
struct JsEvents {
    obj: js_sys::Object,
    inner: Mutex<Inner>,
}

struct Inner {
    sender: Option<async_channel::Sender<RelayTransportEvent>>,
}

impl JsEvents {
    fn new() -> Arc<Self> {
        let events = Arc::new(JsEvents {
            obj: js_sys::Object::new(),
            inner: Mutex::new(Inner { sender: None }),
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
                    let _ =
                        sender.try_send(RelayTransportEvent::PacketReceived(Bytes::from(bytes)));
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
                    .map(|g| g.sender.clone())
                    .unwrap_or(None);
                if let Some(sender) = sender {
                    let reason = reason.as_string().map(RelayDisconnectReason::ReadError);
                    let _ = sender.try_send(RelayTransportEvent::Disconnected(
                        reason.unwrap_or(RelayDisconnectReason::Closed),
                    ));
                }
            }) as Box<dyn FnMut(JsValue)>)
        };
        let _ = js_sys::Reflect::set(
            &events.obj,
            &JsValue::from_str("onPacket"),
            on_packet.as_ref().unchecked_ref(),
        );
        let _ = js_sys::Reflect::set(
            &events.obj,
            &JsValue::from_str("onOpen"),
            on_open.as_ref().unchecked_ref(),
        );
        let _ = js_sys::Reflect::set(
            &events.obj,
            &JsValue::from_str("onClose"),
            on_close.as_ref().unchecked_ref(),
        );
        // The JS object owns the closures now; they hold only a weak backref,
        // so dropping the `Arc` silences them without leaking the call.
        on_packet.forget();
        on_open.forget();
        on_close.forget();
        events
    }

    fn handler_object(&self) -> &JsValue {
        // The object reference the `connect` call receives.
        unsafe { &*(&self.obj as *const js_sys::Object as *const JsValue) }
    }

    fn install(self: &Arc<Self>, sender: async_channel::Sender<RelayTransportEvent>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.sender = Some(sender);
        }
    }
}

/// The live JS connection plus its event callbacks, behind the trait.
struct ConnectedPipe {
    pipe: JsPipe,
    /// Held so the `onPacket`/`onOpen`/`onClose` closures stay able to reach
    /// the sender: they hold only a weak backref, which this keeps alive.
    #[allow(dead_code)]
    events: Arc<JsEvents>,
}

#[derive(Clone)]
struct JsPipe {
    send: js_sys::Function,
    close: js_sys::Function,
    this: JsValue,
}

impl JsPipe {
    fn from_value(value: &JsValue) -> Result<Self, String> {
        let get = |name: &str| {
            js_sys::Reflect::get(value, &JsValue::from_str(name))
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                .ok_or_else(|| format!("relay connection has no {name}"))
        };
        Ok(JsPipe {
            send: get("send")?,
            close: get("close")?,
            this: value.clone(),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RelayTransport for ConnectedPipe {
    async fn send(&self, data: Bytes) -> anyhow::Result<()> {
        let bytes = js_sys::Uint8Array::from(data.as_ref());
        self.pipe
            .send
            .call1(&self.pipe.this, &bytes.into())
            .map_err(|_| anyhow::anyhow!("relay send threw"))?;
        Ok(())
    }

    async fn disconnect(&self) {
        let _ = self.pipe.close.call0(&self.pipe.this);
    }
}
