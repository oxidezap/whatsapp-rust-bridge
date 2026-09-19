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
//! The engine itself is not yet wired: the handshake, the reservation table,
//! and the frame routing below are live, but `BeginOpen` acknowledges
//! without building a `CallEngine`. Wiring the engine needs the relay
//! transport the plugin side supplies (`VoipRelayTransport` in `ts/`), and
//! inventing its shape here would be guessing at the contract. That is the
//! stub, recorded honestly: no engine runs in this revision.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use voip_abi::{
    ABI_MAJOR, ABI_MINOR, AbiErrorCode, BeginOpenRequest, CancelOpenRequest, CancelOpenResponse,
    CancelOutcome, Capabilities, CommandRequest, Frame, GroupFitsRequest, GroupFitsResponse,
    HelloRequest, HelloResponse, MediaFrame, Opcode, ReserveRequest, Role, SessionId, SessionTable,
    StatsData, StatsRequest,
};
use wasm_bindgen::prelude::*;

/// Capability bits this side offers: everything the (stub) engine will
/// carry once wired. `RELAY_RECONNECT` stays out: the relay lives on the
/// plugin's transport, so this side must not promise reconnect help it
/// never gives. Mirrors `ABI_CAPABILITIES` on the core side.
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
    /// Whether `BeginOpen` has completed (the `OPEN` push went out).
    open: bool,
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
            open: false,
        },
    );
    Frame::respond(req, Vec::new())
}

/// Starts asynchronous setup with the open params. The ack means setup
/// started, not finished; completion arrives as an `OPEN` push. The engine
/// itself is not yet wired, so this revision completes setup inline: the
/// ack goes out and the `OPEN` push follows, with no media flowing yet.
fn on_begin_open(req: &Frame) -> Frame {
    let Ok(begin) = BeginOpenRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("BEGIN_OPEN misformed"));
    };
    let mut st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
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
    if session.opening || session.open {
        return Frame::fail(req, AbiErrorCode::Busy, Some("open already in flight"));
    }
    // The params decode above; the engine they would build does not exist
    // yet. Record the intent so `CANCEL_OPEN` has something to abort and
    // complete inline: ack now, `OPEN` push next.
    let _params = begin.params;
    session.opening = true;
    session.open = true;
    session.opening = false;
    let id = session.id;
    drop(st);
    push_open(id);
    Frame::respond(req, Vec::new())
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

/// Aborts an in-flight `BeginOpen`. The ack carries the outcome: setup
/// here completes inline, so there is never anything in flight — an open
/// session reports `AlreadyOpen`, anything else `Unknown`.
fn on_cancel_open(req: &Frame) -> Frame {
    let Ok(cancel) = CancelOpenRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("CANCEL_OPEN misformed"));
    };
    let st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(cancel.session) {
        return session_error(req, e);
    }
    let outcome = match st.live.get(&cancel.session.handle) {
        Some(session) if session.id.generation == cancel.session.generation => {
            if session.opening {
                CancelOutcome::Aborted
            } else if session.open {
                CancelOutcome::AlreadyOpen
            } else {
                CancelOutcome::Unknown
            }
        }
        _ => CancelOutcome::Unknown,
    };
    Frame::respond(req, CancelOpenResponse { outcome }.encode())
}

/// Applies one control intent to a live session. The stub engine accepts
/// every well-formed command; the behavior they name arrives with the
/// engine wiring.
fn on_command(req: &Frame) -> Frame {
    let Ok(cmd) = CommandRequest::decode(&req.payload) else {
        return Frame::fail(req, AbiErrorCode::BadPayload, Some("COMMAND misformed"));
    };
    let st = match state().lock() {
        Ok(st) => st,
        Err(_) => return Frame::fail(req, AbiErrorCode::Internal, Some("engine state is wedged")),
    };
    if let Err(e) = st.sessions.validate(cmd.session) {
        return session_error(req, e);
    }
    let Some(session) = st.live.get(&cmd.session.handle) else {
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
    // The command decoded; applying it is engine work that does not exist
    // yet. Accept it so the core's command grammar stays exercised across
    // the boundary.
    let _command = cmd.command;
    Frame::respond(req, Vec::new())
}

/// Admission probe for a group roster. The stub engine fits nothing: it
/// carries no participant limit of its own, so it refuses rather than
/// invents one.
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
    let _update = fits.update;
    Frame::respond(req, GroupFitsResponse { fits: 0, limit: 0 }.encode())
}

/// Releases a session. Closing is idempotent: dropping half-open state
/// twice is normal on the teardown path.
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
    st.live.remove(&close.session.handle);
    st.sessions.remove(close.session.handle);
    Frame::respond(req, Vec::new())
}

/// Serves a stats poll for one session. The stub engine has counted
/// nothing, so it answers zeroes — the shape the core caches, with no
/// counters invented.
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
    Frame::respond(req, zero_stats().encode())
}

/// Consumes one inbound media frame. The stub engine drops it after the
/// session check: the bytes decoded, the generation matched, nothing
/// flowed. The ack is the whole of the behavior.
fn on_media_in(req: &Frame) -> Frame {
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
    let _bytes = frame.data;
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
