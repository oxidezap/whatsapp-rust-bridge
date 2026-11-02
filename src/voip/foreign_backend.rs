//! The backend the registry reserves media sessions from when the host
//! supplied a media plugin.
//!
//! The lifecycle is the trait's: `reserve` hands back a session for a
//! generational key, `open` brings it operational over the wire, and every
//! later call lands on the session. The backend itself owns the handle
//! space, the session table the pushes route into, and the push entry point
//! it hands to JS.

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use async_trait::async_trait;
use voip_abi as abi;
use wasm_bindgen::prelude::*;
use whatsapp_rust::voip_control as vc;

use super::foreign_session::ForeignVoipSession;
use super::{BackendCommsError, OPEN_CEILING, VoipBackendCallbacks, abi as conv};

/// Next handle values start at 1; 0 is never a session.
const FIRST_HANDLE: u32 = 1;

/// The media backend behind `extensions.voipBackend`.
pub struct ForeignVoipBackend {
    inner: Arc<BackendInner>,
}

struct BackendInner {
    callbacks: VoipBackendCallbacks,
    agreed: u32,
    runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
    ceiling: std::time::Duration,
    next_handle: AtomicU32,
    sessions: Mutex<HashMap<u32, Arc<ForeignVoipSession>>>,
    keys: Mutex<HashMap<(String, u64), u32>>,
}

impl ForeignVoipBackend {
    /// Builds the backend over validated callbacks and a completed
    /// handshake. The agreed capabilities gate what the pumps may send.
    pub fn new(
        callbacks: VoipBackendCallbacks,
        agreed: u32,
        runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
    ) -> Self {
        ForeignVoipBackend::with_ceiling(callbacks, agreed, runtime, OPEN_CEILING)
    }

    /// Builds with an explicit open ceiling. Production passes
    /// [`OPEN_CEILING`](super::OPEN_CEILING); tests shrink it.
    #[allow(clippy::arc_with_non_send_sync)] // The core backend contract uses Arc on WASM too.
    pub(crate) fn with_ceiling(
        callbacks: VoipBackendCallbacks,
        agreed: u32,
        runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
        ceiling: std::time::Duration,
    ) -> Self {
        ForeignVoipBackend {
            inner: Arc::new(BackendInner {
                callbacks,
                agreed,
                runtime,
                ceiling,
                next_handle: AtomicU32::new(FIRST_HANDLE),
                sessions: Mutex::new(HashMap::new()),
                keys: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The inbound entry point registered on the plugin via
    /// `setPushHandler`. The closure holds the backend weakly: the JS
    /// object owns the closure, and a push after teardown is a no-op
    /// rather than a use-after-free.
    pub fn push_entry(&self) -> JsValue {
        let weak = Arc::downgrade(&self.inner);
        let closure = Closure::wrap(Box::new(move |data: js_sys::Uint8Array| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.route_push(&crate::js_bytes::to_vec(&data));
        }) as Box<dyn FnMut(js_sys::Uint8Array)>);
        closure.into_js_value()
    }

    /// Looks a session up for `open`, by the spec's generational key.
    fn session_for(&self, spec: &vc::MediaSessionSpec) -> Option<Arc<ForeignVoipSession>> {
        let keys = self.inner.keys.lock().ok()?;
        let handle = keys.get(&(spec.key.call_id.clone(), spec.key.generation))?;
        self.inner.sessions.lock().ok()?.get(handle).cloned()
    }
}

impl BackendInner {
    fn prune_closed(&self) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };
        sessions.retain(|_, session| !session.is_closed());
        if let Ok(mut keys) = self.keys.lock() {
            keys.retain(|_, handle| sessions.contains_key(handle));
        }
    }

    /// One request across, one checked response back.
    async fn request(
        &self,
        opcode: abi::Opcode,
        body: Vec<u8>,
    ) -> Result<abi::Frame, RequestError> {
        let req = abi::Frame::request(opcode, body);
        let encoded = abi::SecretBytes::new(req.encode());
        let raw = self
            .callbacks
            .send(encoded.as_bytes())
            .await
            .map_err(RequestError::Comms)?;
        let resp = abi::Frame::decode(&raw).map_err(RequestError::Framing)?;
        if resp.major != abi::ABI_MAJOR {
            return Err(RequestError::WrongResponse(
                "unsupported ABI major".to_owned(),
            ));
        }
        if resp.opcode != opcode {
            return Err(RequestError::WrongResponse(format!(
                "voip plugin answered {} to {}, want the echo",
                resp.opcode.name(),
                opcode.name()
            )));
        }
        if resp.is_error() {
            let body = abi::ErrorBody::decode(&resp.payload).map_err(|_| {
                RequestError::WrongResponse("voip plugin error body misformed".to_owned())
            })?;
            return Err(RequestError::Abi(body.code, body.detail));
        }
        if !resp.is_response() {
            return Err(RequestError::WrongResponse(format!(
                "voip plugin answered {} without the response flag",
                opcode.name()
            )));
        }
        Ok(resp)
    }

    /// Routes one engine-side push into its session. Pushes carry no
    /// response, so anything malformed is logged and dropped: there is no
    /// one to answer, and the call the frame belonged to is named inside.
    fn route_push(&self, bytes: &[u8]) {
        let frame = match abi::Frame::decode(bytes) {
            Ok(frame) => frame,
            Err(e) => {
                log::warn!("voip plugin push misframed, dropped: {e}");
                return;
            }
        };
        if frame.major != abi::ABI_MAJOR {
            log::warn!("voip plugin push with major {}, dropped", frame.major);
            return;
        }
        let session = match self.session_for_frame(&frame) {
            Some(session) => session,
            None => {
                log::warn!(
                    "voip plugin push for unknown session ({}), dropped",
                    frame.opcode.name()
                );
                return;
            }
        };
        session.deliver_push(frame);
        self.prune_closed();
    }

    /// Finds the session a push names, checking the generation before the
    /// session ever sees it. Stale pushes never touch a replacement.
    fn session_for_frame(&self, frame: &abi::Frame) -> Option<Arc<ForeignVoipSession>> {
        let payload = &frame.payload;
        let (handle, generation) = match frame.opcode {
            abi::Opcode::Open
            | abi::Opcode::Event
            | abi::Opcode::Stats
            | abi::Opcode::MediaEnded
            | abi::Opcode::PcmOut
            | abi::Opcode::EncodedAudioOut
            | abi::Opcode::VideoOut => {
                let mut r = abi::Reader::new(payload);
                let id = abi::SessionId {
                    handle: r.u32_le().ok()?,
                    generation: r.u64_le().ok()?,
                };
                (id.handle, id.generation)
            }
            _ => {
                log::warn!(
                    "voip plugin pushed {}, which flows core to voip; dropped",
                    frame.opcode.name()
                );
                return None;
            }
        };
        let sessions = self.sessions.lock().ok()?;
        let session = sessions.get(&handle)?.clone();
        if !session.is_current(generation) {
            log::warn!("stale voip plugin push for handle {handle}, dropped");
            return None;
        }
        Some(session)
    }
}

/// What one request across the callbacks can report. The mapping into the
/// setup grammar is at the call site: a broken link is `Connect`, a typed
/// plugin failure keeps its code, and anything misformed is setup.
enum RequestError {
    Comms(BackendCommsError),
    Framing(abi::DecodeError),
    WrongResponse(String),
    Abi(abi::AbiErrorCode, Option<String>),
}

impl RequestError {
    fn into_setup(self, what: &str) -> vc::MediaSetupError {
        match self {
            RequestError::Comms(e) => vc::MediaSetupError::Connect(format!(
                "voip plugin link failed during {what}: {}",
                e.0
            )),
            RequestError::Framing(e) => {
                vc::MediaSetupError::Backend(format!("voip plugin misframed {what}: {e}"))
            }
            RequestError::WrongResponse(detail) => {
                vc::MediaSetupError::Backend(format!("voip plugin misanswered {what}: {detail}"))
            }
            RequestError::Abi(code, detail) => conv::setup_error(code, detail.as_deref()),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl vc::VoipMediaBackend for ForeignVoipBackend {
    #[allow(clippy::arc_with_non_send_sync)] // The core session contract uses Arc on WASM too.
    fn reserve(
        &self,
        key: &vc::MediaSessionKey,
        direction: vc::CallDirection,
    ) -> Arc<dyn vc::VoipMediaSession> {
        self.inner.prune_closed();
        // Fresh nonzero identities prevent late pushes from targeting a replacement.
        let handle = loop {
            let candidate = self.inner.next_handle.fetch_add(1, Ordering::Relaxed);
            if candidate != 0 && !self.inner.sessions.lock().unwrap().contains_key(&candidate) {
                break candidate;
            }
        };
        let session = Arc::new(ForeignVoipSession::new(
            handle,
            key.clone(),
            direction,
            self.inner.callbacks.clone(),
            self.inner.runtime.clone(),
            self.inner.agreed,
        ));
        let weak = Arc::downgrade(&self.inner);
        session.set_on_closed(Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.prune_closed();
            }
        }));
        if let Ok(mut sessions) = self.inner.sessions.lock() {
            // Sessions the registry released stay reachable here; evict the
            // ended ones while allocating so the table cannot grow with
            // dead calls.
            sessions.retain(|_, s| !s.is_closed());
            sessions.insert(handle, session.clone());
        }
        if let Ok(mut keys) = self.inner.keys.lock() {
            keys.insert((key.call_id.clone(), key.generation), handle);
        }
        session
    }

    async fn open(
        &self,
        spec: vc::MediaSessionSpec,
        ctx: vc::MediaOpenContext,
    ) -> Result<(), vc::MediaSetupError> {
        let Some(session) = self.session_for(&spec) else {
            return Err(vc::MediaSetupError::Backend(
                "open for a session that was never reserved".to_owned(),
            ));
        };
        let capability = match &ctx.audio {
            vc::MediaAudioPorts::Pcm { .. } => abi::Capabilities::PCM,
            vc::MediaAudioPorts::Encoded { .. } => abi::Capabilities::ENCODED_AUDIO,
            _ => 0,
        };
        if capability == 0
            || self.inner.agreed & capability == 0
            || (spec.enable_video && self.inner.agreed & abi::Capabilities::VIDEO == 0)
        {
            return Err(conv::setup_error(
                abi::AbiErrorCode::NotSupported,
                Some("media mode not negotiated"),
            ));
        }
        // Local conversion must not issue cancellation for a request never sent.
        let params = conv::params_from_spec(&spec, &session.ctx_bits(&ctx, spec.enable_video))?;
        let direction = conv::map_direction(session.direction()).ok_or_else(|| {
            vc::MediaSetupError::Backend("call direction this bridge predates".to_owned())
        })?;
        whatsapp_rust::wacore::runtime::timeout(
            &*self.inner.runtime,
            self.inner.ceiling,
            session.begin_open_attempt(),
        )
        .await
        .map_err(|_| {
            vc::MediaSetupError::Connect(
                "voip plugin did not finish prior cancellation within the dial ceiling".to_owned(),
            )
        })??;
        let mut guard = OpenGuard::new(session.clone(), self.inner.clone());
        let mut ctx = ctx;
        session.prepare_teardown(ctx.video_teardown.take());
        session.prepare_sinks(&spec.audio.format, &ctx);

        if !session.engine_reserved() {
            let reserve = abi::ReserveRequest {
                session: session.session_id(),
                call_id: spec.key.call_id.clone(),
                direction,
            };
            whatsapp_rust::wacore::runtime::timeout(
                &*self.inner.runtime,
                self.inner.ceiling,
                self.inner.request(abi::Opcode::Reserve, reserve.encode()),
            )
            .await
            .map_err(|_| {
                vc::MediaSetupError::Connect(
                    "voip plugin did not acknowledge reservation within the dial ceiling"
                        .to_owned(),
                )
            })?
            .map_err(|e| e.into_setup("reserve"))?;
            session.mark_engine_reserved();
        }

        let begin = abi::BeginOpenRequest {
            session: session.session_id(),
            params,
        };
        whatsapp_rust::wacore::runtime::timeout(
            &*self.inner.runtime,
            self.inner.ceiling,
            self.inner.request(abi::Opcode::BeginOpen, begin.encode()),
        )
        .await
        .map_err(|_| {
            vc::MediaSetupError::Connect(
                "voip plugin did not acknowledge open within the dial ceiling".to_owned(),
            )
        })?
        .map_err(|e| e.into_setup("begin open"))?;

        // The relay dial happens engine-side between the ack and OPEN; the
        // same ceiling that bounds the resident dial bounds this wait, and
        // dropping the future (hangup, terminate, disconnect, replacement)
        // drops the guard, which cancels the in-flight open.
        whatsapp_rust::wacore::runtime::timeout(
            &*self.inner.runtime,
            self.inner.ceiling,
            session.wait_opened(),
        )
        .await
        .map_err(|_| {
            vc::MediaSetupError::Connect(
                "voip plugin media did not come up within the dial ceiling".to_owned(),
            )
        })??;
        session.attach(&spec.audio.format, ctx, spec.enable_media);
        guard.disarm();
        Ok(())
    }
}

/// Cancels the in-flight open unless the open already resolved. Lives on
/// the `open` future's stack, so dropping the future — hangup, terminate,
/// disconnect, replacement — sends `CANCEL_OPEN` without any caller action.
struct OpenGuard {
    session: Option<Arc<ForeignVoipSession>>,
    inner: Arc<BackendInner>,
}

impl OpenGuard {
    fn new(session: Arc<ForeignVoipSession>, inner: Arc<BackendInner>) -> Self {
        OpenGuard {
            session: Some(session),
            inner,
        }
    }

    fn disarm(&mut self) {
        if let Some(session) = self.session.take() {
            session.finish_open_attempt();
        }
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let cancelled = session.track_cancel();
        session.finish_open_attempt();
        let inner = self.inner.clone();
        let runtime = inner.runtime.clone();
        // Detached: the guard is gone by definition here, and dropping the
        // handle would abort the cancel before its first poll.
        runtime
            .spawn(Box::pin(async move {
                let cancel = abi::CancelOpenRequest {
                    session: session.session_id(),
                };
                match whatsapp_rust::wacore::runtime::timeout(
                    &*inner.runtime,
                    inner.ceiling,
                    inner.request(abi::Opcode::CancelOpen, cancel.encode()),
                )
                .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => log::warn!(
                        "voip plugin cancel-open failed: {}",
                        e.into_setup("cancel open")
                    ),
                    Err(_) => log::warn!("voip plugin cancel-open timed out"),
                }
                let _ = cancelled.send(());
            }))
            .detach();
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    use vc::VoipMediaBackend;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn allocation_wrap_preserves_live_nonzero_handles() {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(
            &object,
            &"sendFrame".into(),
            &js_sys::Function::new_with_args("frame", "return Promise.resolve(frame)"),
        )
        .unwrap();
        js_sys::Reflect::set(
            &object,
            &"setPushHandler".into(),
            &js_sys::Function::new_no_args(""),
        )
        .unwrap();
        let callbacks = VoipBackendCallbacks::from_js(&object.into()).unwrap();
        let backend =
            ForeignVoipBackend::new(callbacks, u32::MAX, Arc::new(crate::runtime::WasmRuntime));
        let key = |generation| {
            vc::MediaSessionKey::builder()
                .call_id("wrap".into())
                .generation(generation)
                .build()
        };
        let original = backend.reserve(&key(1), vc::CallDirection::Outgoing);
        backend.inner.next_handle.store(u32::MAX, Ordering::Relaxed);
        let maximum = backend.reserve(&key(2), vc::CallDirection::Outgoing);
        let wrapped = backend.reserve(&key(3), vc::CallDirection::Outgoing);
        let sessions = backend.inner.sessions.lock().unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(sessions.contains_key(&1));
        assert!(sessions.contains_key(&u32::MAX));
        assert!(sessions.contains_key(&2));
        assert!(!sessions.contains_key(&0));
        drop(sessions);
        assert_eq!(
            backend.inner.keys.lock().unwrap().get(&("wrap".into(), 1)),
            Some(&1)
        );
        drop((original, maximum, wrapped));
    }
}
