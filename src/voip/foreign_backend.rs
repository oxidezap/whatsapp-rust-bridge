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
    /// Latest handle per call. A new generation for a call whose session
    /// already ended reuses it, so the wire identity stays stable and the
    /// generation check keeps late pushes for the old generation out.
    handles: Mutex<HashMap<String, u32>>,
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
                handles: Mutex::new(HashMap::new()),
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
    /// One request across, one checked response back.
    async fn request(
        &self,
        opcode: abi::Opcode,
        body: Vec<u8>,
    ) -> Result<abi::Frame, RequestError> {
        let req = abi::Frame::request(opcode, body);
        let raw = self
            .callbacks
            .send(&req.encode())
            .await
            .map_err(RequestError::Comms)?;
        let resp = abi::Frame::decode(&raw).map_err(RequestError::Framing)?;
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
    fn reserve(
        &self,
        key: &vc::MediaSessionKey,
        direction: vc::CallDirection,
    ) -> Arc<dyn vc::VoipMediaSession> {
        // A call whose session already ended keeps its handle: the new
        // generation replaces the old under the same wire identity. A call
        // that is still live mints fresh, so the replacement cannot steal
        // the live session's pushes.
        let reuse = if let (Ok(handles), Ok(sessions)) =
            (self.inner.handles.lock(), self.inner.sessions.lock())
        {
            handles
                .get(&key.call_id)
                .copied()
                .filter(|handle| sessions.get(handle).map(|s| s.is_closed()).unwrap_or(true))
        } else {
            None
        };
        let handle = match reuse {
            Some(handle) => handle,
            None => {
                let handle = self.inner.next_handle.fetch_add(1, Ordering::Relaxed);
                // `u32::MAX` sessions without a close is a leak, not a
                // wrap: fail the reservation rather than reuse handle 0.
                if handle == 0 {
                    self.inner
                        .next_handle
                        .store(FIRST_HANDLE, Ordering::Relaxed);
                    FIRST_HANDLE
                } else {
                    handle
                }
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
        if let Ok(mut handles) = self.inner.handles.lock() {
            handles.insert(key.call_id.clone(), handle);
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
        let params = conv::params_from_spec(&spec, &session.ctx_bits(&ctx, spec.enable_video))?;
        let mut guard = OpenGuard::new(session.clone(), self.inner.clone());

        let reserve = abi::ReserveRequest {
            session: session.session_id(),
            call_id: spec.key.call_id.clone(),
            direction: conv::map_direction(session.direction()).ok_or_else(|| {
                vc::MediaSetupError::Backend("call direction this bridge predates".to_owned())
            })?,
        };
        self.inner
            .request(abi::Opcode::Reserve, reserve.encode())
            .await
            .map_err(|e| e.into_setup("reserve"))?;

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

        session.attach(&spec.audio.format, ctx);

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
        self.session = None;
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let inner = self.inner.clone();
        let runtime = inner.runtime.clone();
        // Detached: the guard is gone by definition here, and dropping the
        // handle would abort the cancel before its first poll.
        runtime
            .spawn(Box::pin(async move {
                let cancel = abi::CancelOpenRequest {
                    session: session.session_id(),
                };
                match inner
                    .request(abi::Opcode::CancelOpen, cancel.encode())
                    .await
                {
                    Ok(_) => {}
                    Err(e) => log::warn!(
                        "voip plugin cancel-open failed: {}",
                        e.into_setup("cancel open")
                    ),
                }
            }))
            .detach();
    }
}
