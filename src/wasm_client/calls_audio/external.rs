//! Call control over a host-owned connection and Signal repository.

use super::*;
use async_trait::async_trait;
use whatsapp_rust::voip::direct::{CallAckFuture, DirectCallHost, DirectCallKeys, DirectCallState};
use whatsapp_rust::wacore::message_processing::EncType;
use whatsapp_rust::wacore::stanza::call::OfferDeviceKey;
use whatsapp_rust::wacore::voip_control::registry::CallRegistry;
use whatsapp_rust::wacore_binary::Node;

#[wasm_bindgen(typescript_custom_section)]
const HOST_TYPES: &str = r#"
export interface DirectCallHostCallbacks {
  ownLid(): string | undefined;
  ownPn(): string | undefined;
  namespaceAlias(peer: string): Promise<string | undefined>;
  learnIdentity(lid: string, pn: string, offline: boolean, hasUsername: boolean): Promise<void>;
  isConnected(): boolean;
  generateRequestId(): string;
  resolvePeer(peer: string): Promise<string>;
  prepareKeys(peer: string, plaintext: Uint8Array): Promise<{
    deviceKeys: { deviceJid: string; ciphertext: Uint8Array; encType: 'msg' | 'pkmsg' }[];
    ringDevices: string[];
    deviceIdentity?: Uint8Array;
  }>;
  decrypt(peer: string, encType: 'msg' | 'pkmsg', ciphertext: Uint8Array): Promise<Uint8Array>;
  readToken(peer: string): Promise<{ token: Uint8Array; timestamp?: number | string } | undefined>;
  issueToken(peer: string): Promise<void>;
  sendNode(node: BinaryNode): Promise<void>;
  /** Registers before sending. cancel releases that registration on completion or cancellation. */
  watchAck(id: string): { promise: Promise<BinaryNode>; cancel(): void };
}
"#;

pub(super) struct JsDirectCallHost {
    receiver: JsValue,
    own_lid: js_sys::Function,
    own_pn: js_sys::Function,
    namespace_alias: js_sys::Function,
    learn_identity: js_sys::Function,
    is_connected: js_sys::Function,
    generate_request_id: js_sys::Function,
    resolve_peer: js_sys::Function,
    prepare_keys: js_sys::Function,
    decrypt: js_sys::Function,
    read_token: js_sys::Function,
    issue_token: js_sys::Function,
    send_node: js_sys::Function,
    watch_ack: js_sys::Function,
    live: std::sync::atomic::AtomicBool,
    cancel: async_channel::Sender<()>,
    cancelled: async_channel::Receiver<()>,
    state: DirectCallState,
    runtime: Arc<dyn wacore::runtime::Runtime>,
}

crate::wasm_send_sync!(JsDirectCallHost);

fn callback_error(operation: &str, error: JsValue) -> whatsapp_rust::CallError {
    let message = js_sys::Reflect::get(&error, &"message".into())
        .ok()
        .and_then(|value| value.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "host callback failed".into());
    let message = format!("{operation}: {message}");
    match operation {
        "decrypt" => whatsapp_rust::CallError::Decrypt(message),
        "sendNode" | "watchAck" => whatsapp_rust::CallError::Response(message),
        _ => whatsapp_rust::CallError::Setup(message),
    }
}

fn property(value: &JsValue, name: &str) -> Result<JsValue, whatsapp_rust::CallError> {
    js_sys::Reflect::get(value, &name.into()).map_err(|error| callback_error(name, error))
}

fn bytes(value: JsValue, name: &str) -> Result<Vec<u8>, whatsapp_rust::CallError> {
    let value = value
        .dyn_into::<js_sys::Uint8Array>()
        .map_err(|_| whatsapp_rust::CallError::Setup(format!("{name} must be a Uint8Array")))?;
    Ok(crate::js_bytes::to_vec(&value))
}

fn jid(value: JsValue, name: &str) -> Result<Jid, whatsapp_rust::CallError> {
    value
        .as_string()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| whatsapp_rust::CallError::Setup(format!("{name} must be a JID")))
}

// The callback may continue after Rust cancellation. A JS-only cleanup retains the input,
// not the controller, until the callback settles; clearing it sooner would corrupt its read.
fn wipe_after_settlement(
    value: JsValue,
    data: &js_sys::Uint8Array,
) -> Result<JsValue, whatsapp_rust::CallError> {
    let promise = js_sys::Promise::resolve(&value);
    let cleanup = property(data.as_ref(), "fill")?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| whatsapp_rust::CallError::Setup("Uint8Array.fill is unavailable".into()))?
        .bind1(data.as_ref(), &JsValue::from_f64(0.0));
    property(promise.as_ref(), "finally")?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| whatsapp_rust::CallError::Setup("Promise.finally is unavailable".into()))?
        .call1(promise.as_ref(), cleanup.as_ref())
        .map_err(|error| callback_error("prepareKeys", error))
}

impl JsDirectCallHost {
    async fn resolve(
        &self,
        value: JsValue,
        operation: &str,
    ) -> Result<JsValue, whatsapp_rust::CallError> {
        let observed = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&value));
        super::super::settle_client_future(self.cancelled.clone(), Box::pin(observed))
            .await
            .map_err(|error| callback_error(operation, error))
    }

    fn new(
        receiver: JsValue,
        registry: Arc<CallRegistry>,
        runtime: Arc<dyn wacore::runtime::Runtime>,
    ) -> Result<Self, crate::errors::BridgeError> {
        let function = |name: &str| -> Result<js_sys::Function, crate::errors::BridgeError> {
            js_sys::Reflect::get(&receiver, &name.into())
                .map_err(|_| {
                    crate::errors::invalid_arg("host", format!("host.{name} is unreadable"))
                })?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| {
                    crate::errors::invalid_arg("host", format!("host.{name} must be a function"))
                })
        };
        let (cancel, cancelled) = async_channel::bounded(1);
        Ok(Self {
            own_lid: function("ownLid")?,
            own_pn: function("ownPn")?,
            namespace_alias: function("namespaceAlias")?,
            learn_identity: function("learnIdentity")?,
            is_connected: function("isConnected")?,
            generate_request_id: function("generateRequestId")?,
            resolve_peer: function("resolvePeer")?,
            prepare_keys: function("prepareKeys")?,
            decrypt: function("decrypt")?,
            read_token: function("readToken")?,
            issue_token: function("issueToken")?,
            send_node: function("sendNode")?,
            watch_ack: function("watchAck")?,
            receiver,
            live: std::sync::atomic::AtomicBool::new(true),
            cancel,
            cancelled,
            state: DirectCallState::new(registry),
            runtime,
        })
    }
}

struct AckRegistration {
    receiver: JsValue,
    cancel: js_sys::Function,
}

impl Drop for AckRegistration {
    fn drop(&mut self) {
        let _ = self.cancel.call0(&self.receiver);
    }
}

#[async_trait(?Send)]
impl DirectCallHost for JsDirectCallHost {
    fn call_state(&self) -> &DirectCallState {
        &self.state
    }
    fn runtime(&self) -> Arc<dyn wacore::runtime::Runtime> {
        self.runtime.clone()
    }
    fn own_lid(&self) -> Option<Jid> {
        self.own_lid
            .call0(&self.receiver)
            .ok()
            .and_then(|value| value.as_string())
            .and_then(|value| value.parse().ok())
    }
    fn own_pn(&self) -> Option<Jid> {
        self.own_pn
            .call0(&self.receiver)
            .ok()
            .and_then(|value| value.as_string())
            .and_then(|value| value.parse().ok())
    }
    async fn namespace_alias(&self, peer: &Jid) -> Option<Jid> {
        let value = self
            .namespace_alias
            .call1(&self.receiver, &peer.to_string().into())
            .ok()?;
        let value = self.resolve(value, "namespaceAlias").await.ok()?;
        if value.is_null() || value.is_undefined() {
            None
        } else {
            jid(value, "namespaceAlias").ok()
        }
    }
    async fn learn_identity(
        &self,
        lid: &Jid,
        pn: &Jid,
        offline: bool,
        has_username: bool,
    ) -> Result<(), whatsapp_rust::CallError> {
        let args = js_sys::Array::new();
        args.push(&lid.to_string().into());
        args.push(&pn.to_string().into());
        args.push(&offline.into());
        args.push(&has_username.into());
        let value = self
            .learn_identity
            .apply(&self.receiver, &args)
            .map_err(|error| callback_error("learnIdentity", error))?;
        self.resolve(value, "learnIdentity").await?;
        Ok(())
    }
    fn is_connected(&self) -> bool {
        self.live.load(std::sync::atomic::Ordering::Relaxed)
            && self
                .is_connected
                .call0(&self.receiver)
                .ok()
                .and_then(|value| value.as_bool())
                == Some(true)
    }
    fn generate_request_id(&self) -> String {
        match self
            .generate_request_id
            .call0(&self.receiver)
            .ok()
            .and_then(|value| value.as_string())
        {
            Some(value) if !value.is_empty() => value,
            _ => {
                self.live.store(false, std::sync::atomic::Ordering::Relaxed);
                String::new()
            }
        }
    }
    async fn resolve_peer(&self, peer: &Jid) -> Result<Jid, whatsapp_rust::CallError> {
        let value = self
            .resolve_peer
            .call1(&self.receiver, &peer.to_string().into())
            .map_err(|error| callback_error("resolvePeer", error))?;
        jid(self.resolve(value, "resolvePeer").await?, "resolvePeer")
    }
    async fn prepare_keys(
        &self,
        peer: &Jid,
        plaintext: &[u8],
    ) -> Result<DirectCallKeys, whatsapp_rust::CallError> {
        let data = js_sys::Uint8Array::from(plaintext);
        let value = self
            .prepare_keys
            .call2(&self.receiver, &peer.to_string().into(), &data)
            .map_err(|error| {
                data.fill(0, 0, data.length());
                callback_error("prepareKeys", error)
            })?;
        let value = wipe_after_settlement(value, &data).inspect_err(|_| {
            data.fill(0, 0, data.length());
        })?;
        let result = self.resolve(value, "prepareKeys").await?;
        let devices = property(&result, "deviceKeys")?;
        let rings = property(&result, "ringDevices")?;
        if !js_sys::Array::is_array(&devices) || !js_sys::Array::is_array(&rings) {
            return Err(whatsapp_rust::CallError::Setup(
                "prepareKeys requires deviceKeys and ringDevices arrays".into(),
            ));
        }
        let mut device_keys = Vec::new();
        for item in js_sys::Array::from(&devices).iter() {
            let device_jid = jid(property(&item, "deviceJid")?, "deviceJid")?;
            let ciphertext = bytes(property(&item, "ciphertext")?, "ciphertext")?;
            if ciphertext.is_empty() {
                return Err(whatsapp_rust::CallError::Setup(
                    "ciphertext must not be empty".into(),
                ));
            }
            let enc_type = property(&item, "encType")?
                .as_string()
                .filter(|value| value == "msg" || value == "pkmsg")
                .ok_or_else(|| {
                    whatsapp_rust::CallError::Setup("encType must be msg or pkmsg".into())
                })?;
            device_keys.push(OfferDeviceKey {
                device_jid,
                ciphertext,
                enc_type,
            });
        }
        let ring_devices = js_sys::Array::from(&rings)
            .iter()
            .map(|value| jid(value, "ringDevices"))
            .collect::<Result<_, _>>()?;
        let identity = property(&result, "deviceIdentity")?;
        let device_identity = if identity.is_null() || identity.is_undefined() {
            None
        } else {
            Some(bytes(identity, "deviceIdentity")?)
        };
        Ok(DirectCallKeys {
            device_keys,
            ring_devices,
            device_identity,
        })
    }
    async fn decrypt(
        &self,
        peer: &Jid,
        enc_type: EncType,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, whatsapp_rust::CallError> {
        let kind = match enc_type {
            EncType::Message => "msg",
            EncType::PreKeyMessage => "pkmsg",
            _ => {
                return Err(whatsapp_rust::CallError::Setup(
                    "unsupported call encryption type".into(),
                ));
            }
        };
        let value = self
            .decrypt
            .call3(
                &self.receiver,
                &peer.to_string().into(),
                &kind.into(),
                &js_sys::Uint8Array::from(ciphertext),
            )
            .map_err(|error| callback_error("decrypt", error))?;
        bytes(self.resolve(value, "decrypt").await?, "decrypt")
    }
    async fn read_token(&self, peer: &Jid) -> Result<Option<Vec<u8>>, whatsapp_rust::CallError> {
        let value = self
            .read_token
            .call1(&self.receiver, &peer.to_string().into())
            .map_err(|error| callback_error("readToken", error))?;
        let value = self.resolve(value, "readToken").await?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else {
            let stamp = property(&value, "timestamp")?;
            let stamp = if let Some(stamp) = stamp.as_string() {
                stamp.parse::<i64>().ok()
            } else {
                stamp
                    .as_f64()
                    .filter(|stamp| {
                        stamp.is_finite()
                            && stamp.fract() == 0.0
                            && *stamp >= 0.0
                            && *stamp <= 9_007_199_254_740_991.0
                    })
                    .map(|stamp| stamp as i64)
            };
            let Some(stamp) = stamp else {
                return Ok(None);
            };
            if wacore::iq::tctoken::is_tc_token_expired_with(stamp, &Default::default()) {
                return Ok(None);
            }
            bytes(property(&value, "token")?, "readToken.token")
                .map(|token| (!token.is_empty()).then_some(token))
        }
    }
    async fn issue_token(&self, peer: &Jid) {
        if !self.is_connected() {
            return;
        }
        let result = self
            .issue_token
            .call1(&self.receiver, &peer.to_string().into());
        let result = match result {
            Ok(value) => self.resolve(value, "issueToken").await,
            Err(error) => Err(callback_error("issueToken", error)),
        };
        if let Err(error) = result {
            log::warn!("call token issuance failed: {error}");
        }
    }
    async fn send_node(&self, node: Node) -> Result<(), whatsapp_rust::CallError> {
        if !self.is_connected() {
            return Err(whatsapp_rust::CallError::Response(
                "host disconnected".into(),
            ));
        }
        let node = node_to_js(&node).map_err(|error| callback_error("sendNode", error))?;
        let value = self
            .send_node
            .call1(&self.receiver, &node)
            .map_err(|error| callback_error("sendNode", error))?;
        self.resolve(value, "sendNode").await?;
        Ok(())
    }
    fn watch_ack(&self, id: &str) -> Result<CallAckFuture, whatsapp_rust::CallError> {
        if !self.is_connected() {
            return Err(whatsapp_rust::CallError::Response(
                "host disconnected".into(),
            ));
        }
        let result = self
            .watch_ack
            .call1(&self.receiver, &id.into())
            .map_err(|error| callback_error("watchAck", error))?;
        let cancel = property(&result, "cancel")?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| {
                whatsapp_rust::CallError::Setup("watchAck.cancel must be a function".into())
            })?;
        let registration = AckRegistration {
            receiver: result.clone(),
            cancel,
        };
        let pending = property(&result, "promise")?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| {
                whatsapp_rust::CallError::Setup("watchAck.promise must be a Promise".into())
            })?;
        // Observe rejection now even if startup is cancelled before this future first polls.
        let observed = wasm_bindgen_futures::JsFuture::from(pending);
        let cancelled = self.cancelled.clone();
        Ok(Box::pin(async move {
            let _registration = registration;
            let value = super::super::settle_client_future(cancelled, Box::pin(observed))
                .await
                .map_err(|error| callback_error("watchAck", error))?;
            js_to_node(&value).map_err(|error| whatsapp_rust::CallError::Setup(error.to_string()))
        }))
    }
}

/// PCM calling on an existing authenticated host. This owns no transport or Signal store.
#[wasm_bindgen]
pub struct WasmCallControl {
    host: Arc<JsDirectCallHost>,
    media: CallMedia,
    on_event: Option<MediaCallback>,
}

crate::wasm_send_sync!(WasmCallControl);

#[wasm_bindgen(js_name = createCallControl, unchecked_return_type = "WasmCallControl")]
#[allow(clippy::arc_with_non_send_sync)] // WASM runtime and registry are local; core contracts share them through Arc.
pub async fn create_call_control(
    #[wasm_bindgen(unchecked_param_type = "DirectCallHostCallbacks")] host: JsValue,
    #[wasm_bindgen(
        unchecked_param_type = "Pick<WhatsAppEventCallbacks, 'onEvent' | 'onCallPcm' | 'onCallEvent'>"
    )]
    callbacks: JsValue,
    extensions: JsValue,
) -> Result<WasmCallControl, crate::errors::BridgeError> {
    create_call_control_with_ceiling(host, callbacks, extensions, crate::voip::OPEN_CEILING).await
}

#[allow(clippy::arc_with_non_send_sync)] // Local WASM registry, as above.
async fn create_call_control_with_ceiling(
    host: JsValue,
    callbacks: JsValue,
    extensions: JsValue,
    ceiling: std::time::Duration,
) -> Result<WasmCallControl, crate::errors::BridgeError> {
    let callbacks = if callbacks.is_null() || callbacks.is_undefined() {
        js_sys::Object::new().into()
    } else {
        callbacks
    };
    let audio = media_callback(&callbacks, "onCallPcm")?.map(std::rc::Rc::new);
    let events = media_callback(&callbacks, "onCallEvent")?.map(std::rc::Rc::new);
    let on_event = media_callback(&callbacks, "onEvent")?;
    let plugin = crate::voip::ClientExtensions::from_js(Some(&extensions))?
        .voip_backend
        .ok_or_else(|| {
            crate::errors::invalid_arg("extensions", "a VoIP media backend is required")
        })?;
    // Standalone consumers have no messaging client initialization barrier.
    crate::js_time::init_time_provider();
    let runtime = Arc::new(crate::runtime::WasmRuntime) as Arc<dyn wacore::runtime::Runtime>;
    let registry = Arc::new(CallRegistry::new());
    let host = Arc::new(JsDirectCallHost::new(
        host,
        registry.clone(),
        runtime.clone(),
    )?);
    let agreed = wacore::runtime::timeout(&*runtime, ceiling, crate::voip::handshake(&plugin))
        .await
        .map_err(|_| {
            crate::errors::invalid_arg("extensions", "VoIP backend handshake timed out")
        })??;
    let backend = Arc::new(crate::voip::ForeignVoipBackend::new(
        plugin.clone(),
        agreed,
        runtime.clone(),
    ));
    plugin.register_push(&backend.push_entry())?;
    if !registry.install_backend(backend) {
        return Err(crate::errors::internal(
            "call registry already has a media session",
        ));
    }
    let media = CallMedia {
        control: CallControl::Direct(host.clone()),
        call_records: std::rc::Rc::new(RefCell::new(HashMap::new())),
        call_offers: Arc::new(Mutex::new(OfferCache::default())),
        past_call_stats: Arc::new(Mutex::new(VecDeque::new())),
        runtime,
        call_audio_callback: None,
        call_pcm_callback: audio,
        call_event_callback: events,
        call_video_callback: None,
        call_generation: std::rc::Rc::new(std::cell::Cell::new(0)),
        call_reserved: std::rc::Rc::new(std::cell::Cell::new(0)),
        calls_live: std::rc::Rc::new(std::cell::Cell::new(true)),
        call_admission: Arc::new(async_lock::Mutex::new(())),
    };
    Ok(WasmCallControl {
        host,
        media,
        on_event,
    })
}

#[wasm_bindgen]
impl WasmCallControl {
    #[wasm_bindgen(js_name = dialCallPcm, unchecked_return_type = "Promise<WasmCallHandle>")]
    pub fn dial_call_pcm(&self, peer: String) -> js_sys::Promise {
        let media = self.media.clone();
        self.operation(async move {
            media
                .dial_call_mode(peer, CallAudioMode::Pcm, false)
                .await
                .map(JsValue::from)
        })
    }

    #[wasm_bindgen(js_name = acceptCallPcm, unchecked_return_type = "Promise<WasmCallHandle>")]
    pub fn accept_call_pcm(&self, call_id: String) -> js_sys::Promise {
        let media = self.media.clone();
        self.operation(async move {
            media
                .accept_call_mode(call_id, CallAudioMode::Pcm, false)
                .await
                .map(JsValue::from)
        })
    }

    #[wasm_bindgen(js_name = callPushPcm16)]
    pub fn push_pcm(
        &self,
        call_id: &str,
        samples: &[i16],
    ) -> Result<bool, crate::errors::BridgeError> {
        push_pcm16(&self.media.call_records, call_id, samples)
    }

    #[wasm_bindgen(js_name = endCall, unchecked_return_type = "Promise<CallEndResult>")]
    pub fn end_call(&self, call_id: String) -> js_sys::Promise {
        let media = self.media.clone();
        self.operation(async move { to_ts(media.end_call(call_id).await?) })
    }

    #[wasm_bindgen(js_name = terminateCall, unchecked_return_type = "Promise<CallEndResult>")]
    pub fn terminate_call(
        &self,
        call_id: String,
        peer: String,
        call_creator: String,
    ) -> js_sys::Promise {
        let media = self.media.clone();
        self.operation(async move {
            let peer = parse_named_jid("peer", &peer)?;
            let creator = parse_named_jid("callCreator", &call_creator)?;
            to_ts(super::terminate_call(&media, call_id, peer, creator).await?)
        })
    }

    #[wasm_bindgen(js_name = setCallMuted, unchecked_return_type = "Promise<void>")]
    pub fn set_muted(&self, call_id: String, muted: bool) -> js_sys::Promise {
        let media = self.media.clone();
        self.operation(async move {
            media.set_call_muted(call_id, muted).await?;
            Ok(JsValue::UNDEFINED)
        })
    }

    #[wasm_bindgen(js_name = getCallMediaStats)]
    pub fn stats(
        &self,
        call_id: &str,
    ) -> Result<Ts<crate::result_types::CallMediaStatsResult>, crate::errors::BridgeError> {
        to_ts(media_stats(
            &self.media.call_records,
            &self.media.past_call_stats,
            call_id,
        )?)
    }

    /// Generic stanza ACKs remain the host's responsibility.
    #[wasm_bindgen(js_name = receiveNode, unchecked_return_type = "Promise<void>")]
    pub fn receive_node(
        &self,
        #[wasm_bindgen(unchecked_param_type = "BinaryNode")] node: JsValue,
    ) -> js_sys::Promise {
        let host = self.host.clone();
        let media = self.media.clone();
        let callback = self.on_event.clone();
        self.operation(async move {
            if !media.calls_live.get() {
                return Err(crate::errors::BridgeError::NotConnected);
            }
            let node = js_to_node(&node)?;
            let incoming = whatsapp_rust::voip::direct::ingest(host, &node.as_node_ref())
                .await
                .map_err(crate::errors::BridgeError::from)?;
            if let Some(incoming) = incoming {
                let event = Event::IncomingCall(Box::new(incoming));
                note_call_event(&media.call_offers, &event);
                if media.calls_live.get()
                    && let Some(callback) = callback
                {
                    let event = event_to_js(&event)
                        .map_err(|_| crate::errors::internal("call event serialization failed"))?;
                    callback
                        .call(&event)
                        .map_err(|_| crate::errors::internal("onEvent threw"))?;
                }
            }
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Release this calling integration without closing the host's WhatsApp connection.
    pub fn disconnect(&self) {
        self.shutdown();
    }
}

impl WasmCallControl {
    fn operation<Fut, T>(&self, operation: Fut) -> js_sys::Promise
    where
        Fut: std::future::Future<Output = Result<T, crate::errors::BridgeError>> + 'static,
        T: Into<JsValue>,
    {
        super::super::client_operation(self.host.cancelled.clone(), async move {
            operation
                .await
                .map(Into::into)
                .map_err(|error| bridge_error_to_js_value(&error))
        })
    }

    fn shutdown(&self) {
        if !self.media.calls_live.replace(false) {
            return;
        }
        self.host
            .live
            .store(false, std::sync::atomic::Ordering::Relaxed);
        whatsapp_rust::voip::direct::disconnect(self.host.as_ref());
        self.host.cancel.close();
        let records = self
            .media
            .call_records
            .borrow()
            .iter()
            .map(|(id, record)| {
                if let Some(task) = &record.forwarder {
                    task.abort();
                }
                (id.clone(), record.generation)
            })
            .collect::<Vec<_>>();
        for (id, generation) in records {
            self.media.finish_call(&id, generation);
        }
        let mut offers = self
            .media
            .call_offers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        offers.entries.clear();
        offers.event_identity.clear();
    }
}

impl Drop for WasmCallControl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)] // Same local WASM registry as the exported constructor.
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn callbacks() -> JsValue {
        js_sys::Function::new_no_args(r#"
          return {
            lid: '100001@lid', calls: 0, cancelled: 0,
            ownLid() { return this.lid },
            ownPn() { return '100001@s.whatsapp.net' },
            namespaceAlias() { return Promise.resolve(undefined) },
            learnIdentity() { this.calls++; return Promise.resolve() },
            isConnected() { return true },
            generateRequestId() { return 'CALL-REQUEST' },
            resolvePeer(peer) { return Promise.resolve(peer) },
            prepareKeys(peer, data) {
              this.calls++; this.plaintext = data;
              return Promise.resolve({
                deviceKeys: [{ deviceJid: peer, ciphertext: new Uint8Array([9,8]), encType:'msg' }],
                ringDevices: [peer], deviceIdentity: new Uint8Array([7])
              })
            },
            decrypt(peer, kind, data) { this.calls++; return Promise.resolve(new Uint8Array([4,5])) },
            readToken(peer) { return Promise.resolve(undefined) },
            issueToken(peer) { this.calls++; return Promise.resolve() },
            sendNode(node) { this.calls++; this.sent = node; return Promise.resolve() },
            watchAck(id) {
              this.calls++;
              return { promise: new Promise(() => {}), cancel: () => { this.cancelled++ } }
            }
          }
        "#).call0(&JsValue::NULL).unwrap()
    }

    fn host(value: JsValue) -> JsDirectCallHost {
        JsDirectCallHost::new(
            value,
            Arc::new(CallRegistry::new()),
            Arc::new(crate::runtime::WasmRuntime),
        )
        .unwrap()
    }

    fn number(value: &JsValue, field: &str) -> f64 {
        property(value, field).unwrap().as_f64().unwrap()
    }

    #[wasm_bindgen_test]
    async fn keys_keep_host_receiver_and_snapshot_returned_bytes() {
        let callbacks = callbacks();
        let host = host(callbacks.clone());
        assert_eq!(host.own_lid().unwrap().to_string(), "100001@lid");
        let peer: Jid = "100002:1@lid".parse().unwrap();
        let keys = host.prepare_keys(&peer, &[1, 2, 3]).await.unwrap();
        assert_eq!(keys.device_keys.len(), 1);
        assert_eq!(keys.device_keys[0].device_jid, peer);
        assert_eq!(keys.device_keys[0].ciphertext, [9, 8]);
        assert_eq!(keys.ring_devices, [peer]);
        assert_eq!(keys.device_identity.unwrap(), [7]);
        assert_eq!(number(&callbacks, "calls"), 1.0);
        // The temporary cross-boundary plaintext no longer retains the call key.
        assert_eq!(
            bytes(property(&callbacks, "plaintext").unwrap(), "plaintext").unwrap(),
            [0, 0, 0]
        );
    }

    #[wasm_bindgen_test]
    fn unpolled_ack_future_owns_cancellation() {
        let callbacks = callbacks();
        let host = host(callbacks.clone());
        let pending = host.watch_ack("CALL-REQUEST").unwrap();
        assert_eq!(number(&callbacks, "calls"), 1.0);
        assert_eq!(number(&callbacks, "cancelled"), 0.0);
        drop(pending);
        assert_eq!(number(&callbacks, "cancelled"), 1.0);
    }

    #[wasm_bindgen_test]
    async fn failed_token_read_rejects_instead_of_becoming_absent() {
        let callbacks = callbacks();
        js_sys::Reflect::set(
            &callbacks,
            &"readToken".into(),
            &js_sys::Function::new_no_args("return Promise.reject(new Error('store unavailable'))"),
        )
        .unwrap();
        let host = host(callbacks);
        let peer = "100002@lid".parse().unwrap();
        let error = host.read_token(&peer).await.unwrap_err();
        assert!(error.to_string().contains("readToken: store unavailable"));
    }

    #[wasm_bindgen_test]
    async fn token_records_use_shared_expiry_without_mutating_storage() {
        crate::js_time::init_time_provider();
        for body in [
            "return Promise.resolve({token:new Uint8Array([7]),timestamp:'1'})",
            "return Promise.resolve({token:new Uint8Array([7])})",
            "return Promise.resolve({token:new Uint8Array(),timestamp:Math.floor(Date.now()/1000)})",
        ] {
            let callbacks = callbacks();
            js_sys::Reflect::set(
                &callbacks,
                &"readToken".into(),
                &js_sys::Function::new_no_args(body),
            )
            .unwrap();
            assert!(
                host(callbacks)
                    .read_token(&"100002@lid".parse().unwrap())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let callbacks = callbacks();
        js_sys::Reflect::set(&callbacks, &"readToken".into(), &js_sys::Function::new_no_args(
            "this.record = {token:new Uint8Array([7]),timestamp:String(Math.floor(Date.now()/1000)),senderTimestamp:1}; return Promise.resolve(this.record)"
        )).unwrap();
        assert_eq!(
            host(callbacks.clone())
                .read_token(&"100002@lid".parse().unwrap())
                .await
                .unwrap(),
            Some(vec![7])
        );
        let record = property(&callbacks, "record").unwrap();
        assert_eq!(
            bytes(property(&record, "token").unwrap(), "token").unwrap(),
            vec![7]
        );
        assert_eq!(number(&record, "senderTimestamp"), 1.0);
    }

    #[wasm_bindgen_test]
    async fn malformed_request_id_prevents_send() {
        let callbacks = callbacks();
        js_sys::Reflect::set(
            &callbacks,
            &"generateRequestId".into(),
            &js_sys::Function::new_no_args("throw new Error('not ready')"),
        )
        .unwrap();
        let host = host(callbacks.clone());
        assert!(host.generate_request_id().is_empty());
        let node = Node::new("call", wacore_binary::node::Attrs::new(), None);
        assert!(host.send_node(node).await.is_err());
        assert_eq!(number(&callbacks, "calls"), 0.0);
    }

    #[wasm_bindgen_test]
    async fn invalid_host_is_rejected_before_plugin_handshake() {
        let plugin = js_sys::Function::new_no_args("return { calls:0, sendFrame() { this.calls++; return Promise.reject(new Error('must not run')) }, setPushHandler() {} }").call0(&JsValue::NULL).unwrap();
        let extensions = js_sys::Object::new();
        js_sys::Reflect::set(&extensions, &"voipBackend".into(), &plugin).unwrap();
        let result = create_call_control(
            js_sys::Object::new().into(),
            JsValue::UNDEFINED,
            extensions.into(),
        )
        .await;
        assert!(
            matches!(result, Err(crate::errors::BridgeError::InvalidArgument { field, .. }) if field == "host")
        );
        assert_eq!(number(&plugin, "calls"), 0.0);
    }
    #[wasm_bindgen_test]
    async fn stalled_backend_handshake_is_bounded() {
        let plugin = js_sys::Function::new_no_args(
            "return {sendFrame() { return new Promise(() => {}) },setPushHandler() {}}",
        )
        .call0(&JsValue::NULL)
        .unwrap();
        let extensions = js_sys::Object::new();
        js_sys::Reflect::set(&extensions, &"voipBackend".into(), &plugin).unwrap();
        let result = create_call_control_with_ceiling(
            callbacks(),
            JsValue::UNDEFINED,
            extensions.into(),
            std::time::Duration::from_millis(1),
        )
        .await;
        assert!(
            matches!(result, Err(crate::errors::BridgeError::InvalidArgument { field, reason }) if field == "extensions" && reason.contains("timed out"))
        );
    }

    fn plugin() -> JsValue {
        js_sys::Function::new_no_args(
            r#"
          return {
            sendFrame() {
              const frame = new Uint8Array(17);
              frame.set([0x4f,0x5a,0x56,0x50,1,1,1,1,0]);
              new DataView(frame.buffer).setUint32(9,4,true);
              new DataView(frame.buffer).setUint32(13,0xffffff,true);
              return Promise.resolve(frame);
            },
            setPushHandler(handler) { this.push = handler }
          }
        "#,
        )
        .call0(&JsValue::NULL)
        .unwrap()
    }

    async fn controller(host: JsValue) -> WasmCallControl {
        let extensions = js_sys::Object::new();
        js_sys::Reflect::set(&extensions, &"voipBackend".into(), &plugin()).unwrap();
        create_call_control(host, JsValue::UNDEFINED, extensions.into())
            .await
            .unwrap()
    }

    #[wasm_bindgen_test]
    async fn disconnect_wins_before_an_unpolled_start() {
        let callbacks = callbacks();
        let control = controller(callbacks.clone()).await;
        let dialing = control.dial_call_pcm("100002@lid".into());
        control.disconnect();
        let error = wasm_bindgen_futures::JsFuture::from(dialing)
            .await
            .unwrap_err();
        assert_eq!(
            property(&error, "kind").unwrap().as_string().as_deref(),
            Some("not-connected")
        );
        assert_eq!(number(&callbacks, "calls"), 0.0);
        assert!(control.media.call_records.borrow().is_empty());
    }

    #[wasm_bindgen_test]
    async fn disconnect_releases_ingress_waiting_on_the_host() {
        let callbacks = callbacks();
        js_sys::Function::new_no_args(
            r#"
          this.entered = new Promise(resolve => { this.enteredResolve = resolve });
          this.learnIdentity = function() { this.enteredResolve(); return new Promise(() => {}) };
        "#,
        )
        .call0(&callbacks)
        .unwrap();
        let control = controller(callbacks.clone()).await;
        let offer = js_sys::Function::new_no_args(r#"
          return {tag:'call',attrs:{from:'100002@lid',id:'OFFER',t:'1'},content:[{
            tag:'offer',attrs:{'call-id':'CALL-1','call-creator':'100002@lid',caller_pn:'100002@s.whatsapp.net'},
            content:[{tag:'audio',attrs:{enc:'opus',rate:'16000'}}]
          }]}
        "#).call0(&JsValue::NULL).unwrap();
        let receiving = control.receive_node(offer);
        let entered = property(&callbacks, "entered")
            .unwrap()
            .dyn_into::<js_sys::Promise>()
            .unwrap();
        wasm_bindgen_futures::JsFuture::from(entered).await.unwrap();
        control.disconnect();
        let error = wasm_bindgen_futures::JsFuture::from(receiving)
            .await
            .unwrap_err();
        assert_eq!(
            property(&error, "kind").unwrap().as_string().as_deref(),
            Some("not-connected")
        );
        assert!(control.media.call_offers.lock().unwrap().entries.is_empty());
        assert!(control.media.call_records.borrow().is_empty());
    }
    #[wasm_bindgen_test]
    async fn identical_call_ids_stay_bound_to_their_host() {
        let a_callbacks = callbacks();
        let b_callbacks = callbacks();
        let a = controller(a_callbacks.clone()).await;
        let b = controller(b_callbacks.clone()).await;
        let offer = js_sys::Function::new_no_args(
            r#"
          return {tag:'call',attrs:{from:'100002@lid',id:'OFFER',t:'1'},content:[{
            tag:'offer',attrs:{'call-id':'SAME-ID','call-creator':'100002@lid'},
            content:[{tag:'audio',attrs:{enc:'opus',rate:'16000'}}]
          }]}
        "#,
        )
        .call0(&JsValue::NULL)
        .unwrap();
        wasm_bindgen_futures::JsFuture::from(a.receive_node(offer.clone()))
            .await
            .unwrap();
        assert!(
            a.media
                .call_offers
                .lock()
                .unwrap()
                .entries
                .contains_key("SAME-ID")
        );
        assert!(b.media.call_offers.lock().unwrap().entries.is_empty());
        assert_eq!(number(&a_callbacks, "calls"), 1.0);
        assert_eq!(number(&b_callbacks, "calls"), 0.0);
        wasm_bindgen_futures::JsFuture::from(b.receive_node(offer))
            .await
            .unwrap();
        a.disconnect();
        assert!(a.media.call_offers.lock().unwrap().entries.is_empty());
        assert!(
            b.media
                .call_offers
                .lock()
                .unwrap()
                .entries
                .contains_key("SAME-ID")
        );
        assert_eq!(number(&b_callbacks, "calls"), 1.0);
        b.disconnect();
    }
    #[wasm_bindgen_test]
    async fn cancelled_key_preparation_keeps_input_until_callback_settles() {
        let callbacks = callbacks();
        js_sys::Function::new_no_args("this.entered=new Promise(r=>this.enteredResolve=r); this.prepareKeys=function(peer,data){this.plaintext=data;this.enteredResolve();return new Promise(r=>this.finishKeys=r)}")
            .call0(&callbacks).unwrap();
        let host = Arc::new(host(callbacks.clone()));
        let pending = host.clone();
        let (completed, completion) = futures::channel::oneshot::channel();
        host.runtime
            .spawn(Box::pin(async move {
                assert!(
                    pending
                        .prepare_keys(&"100002@lid".parse().unwrap(), &[1, 2, 3])
                        .await
                        .is_err()
                );
                completed.send(()).unwrap();
            }))
            .detach();
        wasm_bindgen_futures::JsFuture::from(
            property(&callbacks, "entered")
                .unwrap()
                .dyn_into::<js_sys::Promise>()
                .unwrap(),
        )
        .await
        .unwrap();
        host.cancel.close();
        completion.await.unwrap();
        assert_eq!(
            bytes(property(&callbacks, "plaintext").unwrap(), "plaintext").unwrap(),
            vec![1, 2, 3]
        );
        property(&callbacks, "finishKeys")
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap()
            .call0(&callbacks)
            .unwrap();
        for _ in 0..3 {
            wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::UNDEFINED))
                .await
                .unwrap();
        }
        assert_eq!(
            bytes(property(&callbacks, "plaintext").unwrap(), "plaintext").unwrap(),
            vec![0, 0, 0]
        );
    }

    #[wasm_bindgen_test]
    async fn shutdown_releases_detached_token_callback_and_ignores_late_completion() {
        let callbacks = callbacks();
        js_sys::Function::new_no_args(
            r#"
          this.entered = new Promise(resolve => { this.enteredResolve = resolve });
          this.issueToken = function() {
            this.enteredResolve();
            return new Promise(resolve => { this.resolveIssue = resolve });
          };
        "#,
        )
        .call0(&callbacks)
        .unwrap();
        let host = Arc::new(host(callbacks.clone()));
        let weak = Arc::downgrade(&host);
        let pending = host.clone();
        let (completed, completion) = futures::channel::oneshot::channel();
        host.runtime
            .spawn(Box::pin(async move {
                pending.issue_token(&"100002@lid".parse().unwrap()).await;
                drop(pending);
                completed.send(()).unwrap();
            }))
            .detach();
        let entered = property(&callbacks, "entered")
            .unwrap()
            .dyn_into::<js_sys::Promise>()
            .unwrap();
        wasm_bindgen_futures::JsFuture::from(entered).await.unwrap();
        host.live.store(false, std::sync::atomic::Ordering::Relaxed);
        host.cancel.close();
        drop(host);
        completion.await.unwrap();
        assert!(
            weak.upgrade().is_none(),
            "a pending host Promise must not retain the Rust controller"
        );
        property(&callbacks, "resolveIssue")
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap()
            .call0(&callbacks)
            .unwrap();
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::UNDEFINED))
            .await
            .unwrap();
        assert!(weak.upgrade().is_none());
    }
}
