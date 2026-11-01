//! The engine side of the OZVP wire: `voip.wasm`.
//!
//! This crate speaks the versioned `voip-abi` binary protocol with the
//! bridge's core module and drives `wacore`'s call engine. It names no
//! WhatsApp client type: the only core it knows is `wacore` (engine, MLOW
//! codec, wasm getrandom), never `whatsapp-rust` (the client). The gate is
//! structural — there is no `whatsapp-rust` entry in this crate's
//! dependency graph, which CI asserts — not a promise in a comment.
//!
//! The surface mirrors what `src/voip/foreign_session.rs` and
//! `src/voip/abi.rs` do on the core side: a `HELLO` handshake guarded by
//! [`voip_abi::negotiate`], a session table with generational identity, and
//! one frame in / one frame out per request. Pushes (`OPEN`, `EVENT`,
//! `STATS`, `MEDIA_ENDED`, the `_OUT` media frames) leave through the
//! handler installed by [`set_push_handler`].
//!
//! `BeginOpen` builds a real `CallEngine` from the open params (see
//! [`spec`]), dials the relay through the plugin transport installed by
//! [`relay::set_relay_transport`], and runs `run_call` on the [`runtime`].
//! The engine's outputs fan back out as pushes in [`call`]: decoded PCM,
//! encoded packets, video units, events, stats, and the terminal close.

#[cfg(target_arch = "wasm32")]
use wacore as clock_core;
#[cfg(target_arch = "wasm32")]
#[path = "../../../src/js_time.rs"]
mod js_time;

mod call;
mod mlow;
mod relay;
pub use mlow::MlowAudioDecoder;
mod runtime;
mod spec;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::call::{EncodedAudioOut, LiveCall, event_push, stats_push};
use voip_abi::{
    ABI_MAJOR, ABI_MINOR, AbiErrorCode, BeginOpenRequest, CancelOpenRequest, CancelOpenResponse,
    CancelOutcome, Capabilities, CommandRequest, Frame, GroupFitsRequest, GroupFitsResponse,
    HelloRequest, HelloResponse, MediaFrame, Opcode, ReserveRequest, Role, SessionId, SessionTable,
    StatsData, StatsRequest,
};
use wasm_bindgen::prelude::*;

/// Capability bits this side offers: everything the engine carries.
/// `RELAY_RECONNECT` stays out: the relay lives on the plugin's transport,
/// which redials out of band, so this side must not promise reconnect help
/// it never gives. Mirrors `ABI_CAPABILITIES` on the core side.
const ENGINE_CAPABILITIES: u32 = Capabilities::PCM
    | Capabilities::VIDEO
    | Capabilities::ENCODED_AUDIO
    | Capabilities::GROUP_CALLS
    | Capabilities::CALL_LINKS
    | Capabilities::STATS_PUSH;

/// One engine-side session behind a reserved handle.
struct EngineSession {
    /// The generational identity the core reserved.
    id: SessionId,
    /// The call this handle stands for. Retained so a log can name the
    /// call without re-reading the table; the routing itself keys on the
    /// handle, never on this.
    #[allow(dead_code)]
    call_id: String,
    /// Whether `BeginOpen` has been acknowledged for this session.
    opening: bool,
    open_attempt: u64,
    /// The running call, once `BeginOpen` built its engine and the `OPEN`
    /// push went out. `None` while setup is still in flight.
    live_call: Option<LiveCall>,
}

/// The engine module's mutable state, behind one lock.
struct State {
    /// The minor both sides agreed to speak, once the handshake ran.
    agreed_minor: Option<u8>,
    agreed_capabilities: u32,
    /// Handle-to-reservation registry with generation checks.
    sessions: SessionTable,
    /// The live sessions behind the reserved handles.
    live: HashMap<u32, EngineSession>,
}

impl State {
    fn new() -> Self {
        State {
            agreed_minor: None,
            agreed_capabilities: 0,
            sessions: SessionTable::new(),
            live: HashMap::new(),
        }
    }
}

fn state() -> &'static Mutex<State> {
    thread_local! {
        static STATE: &'static Mutex<State> = Box::leak(Box::new(Mutex::new(State::new())));
    }
    STATE.with(|state| *state)
}

/// The push entry point the core registered. Called for `OPEN`, `EVENT`,
/// `STATS` pushes, `MEDIA_ENDED`, and the `_OUT` media frames. Absent until
/// [`set_push_handler`] runs: pushes before install are dropped, never
/// queued, because a session that ends before its handler exists has no
/// consumer to owe them to.
static PUSH: OnceLock<Mutex<Option<js_sys::Function>>> = OnceLock::new();

fn push_slot() -> &'static Mutex<Option<js_sys::Function>> {
    PUSH.get_or_init(|| Mutex::new(None))
}

/// Emits one push frame to the core. A throwing or missing handler drops
/// the frame: pushes are notifications, and the core re-reads state on its
/// next request rather than depending on any single one.
fn push(payload: Vec<u8>) {
    let handler = push_slot().lock().map(|g| g.clone()).unwrap_or(None);
    let Some(handler) = handler else { return };
    let bytes = js_sys::Uint8Array::from(payload.as_slice());
    let _ = handler.call1(&JsValue::UNDEFINED, &bytes.into());
}

/// Rewrite an RFC Opus CELT packet into WhatsApp's MLOW escape.
#[wasm_bindgen(js_name = packetizeOpusForMlow)]
pub fn packetize_opus_for_mlow(data: &[u8]) -> Result<Vec<u8>, JsValue> {
    let mut packet = data.to_vec();
    wacore::voip::packetize_opus_for_mlow(&mut packet)
        .map_err(|e| JsValue::from_str(&format!("data: {e}")))?;
    Ok(packet)
}

/// Restore the original RFC Opus TOC before handing the packet to a decoder.
#[wasm_bindgen(js_name = depacketizeOpusFromMlow)]
pub fn depacketize_opus_from_mlow(data: &[u8]) -> Result<Vec<u8>, JsValue> {
    let mut packet = data.to_vec();
    wacore::voip::depacketize_opus_from_mlow(&mut packet)
        .map_err(|e| JsValue::from_str(&format!("data: {e}")))?;
    Ok(packet)
}

/// The handshake: the core probes with `HELLO`, and the version gate runs
/// before anything else exists. A major mismatch fails here, with no
/// session and no engine. Returns the responder's capabilities.
#[wasm_bindgen]
pub fn init(abi_major: u32, abi_minor: u32) -> Result<js_sys::Uint8Array, JsValue> {
    let peer_major =
        u8::try_from(abi_major).map_err(|_| JsValue::from_str("voip ABI major out of range"))?;
    let peer_minor =
        u8::try_from(abi_minor).map_err(|_| JsValue::from_str("voip ABI minor out of range"))?;
    let agreed = voip_abi::negotiate(peer_major, peer_minor, ABI_MINOR)
        .map_err(|e| JsValue::from_str(&format!("voip ABI handshake refused: {e}")))?;
    // Each WASM instance owns independent wacore clock statics. Install before
    // any session can create playout, pacing, or protocol deadlines.
    #[cfg(target_arch = "wasm32")]
    js_time::init_time_provider();
    if let Ok(mut st) = state().lock() {
        st.agreed_minor = Some(agreed.minor);
        st.agreed_capabilities =
            voip_abi::capabilities_for_minor(ENGINE_CAPABILITIES, agreed.minor);
    }
    Ok(js_sys::Uint8Array::from(
        HelloResponse {
            capabilities: voip_abi::capabilities_for_minor(ENGINE_CAPABILITIES, agreed.minor),
        }
        .encode()
        .as_slice(),
    ))
}

/// Installs the core's inbound entry point. Called once, at install; the
/// engine side calls it for `OPEN`, `EVENT`, `STATS` pushes, `MEDIA_ENDED`,
/// and the `_OUT` media frames.
#[wasm_bindgen]
pub fn set_push_handler(handler: js_sys::Function) {
    if let Ok(mut slot) = push_slot().lock() {
        *slot = Some(handler);
    }
}

/// One request frame in, one response frame out. The core never sends a
/// notification opcode here; pushes leave through the installed handler.
/// Media `_In` frames are acknowledged (the stub engine consumes them);
/// `_Out` frames never arrive here — they leave as pushes.
#[wasm_bindgen]
pub fn send_frame(frame: Vec<u8>) -> Result<js_sys::Uint8Array, JsValue> {
    let frame = voip_abi::SecretBytes::new(frame);
    let req = Frame::decode(frame.as_bytes())
        .map_err(|e| JsValue::from_str(&format!("voip frame misframed: {e}")))?;
    let resp = route(&req);
    Ok(js_sys::Uint8Array::from(resp.encode().as_slice()))
}

/// Routes one decoded request to its handler. Every arm returns the single
/// response frame the core awaits; notifications to the core leave through
/// [`push`], never here.
fn route(req: &Frame) -> Frame {
    match req.opcode {
        Opcode::Hello => on_hello(req),
        Opcode::Reserve => on_reserve(req),
        Opcode::BeginOpen => on_begin_open(req),
        Opcode::CancelOpen => on_cancel_open(req),
        Opcode::Command => on_command(req),
        Opcode::GroupFits => on_group_fits(req),
        Opcode::Close => on_close(req),
        Opcode::Stats => on_stats(req),
        Opcode::PcmIn | Opcode::EncodedAudioIn | Opcode::VideoIn => on_media_in(req),
        // The core never sends these engine-side pushes as requests; a
        // frame carrying one is a misdirected push, not a request.
        Opcode::Open
        | Opcode::Event
        | Opcode::MediaEnded
        | Opcode::PcmOut
        | Opcode::EncodedAudioOut
        | Opcode::VideoOut => Frame::fail(
            req,
            AbiErrorCode::BadPayload,
            Some("push opcode sent as request"),
        ),
    }
}

/// Answers the core's `HELLO` probe with this side's capabilities. The
/// handshake gate itself lives in [`init`]; this path handles a `HELLO`
/// that arrives as a framed request instead.
fn on_hello(req: &Frame) -> Frame {
    let Ok(hello) = HelloRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("HELLO misformed"));
    };
    if hello.role != Role::Core {
        return Frame::fail(
            req,
            AbiErrorCode::BadPayload,
            Some("HELLO role is not core"),
        );
    }
    if req.major != ABI_MAJOR {
        return Frame::fail(
            req,
            AbiErrorCode::BadVersion,
            Some("HELLO speaks another major"),
        );
    }
    let minor = req.minor.min(ABI_MINOR);
    let capabilities =
        voip_abi::capabilities_for_minor(ENGINE_CAPABILITIES & hello.capabilities, minor);
    if let Ok(mut st) = state().lock() {
        st.agreed_minor = Some(minor);
        st.agreed_capabilities = capabilities;
    }
    let mut response = Frame::respond(req, HelloResponse { capabilities }.encode());
    response.minor = minor;
    response
}

/// Binds a handle to `(call_id, generation)` before any media message
/// names it. Replacement is `CLOSE` then `RESERVE`, never an overwrite.
fn on_reserve(req: &Frame) -> Frame {
    let Ok(reserve) = ReserveRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("RESERVE misformed"));
    };
    let Ok(mut st) = state().lock() else {
        return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged"));
    };
    if let Err(e) = st.sessions.reserve(reserve.session, &reserve.call_id) {
        let (code, detail) = match e {
            voip_abi::SessionError::HandleInUse { .. } => (
                AbiErrorCode::Internal,
                "session handle still reserved; close it first",
            ),
            voip_abi::SessionError::Unknown | voip_abi::SessionError::Stale { .. } => (
                AbiErrorCode::Internal,
                "session table refused the reservation",
            ),
        };
        return Frame::fail(req, code, Some(detail));
    }
    st.live.insert(
        reserve.session.handle,
        EngineSession {
            id: reserve.session,
            call_id: reserve.call_id,
            opening: false,
            open_attempt: 0,
            live_call: None,
        },
    );
    Frame::respond(req, Vec::new())
}

/// Starts asynchronous setup with the open params. The ack means setup
/// started, not finished; completion arrives as an `OPEN` push. The engine
/// builds on a spawned task: the params become a `MediaSessionSpec`, the
/// spec becomes a `CallEngine` plus a relay dial, and `run_call` drives the
/// call from there. The `OPEN` push goes out only after the task is up; a
/// refusal fails the ack inline with the field that was wrong.
fn on_begin_open(req: &Frame) -> Frame {
    let Ok(begin) = BeginOpenRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("BEGIN_OPEN misformed"));
    };
    let (call_id, params, attempt) = match state().lock() {
        Ok(mut st) => {
            if begin.params.enable_video != 0 && st.agreed_capabilities & Capabilities::VIDEO == 0 {
                return Frame::fail(
                    req,
                    AbiErrorCode::NotSupported,
                    Some("video metadata was not negotiated"),
                );
            }
            if let Err(e) = st.sessions.validate(begin.session) {
                return session_error(req, e);
            }
            let Some(session) = st.live.get_mut(&begin.session.handle) else {
                return Frame::fail(
                    req,
                    AbiErrorCode::UnknownSession,
                    Some("session is not reserved"),
                );
            };
            if session.id.generation != begin.session.generation {
                return Frame::fail(
                    req,
                    AbiErrorCode::StaleGeneration,
                    Some("session generation is not current"),
                );
            }
            if session.opening || session.live_call.is_some() {
                return Frame::fail(req, AbiErrorCode::Busy, Some("open already in flight"));
            }
            session.opening = true;
            session.open_attempt += 1;
            (session.call_id.clone(), begin.params, session.open_attempt)
        }
        Err(_) => {
            return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged"));
        }
    };
    let ack = Frame::respond(req, Vec::new());
    let session = begin.session;
    wasm_bindgen_futures::spawn_local(async move {
        open_async(session, call_id, params, attempt).await;
    });
    ack
}

/// Builds the engine and starts the drive loop off the request path: `dial`
/// awaits the plugin's JS transport, so it cannot run inside `send_frame`.
/// On success the `OPEN` push goes out; on refusal the session is released
/// back to reserved so the core can retry or close, and the failure rides
/// an `EVENT(MEDIA_SETUP_FAILED)` the core's session raises.
async fn open_async(
    session: SessionId,
    call_id: String,
    params: voip_abi::OpenParams,
    attempt: u64,
) {
    use crate::call::PushSinks;
    let live = LiveCall::open(
        session,
        call_id,
        params,
        PushSinks {
            event: Arc::new(|id, event| push(event_push(id, event))),
            stats: Arc::new(|id, stats| push(stats_push(id, stats))),
            pcm: Arc::new(push_pcm),
            encoded: Arc::new(push_encoded),
            video: Arc::new(push_video),
            ended: Arc::new(push_ended),
        },
    )
    .await;
    let Ok(mut st) = state().lock() else { return };
    let Some(entry) = st.live.get_mut(&session.handle) else {
        return;
    };
    if entry.id.generation != session.generation || !entry.opening || entry.open_attempt != attempt
    {
        return;
    }
    match live {
        Ok(call) => {
            entry.opening = false;
            entry.live_call = Some(call);
            drop(st);
            push_open(session);
        }
        Err(error) => {
            entry.opening = false;
            drop(st);
            push_setup_failed(session, &error);
        }
    }
}

/// Emits the `EVENT(MEDIA_SETUP_FAILED)` for an open that never became a
/// call, so the core's `wait_opened` fails instead of hanging to its
/// ceiling. The session stays reserved: the failure named the params, not
/// the handle, so the core may retry or close. The wire code rides beside
/// the detail so the core's setup grammar (`Connect` vs setup) survives
/// the push.
fn push_setup_failed(session: SessionId, error: &crate::call::OpenError) {
    push(event_push(
        session,
        voip_abi::AbiEvent::MediaSetupFailed(format!(
            "{}: {}",
            error.code().name(),
            error.detail()
        )),
    ));
}

fn push_pcm(session: SessionId, seq: u32, data: Vec<u8>) {
    push(
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: Opcode::PcmOut,
            flags: 0,
            payload: MediaFrame { session, seq, data }.encode(),
        }
        .encode(),
    );
}

fn push_encoded(session: SessionId, seq: u32, frame: voip_abi::EncodedFrameDto) {
    push(
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: Opcode::EncodedAudioOut,
            flags: 0,
            payload: EncodedAudioOut {
                session,
                seq,
                frame,
            }
            .encode(),
        }
        .encode(),
    );
}

fn push_video(session: SessionId, seq: u32, frame: voip_abi::VideoFrameDto) {
    push(
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: Opcode::VideoOut,
            flags: 0,
            payload: voip_abi::VideoOut {
                session,
                seq,
                frame,
            }
            .encode(),
        }
        .encode(),
    );
}

fn push_ended(session: SessionId, reason: voip_abi::CloseReason, detail: Option<String>) {
    push(
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: Opcode::MediaEnded,
            flags: 0,
            payload: voip_abi::CloseRequest {
                session,
                reason,
                detail,
            }
            .encode(),
        }
        .encode(),
    );
}

/// Emits the `OPEN` notification for a session whose setup completed.
fn push_open(session: SessionId) {
    let body = voip_abi::OpenNotification { session }.encode();
    push(
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: Opcode::Open,
            flags: 0,
            payload: body,
        }
        .encode(),
    );
}

/// Aborts an in-flight `BeginOpen`. The ack carries the outcome: an
/// opening session reports `Aborted` and releases back to reserved, an
/// open session reports `AlreadyOpen`, anything else `Unknown`.
fn on_cancel_open(req: &Frame) -> Frame {
    let Ok(cancel) = CancelOpenRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("CANCEL_OPEN misformed"));
    };
    let mut st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(cancel.session) {
        return session_error(req, e);
    }
    let outcome = match st.live.get_mut(&cancel.session.handle) {
        Some(session) if session.id.generation == cancel.session.generation => {
            if session.opening {
                // The spawned open still runs, but its completion finds no
                // opening session and drops the call it built: the abort is
                // what the core was promised, even if the task outlives it.
                session.opening = false;
                session.open_attempt += 1;
                CancelOutcome::Aborted
            } else if session.live_call.is_some() {
                CancelOutcome::AlreadyOpen
            } else {
                CancelOutcome::Unknown
            }
        }
        _ => CancelOutcome::Unknown,
    };
    Frame::respond(req, CancelOpenResponse { outcome }.encode())
}

/// Applies one control intent to the live call. A well-formed command for
/// a session with no running engine is `Busy`, not an acceptance: the
/// stub's blanket ack is gone with the engine wiring.
fn on_command(req: &Frame) -> Frame {
    let Ok(cmd) = CommandRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("COMMAND misformed"));
    };
    let mut st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if matches!(
        cmd.command,
        voip_abi::MediaCommand::VideoEnable | voip_abi::MediaCommand::VideoEnableAwaitingAccept
    ) && st.agreed_capabilities & Capabilities::VIDEO == 0
    {
        return Frame::fail(
            req,
            AbiErrorCode::NotSupported,
            Some("video metadata was not negotiated"),
        );
    }
    if let Err(e) = st.sessions.validate(cmd.session) {
        return session_error(req, e);
    }
    let Some(session) = st.live.get_mut(&cmd.session.handle) else {
        return Frame::fail(
            req,
            AbiErrorCode::UnknownSession,
            Some("session is not reserved"),
        );
    };
    if session.id.generation != cmd.session.generation {
        return Frame::fail(
            req,
            AbiErrorCode::StaleGeneration,
            Some("session generation is not current"),
        );
    }
    let Some(call) = session.live_call.as_mut() else {
        return Frame::fail(req, AbiErrorCode::Busy, Some("call is not open"));
    };
    if call.command(cmd.command) {
        Frame::respond(req, Vec::new())
    } else {
        Frame::fail(req, AbiErrorCode::Busy, Some("command refused"))
    }
}

/// Admission probe for a group roster. A live group plane answers from its
/// own fit; a 1:1 call fits no roster. A session with no running engine is
/// `Busy`, not an acceptance: the stub's zeroed answer is gone with the
/// engine wiring.
fn on_group_fits(req: &Frame) -> Frame {
    let Ok(fits) = GroupFitsRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("GROUP_FITS misformed"));
    };
    let st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(fits.session) {
        return session_error(req, e);
    }
    let Some(session) = st.live.get(&fits.session.handle) else {
        return Frame::fail(
            req,
            AbiErrorCode::UnknownSession,
            Some("session is not reserved"),
        );
    };
    if session.id.generation != fits.session.generation {
        return Frame::fail(
            req,
            AbiErrorCode::StaleGeneration,
            Some("session generation is not current"),
        );
    }
    let Some(call) = session.live_call.as_ref() else {
        return Frame::fail(req, AbiErrorCode::Busy, Some("call is not open"));
    };
    let (fits, limit) = call.group_fits(&fits.update);
    Frame::respond(req, GroupFitsResponse { fits, limit }.encode())
}

/// Releases a session. Closing drops the live call with it: the `LiveCall`
/// leaves the table and its drive task aborts, which drops the transport
/// and closes the relay channel. Closing is idempotent: dropping half-open
/// state twice is normal on the teardown path.
fn on_close(req: &Frame) -> Frame {
    let Ok(close) = voip_abi::CloseRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("CLOSE misformed"));
    };
    let mut st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(close.session) {
        // Closing what is already gone is a no-op, not an error — except
        // a stale generation, which names a real replacement that must
        // not be touched.
        match e {
            voip_abi::SessionError::Unknown => return Frame::respond(req, Vec::new()),
            voip_abi::SessionError::Stale { .. } => return session_error(req, e),
            voip_abi::SessionError::HandleInUse { .. } => {
                return Frame::fail(
                    req,
                    AbiErrorCode::Internal,
                    Some("session table is confused"),
                );
            }
        }
    }
    let session = st.live.remove(&close.session.handle);
    st.sessions.remove(close.session.handle);
    drop(st);
    if let Some(session) = session
        && let Some(call) = session.live_call
    {
        call.close();
    }
    Frame::respond(req, Vec::new())
}

/// Serves a stats poll for one session from the live call's published
/// counters. A session with no running engine answers zeroes — the shape
/// the core caches, with no counters invented.
fn on_stats(req: &Frame) -> Frame {
    let Ok(poll) = StatsRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("STATS misformed"));
    };
    let st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(poll.session) {
        return session_error(req, e);
    }
    let stats = st
        .live
        .get(&poll.session.handle)
        .filter(|session| session.id.generation == poll.session.generation)
        .and_then(|session| session.live_call.as_ref())
        .map(LiveCall::stats)
        .unwrap_or_else(zero_stats);
    Frame::respond(req, stats.encode())
}

/// Consumes one inbound media frame into the live call's mailboxes. The
/// bytes decoded and the generation matched; a session with no running
/// engine still acks, because media may overtake the `OPEN` push. The ack
/// is the whole of the behavior.
fn on_media_in(req: &Frame) -> Frame {
    let opcode = req.opcode;
    match opcode {
        Opcode::PcmIn | Opcode::EncodedAudioIn => on_audio_in(req),
        Opcode::VideoIn => on_video_in(req),
        _ => Frame::fail(req, AbiErrorCode::BadPayload, Some("media frame misformed")),
    }
}

fn on_audio_in(req: &Frame) -> Frame {
    let Ok(frame) = MediaFrame::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("media frame misformed"));
    };
    let st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(frame.session) {
        return session_error(req, e);
    }
    let Some(session) = st.live.get(&frame.session.handle) else {
        return Frame::fail(
            req,
            AbiErrorCode::UnknownSession,
            Some("session is not reserved"),
        );
    };
    if session.id.generation != frame.session.generation {
        return Frame::fail(
            req,
            AbiErrorCode::StaleGeneration,
            Some("session generation is not current"),
        );
    }
    let Some(call) = session.live_call.as_ref() else {
        return Frame::fail(req, AbiErrorCode::Busy, Some("media is not open"));
    };
    {
        match req.opcode {
            Opcode::PcmIn => call.pcm_in(&frame),
            _ => call.encoded_in(&frame.data),
        }
    }
    Frame::respond(req, Vec::new())
}

fn on_video_in(req: &Frame) -> Frame {
    let Ok(frame) = voip_abi::VideoIn::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("media frame misformed"));
    };
    let st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if st.agreed_capabilities & Capabilities::VIDEO == 0 {
        return Frame::fail(
            req,
            AbiErrorCode::NotSupported,
            Some("video metadata was not negotiated"),
        );
    }
    if let Err(e) = st.sessions.validate(frame.session) {
        return session_error(req, e);
    }
    let Some(session) = st.live.get(&frame.session.handle) else {
        return Frame::fail(
            req,
            AbiErrorCode::UnknownSession,
            Some("session is not reserved"),
        );
    };
    if session.id.generation != frame.session.generation {
        return Frame::fail(
            req,
            AbiErrorCode::StaleGeneration,
            Some("session generation is not current"),
        );
    }
    let Some(call) = session.live_call.as_ref() else {
        return Frame::fail(req, AbiErrorCode::Busy, Some("media is not open"));
    };
    call.video_in(&frame.input);
    Frame::respond(req, Vec::new())
}

/// Zero counters: the stub engine has counted nothing. The shape the core
/// caches, with no counters invented.
fn zero_stats() -> StatsData {
    StatsData::default()
}

/// Maps a session-table refusal onto the wire grammar: unknown handles
/// and stale generations both fail, and only an exact generation passes.
fn session_error(req: &Frame, e: voip_abi::SessionError) -> Frame {
    match e {
        voip_abi::SessionError::Unknown => Frame::fail(
            req,
            AbiErrorCode::UnknownSession,
            Some("session is not reserved"),
        ),
        voip_abi::SessionError::Stale { .. } => Frame::fail(
            req,
            AbiErrorCode::StaleGeneration,
            Some("session generation is not current"),
        ),
        voip_abi::SessionError::HandleInUse { .. } => Frame::fail(
            req,
            AbiErrorCode::Internal,
            Some("session table is confused"),
        ),
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod lifecycle_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test(async)]
    async fn init_installs_instance_local_wall_and_monotonic_clocks() {
        init(1, 1).unwrap();
        let wall = wacore::time::now_millis();
        let start = wacore::time::Instant::now();
        assert!(
            wall > 1_700_000_000_000,
            "wall clock is not the epoch fallback"
        );
        runtime::set_timeout_future(25).await;
        assert!(wacore::time::now_millis() > wall);
        assert!(start.elapsed() >= std::time::Duration::from_millis(15));
        assert!(
            wacore::time::Instant::now()
                .saturating_duration_since(start + std::time::Duration::from_millis(10))
                > std::time::Duration::ZERO,
            "a real monotonic deadline expires"
        );
        // Repeated negotiation preserves the existing instance clock.
        init(1, 1).unwrap();
        assert!(start.elapsed() >= std::time::Duration::from_millis(15));
    }

    #[wasm_bindgen_test(async)]
    async fn a_cancelled_open_closes_its_late_relay_without_installing_a_call() {
        *state().lock().unwrap() = State::new();
        let (started_tx, started_rx) = futures::channel::oneshot::channel();
        let started = Rc::new(RefCell::new(Some(started_tx)));
        let resolver = Rc::new(RefCell::new(None));
        let resolve_slot = resolver.clone();
        let connect = Closure::wrap(Box::new(move |_: JsValue, _: JsValue| {
            let promise = js_sys::Promise::new(&mut |resolve, _| {
                *resolve_slot.borrow_mut() = Some(resolve);
            });
            if let Some(tx) = started.borrow_mut().take() {
                let _ = tx.send(());
            }
            promise
        })
            as Box<dyn FnMut(JsValue, JsValue) -> js_sys::Promise>);
        let transport = js_sys::Object::new();
        js_sys::Reflect::set(&transport, &"connect".into(), &connect.into_js_value()).unwrap();
        relay::set_relay_transport(transport.into()).unwrap();

        let id = SessionId {
            handle: 9001,
            generation: 47,
        };
        let reserve = Frame::request(
            Opcode::Reserve,
            ReserveRequest {
                session: id,
                call_id: "cancel-late-relay".to_owned(),
                direction: voip_abi::Direction::Incoming,
            }
            .encode(),
        );
        assert!(!on_reserve(&reserve).is_error());
        let begin = Frame::request(
            Opcode::BeginOpen,
            BeginOpenRequest {
                session: id,
                params: call::tests::open_params(),
            }
            .encode(),
        );
        assert!(!on_begin_open(&begin).is_error());
        started_rx.await.expect("setup reached relay connect");
        let cancel = Frame::request(
            Opcode::CancelOpen,
            CancelOpenRequest { session: id }.encode(),
        );
        let cancelled = on_cancel_open(&cancel);
        assert_eq!(
            CancelOpenResponse::decode(&cancelled.payload)
                .unwrap()
                .outcome,
            CancelOutcome::Aborted
        );

        let (closed_tx, closed_rx) = futures::channel::oneshot::channel();
        let closed = Rc::new(RefCell::new(Some(closed_tx)));
        let close = Closure::wrap(Box::new(move || {
            if let Some(tx) = closed.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn FnMut()>);
        let connection = js_sys::Object::new();
        js_sys::Reflect::set(&connection, &"close".into(), &close.into_js_value()).unwrap();
        for method in ["send", "reconnect"] {
            js_sys::Reflect::set(
                &connection,
                &method.into(),
                &js_sys::Function::new_no_args(""),
            )
            .unwrap();
        }
        resolver
            .borrow_mut()
            .take()
            .unwrap()
            .call1(&JsValue::UNDEFINED, &connection)
            .unwrap();
        closed_rx.await.expect("late relay is explicitly closed");
        let state = state().lock().unwrap();
        let entry = state.live.get(&id.handle).unwrap();
        assert!(!entry.opening);
        assert!(
            entry.live_call.is_none(),
            "cancelled setup must not resurrect its call"
        );
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    static TEST_STATE: Mutex<()> = Mutex::new(());
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn legacy_peers_keep_audio_but_cannot_enable_or_feed_video() {
        let _guard = TEST_STATE.lock().unwrap();
        *state().lock().unwrap() = State::new();
        for (minor, capabilities, video) in [
            (1, ENGINE_CAPABILITIES, true),
            (1, Capabilities::PCM, false),
            (0, ENGINE_CAPABILITIES, false),
        ] {
            let mut hello = Frame::request(
                Opcode::Hello,
                HelloRequest {
                    role: Role::Core,
                    capabilities,
                }
                .encode(),
            );
            hello.minor = minor;
            let response = on_hello(&hello);
            assert!(!response.is_error());
            assert_eq!(response.minor, minor);
            let caps = HelloResponse::decode(&response.payload)
                .unwrap()
                .capabilities;
            assert_ne!(caps & Capabilities::PCM, 0);
            assert_eq!(caps & Capabilities::VIDEO != 0, video);
        }
        let mut params = call::tests::open_params();
        params.enable_video = 1;
        let begin = Frame::request(
            Opcode::BeginOpen,
            BeginOpenRequest {
                session: SessionId {
                    handle: 1,
                    generation: 1,
                },
                params,
            }
            .encode(),
        );
        assert_eq!(
            voip_abi::ErrorBody::decode(&on_begin_open(&begin).payload)
                .unwrap()
                .code,
            AbiErrorCode::NotSupported
        );
        let command = Frame::request(
            Opcode::Command,
            CommandRequest {
                session: SessionId {
                    handle: 1,
                    generation: 1,
                },
                command: voip_abi::MediaCommand::VideoEnable,
            }
            .encode(),
        );
        assert_eq!(
            voip_abi::ErrorBody::decode(&on_command(&command).payload)
                .unwrap()
                .code,
            AbiErrorCode::NotSupported
        );
        let input = Frame::request(
            Opcode::VideoIn,
            voip_abi::VideoIn {
                session: SessionId {
                    handle: 1,
                    generation: 1,
                },
                seq: 0,
                input: voip_abi::VideoInputDto {
                    data: vec![],
                    timestamp: None,
                    input_generation: None,
                },
            }
            .encode(),
        );
        assert_eq!(
            voip_abi::ErrorBody::decode(&on_video_in(&input).payload)
                .unwrap()
                .code,
            AbiErrorCode::NotSupported
        );
    }

    #[test]
    fn media_before_open_is_refused_instead_of_acknowledged_and_lost() {
        let _guard = TEST_STATE.lock().unwrap();
        *state().lock().unwrap() = State::new();
        let id = SessionId {
            handle: 41,
            generation: 9,
        };
        let reserve = Frame::request(
            Opcode::Reserve,
            ReserveRequest {
                session: id,
                call_id: "not-yet-open".to_owned(),
                direction: voip_abi::Direction::Outgoing,
            }
            .encode(),
        );
        assert!(!on_reserve(&reserve).is_error());
        for opcode in [Opcode::PcmIn, Opcode::EncodedAudioIn] {
            let frame = Frame::request(
                opcode,
                MediaFrame {
                    session: id,
                    seq: 0,
                    data: vec![0; 4],
                }
                .encode(),
            );
            let response = on_media_in(&frame);
            assert!(response.is_error());
            assert_eq!(
                voip_abi::ErrorBody::decode(&response.payload).unwrap().code,
                AbiErrorCode::Busy
            );
        }
    }
}
