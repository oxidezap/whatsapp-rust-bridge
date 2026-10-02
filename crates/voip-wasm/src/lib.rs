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
    | Capabilities::ENCODED_AUDIO
    | Capabilities::VIDEO
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
    /// The running call, once `BeginOpen` built its engine and the `OPEN`
    /// push went out. `None` while setup is still in flight.
    live_call: Option<LiveCall>,
}

/// The engine module's mutable state, behind one lock.
struct State {
    /// The minor both sides agreed to speak, once the handshake ran.
    agreed_minor: Option<u8>,
    /// Handle-to-reservation registry with generation checks.
    sessions: SessionTable,
    /// The live sessions behind the reserved handles.
    live: HashMap<u32, EngineSession>,
}

impl State {
    fn new() -> Self {
        State {
            agreed_minor: None,
            sessions: SessionTable::new(),
            live: HashMap::new(),
        }
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::new()))
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
    if let Ok(mut st) = state().lock() {
        st.agreed_minor = Some(agreed.minor);
    }
    Ok(js_sys::Uint8Array::from(
        HelloResponse {
            capabilities: ENGINE_CAPABILITIES,
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
pub fn send_frame(frame: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
    let req = Frame::decode(frame)
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
    if let Ok(mut st) = state().lock() {
        st.agreed_minor = Some(hello_cap_minor(req.minor));
    }
    Frame::respond(
        req,
        HelloResponse {
            capabilities: ENGINE_CAPABILITIES,
        }
        .encode(),
    )
}

/// The minor the two sides speak: the lower of the two, whose reader both
/// sides already satisfy by ignoring trailing bytes. `ABI_MINOR` is 0, so
/// the agreement is 0 unconditionally — the `min` only becomes load-bearing
/// when the local minor moves.
fn hello_cap_minor(_peer_minor: u8) -> u8 {
    ABI_MINOR
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
    let (call_id, params) = match state().lock() {
        Ok(mut st) => {
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
            (session.call_id.clone(), begin.params)
        }
        Err(_) => {
            return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged"));
        }
    };
    let ack = Frame::respond(req, Vec::new());
    let session = begin.session;
    wasm_bindgen_futures::spawn_local(async move {
        open_async(session, call_id, params).await;
    });
    ack
}

/// Builds the engine and starts the drive loop off the request path: `dial`
/// awaits the plugin's JS transport, so it cannot run inside `send_frame`.
/// On success the `OPEN` push goes out; on refusal the session is released
/// back to reserved so the core can retry or close, and the failure rides
/// an `EVENT(MEDIA_SETUP_FAILED)` the core's session raises.
async fn open_async(session: SessionId, call_id: String, params: voip_abi::OpenParams) {
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
    if entry.id.generation != session.generation {
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
        voip_abi::AbiEvent::MediaSetupFailed(format!("{}: {error:?}", error.code().name())),
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
    let _reason = close.reason;
    if let Some(session) = st.live.remove(&close.session.handle)
        && let Some(call) = session.live_call
    {
        // The abort drops the transport and closes the relay channel;
        // the ended fan already pushed `MEDIA_ENDED`, so no extra push.
        call.close();
    }
    st.sessions.remove(close.session.handle);
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
    if let Some(call) = session.live_call.as_ref() {
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
    if let Some(call) = session.live_call.as_ref() {
        call.video_in(&frame.input);
    }
    Frame::respond(req, Vec::new())
}

/// Zero counters: the stub engine has counted nothing. The shape the core
/// caches, with no counters invented.
fn zero_stats() -> StatsData {
    StatsData {
        rtp_received: 0,
        rtp_payload_type_unexpected: 0,
        srtp_unprotect_failed: 0,
        sframe_decrypt_failed: 0,
        audio_frames_decoded: 0,
        audio_frames_delivered: 0,
        audio_frames_concealed: 0,
        mlow_off_point_dropped: 0,
        mlow_inactive_or_sid: 0,
        foreign_frames_decoded: 0,
        audio_frames_without_decoder: 0,
        outbound_frames_without_encoder: 0,
        playout_trimmed_samples: 0,
        inbound_pipe_dropped: 0,
        audio_sink_dropped: 0,
        video_sink_dropped: 0,
        peer_keyframe_requests: 0,
        relay_packet_unclassified: 0,
        forwarding_envelope_rejected: 0,
        codec_switches: 0,
    }
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
