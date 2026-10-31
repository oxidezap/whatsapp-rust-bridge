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
    let (tx, rx) = async_channel::bounded(64);
    events.install(tx.clone());
    let endpoint_js = js_endpoint(&endpoint);
    let promise = transport
        .connect
        .call2(&transport.receiver, &endpoint_js, events.handler_object())
        .map_err(|_| "relay connect threw".to_owned())?;
    let connection = JsFuture::from(js_sys::Promise::resolve(&promise))
        .await
        .map_err(|_| "relay connect rejected".to_owned())?;
    let pipe = JsPipe::from_value(&connection)?;
    // The channel is open when `connect` resolved: report it so the driver
    // starts its allocate instead of waiting on an `onOpen` that already
    // happened.
    tx.try_send(RelayTransportEvent::Connected).ok();
    let closing = pipe.clone();
    let close: wacore::voip_control::VideoTeardownHook = Box::new(move || {
        close_detached(&closing.close, &closing.this);
    });
    Ok(Dialed {
        close,
        transport: Arc::new(ConnectedPipe {
            pipe,
            events,
            endpoint,
            relay_events: rx.clone(),
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
                    sender.close();
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
    relay_events: async_channel::Receiver<RelayTransportEvent>,
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
    reconnect: js_sys::Function,
    this: JsValue,
}

fn close_detached(close: &js_sys::Function, receiver: &JsValue) {
    if let Ok(result) = close.call0(receiver) {
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let _ = JsFuture::from(js_sys::Promise::resolve(&result)).await;
        });
        #[cfg(not(target_arch = "wasm32"))]
        let _ = result;
    }
}

impl JsPipe {
    fn from_value(value: &JsValue) -> Result<Self, String> {
        let get = |name: &str| {
            js_sys::Reflect::get(value, &JsValue::from_str(name))
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                .ok_or_else(|| format!("relay connection has no {name}"))
        };
        let close = get("close")?;
        let methods =
            get("send").and_then(|send| get("reconnect").map(|reconnect| (send, reconnect)));
        let (send, reconnect) = match methods {
            Ok(methods) => methods,
            Err(error) => {
                close_detached(&close, value);
                return Err(error);
            }
        };
        Ok(JsPipe {
            send,
            close,
            reconnect,
            this: value.clone(),
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl RelayTransport for ConnectedPipe {
    async fn send(&self, data: Bytes) -> anyhow::Result<()> {
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
        let result = self
            .pipe
            .reconnect
            .call1(&self.pipe.this, &js_endpoint(&endpoint))
            .map_err(|_| anyhow::anyhow!("relay reconnect threw"))?;
        JsFuture::from(js_sys::Promise::resolve(&result))
            .await
            .map_err(|_| anyhow::anyhow!("relay reconnect rejected"))?;
        Ok((
            Arc::new(ConnectedPipe {
                pipe: self.pipe.clone(),
                events: self.events.clone(),
                endpoint,
                relay_events: self.relay_events.clone(),
            }),
            self.relay_events.clone(),
        ))
    }

    async fn disconnect(&self) {
        if let Ok(result) = self.pipe.close.call0(&self.pipe.this) {
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

    #[wasm_bindgen_test]
    fn invalid_opened_connections_close_once_with_their_receiver() {
        let make = js_sys::Function::new_no_args(
            "return {closed:0,send(){},reconnect(){},close(){this.closed++;return Promise.resolve();}};",
        );
        for method in ["send", "reconnect"] {
            for kind in 0..3 {
                let handle: js_sys::Object =
                    make.call0(&JsValue::UNDEFINED).unwrap().unchecked_into();
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
