//! Boundary tests: the backend and session against a fake JS plugin.
//!
//! Real frames cross a real JS boundary — the fake answers through closures
//! — so a pass proves the crossing, not just the codec. The fake speaks
//! OZVP on the plugin side with no knowledge of the Rust types behind it.

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use async_channel::{Receiver, Sender};
use bytes::Bytes;
use voip_abi as abi;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::wasm_bindgen_test as test;
use whatsapp_rust::voip_control as vc;

use super::{ForeignVoipBackend, VoipBackendCallbacks, handshake};

/// What the fake plugin saw, in order: `(opcode, payload)` per request.
type Seen = Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

/// Scripted answers for `BEGIN_OPEN`.
#[derive(Clone)]
enum BeginAnswer {
    Ack,
    Error(abi::AbiErrorCode, Option<String>),
    /// Ack, but never complete: the dial-timeout path.
    AckThenSilence,
    AckThenSetupFailure,
}

struct FakeState {
    seen: Seen,
    push_handler: Arc<Mutex<Option<js_sys::Function>>>,
    hello_caps: u32,
    hello_major: u8,
    hello_minor: u8,
    early_video: Option<abi::VideoFrameDto>,
    begin: BeginAnswer,
    fail_reserve: bool,
    command_busy: std::sync::atomic::AtomicUsize,
    command_accepted: Mutex<Option<futures::channel::oneshot::Sender<()>>>,
}

impl FakeState {
    fn answer(&self, frame: &abi::Frame) -> abi::Frame {
        match frame.opcode {
            abi::Opcode::Hello => {
                let mut resp = abi::Frame::respond(
                    frame,
                    abi::HelloResponse {
                        capabilities: self.hello_caps,
                    }
                    .encode(),
                );
                resp.major = self.hello_major;
                resp.minor = self.hello_minor;
                resp
            }
            abi::Opcode::Reserve => {
                if self.fail_reserve {
                    abi::Frame::fail(frame, abi::AbiErrorCode::Busy, Some("taken"))
                } else {
                    abi::Frame::respond(frame, Vec::new())
                }
            }
            abi::Opcode::BeginOpen => match &self.begin {
                BeginAnswer::Ack
                | BeginAnswer::AckThenSilence
                | BeginAnswer::AckThenSetupFailure => {
                    let ack = abi::Frame::respond(frame, Vec::new());
                    if matches!(self.begin, BeginAnswer::Ack) {
                        // Complete the open like an engine would: read the
                        // session out of the request and push `OPEN` for it.
                        let req =
                            abi::BeginOpenRequest::decode(&frame.payload).expect("BEGIN_OPEN body");
                        if let Some(frame) = &self.early_video {
                            self.push(
                                &abi::Frame::request(
                                    abi::Opcode::VideoOut,
                                    abi::VideoOut {
                                        session: req.session,
                                        seq: 0,
                                        frame: frame.clone(),
                                    }
                                    .encode(),
                                )
                                .encode(),
                            );
                        }
                        let open = abi::Frame::request(
                            abi::Opcode::Open,
                            abi::OpenNotification {
                                session: req.session,
                            }
                            .encode(),
                        );
                        self.push(&open.encode());
                    }
                    if matches!(self.begin, BeginAnswer::AckThenSetupFailure) {
                        let req = abi::BeginOpenRequest::decode(&frame.payload).unwrap();
                        self.push(
                            &abi::Frame::request(
                                abi::Opcode::Event,
                                abi::EventNotification {
                                    session: req.session,
                                    event: abi::AbiEvent::MediaSetupFailed(
                                        "TRANSPORT: relay refused".to_owned(),
                                    ),
                                }
                                .encode(),
                            )
                            .encode(),
                        );
                    }
                    ack
                }
                BeginAnswer::Error(code, detail) => {
                    abi::Frame::fail(frame, *code, detail.as_deref())
                }
            },
            abi::Opcode::CancelOpen => abi::Frame::respond(
                frame,
                abi::CancelOpenResponse {
                    outcome: abi::CancelOutcome::Aborted,
                }
                .encode(),
            ),
            abi::Opcode::Command => {
                if self
                    .command_busy
                    .try_update(
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                        |remaining| remaining.checked_sub(1),
                    )
                    .is_ok()
                {
                    return abi::Frame::fail(frame, abi::AbiErrorCode::Busy, None);
                }
                if let Some(tx) = self.command_accepted.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                abi::Frame::respond(frame, Vec::new())
            }
            abi::Opcode::Close => abi::Frame::respond(frame, Vec::new()),
            _ => abi::Frame::fail(frame, abi::AbiErrorCode::NotSupported, None),
        }
    }

    fn push(&self, bytes: &[u8]) {
        let handler = self.push_handler.lock().ok().and_then(|h| h.clone());
        if let Some(handler) = handler {
            let _ = handler.call1(&JsValue::UNDEFINED, &js_sys::Uint8Array::from(bytes).into());
        }
    }
}

/// Builds live callbacks over a scripted fake. The closures run real JS
/// calls; the state they touch is the only fixture.
fn fake_callbacks(state: Arc<Mutex<FakeState>>) -> VoipBackendCallbacks {
    let for_send = state.clone();
    let send = Closure::wrap(Box::new(move |data: JsValue| {
        let bytes = crate::js_bytes::to_vec(&data.unchecked_into::<js_sys::Uint8Array>());
        let frame = abi::Frame::decode(&bytes).expect("fake reads the request");
        {
            let st = for_send.lock().unwrap();
            st.seen
                .lock()
                .unwrap()
                .push((frame.opcode.as_u8(), frame.payload.clone()));
            let raw = st.answer(&frame).encode();
            js_sys::Promise::resolve(&JsValue::from(js_sys::Uint8Array::from(raw.as_slice())))
        }
    }) as Box<dyn FnMut(JsValue) -> js_sys::Promise>);

    let for_push = state.clone();
    let set_push = Closure::wrap(Box::new(move |handler: JsValue| {
        let func: js_sys::Function = handler.unchecked_into();
        *for_push.lock().unwrap().push_handler.lock().unwrap() = Some(func);
    }) as Box<dyn FnMut(JsValue)>);

    let obj = js_sys::Object::new();
    js_sys::Reflect::set(&obj, &"sendFrame".into(), &send.into_js_value()).unwrap();
    js_sys::Reflect::set(&obj, &"setPushHandler".into(), &set_push.into_js_value()).unwrap();
    VoipBackendCallbacks::from_js(&obj.into()).expect("fake is well-formed")
}

fn backend_with(
    state: Arc<Mutex<FakeState>>,
    ceiling: Option<std::time::Duration>,
) -> (ForeignVoipBackend, Seen) {
    let seen = state.lock().unwrap().seen.clone();
    let callbacks = fake_callbacks(state);
    let runtime =
        Arc::new(crate::runtime::WasmRuntime) as Arc<dyn whatsapp_rust::wacore::runtime::Runtime>;
    let backend = match ceiling {
        Some(d) => ForeignVoipBackend::with_ceiling(callbacks.clone(), u32::MAX, runtime, d),
        None => ForeignVoipBackend::new(callbacks.clone(), u32::MAX, runtime),
    };
    // Install the push entry the way `install` does.
    let push = backend.push_entry();
    callbacks.register_push(&push).expect("fake registers");
    (backend, seen)
}

fn test_key(generation: u64) -> vc::MediaSessionKey {
    vc::MediaSessionKey::builder()
        .call_id("call-test".to_owned())
        .generation(generation)
        .build()
}

struct TestEncodedMic {
    rx: Receiver<Bytes>,
}

impl vc::EncodedAudioSource for TestEncodedMic {
    fn frames(&self) -> Receiver<Bytes> {
        self.rx.clone()
    }
}

struct TestEncodedSpeaker {
    tx: Sender<vc::MediaEncodedFrame>,
}

impl vc::EncodedAudioSink for TestEncodedSpeaker {
    fn frames(&self) -> Sender<vc::MediaEncodedFrame> {
        self.tx.clone()
    }
}

fn test_spec(key: vc::MediaSessionKey) -> vc::MediaSessionSpec {
    vc::MediaSessionSpec::builder()
        .key(key)
        .direction(vc::CallDirection::Outgoing)
        .self_lid("self:1@test".to_owned())
        .peer_lid("peer:2@test".to_owned())
        .call_key(vec![0x33; 32])
        .ssrc(7)
        .audio(
            vc::MediaAudioSpec::builder()
                .format(vc::MediaAudioFormat::MLOW_16KHZ_60MS)
                .io(vc::MediaAudioIo::Encoded)
                .build(),
        )
        .relay_token(vec![0x11; 16])
        .auth_token(vec![0x22; 16])
        .relay_ip("relay.example".to_owned())
        .relay_port(3478)
        .integrity_key(vec![0x44; 32])
        .warp_mi_tag_len(16)
        .enable_media(true)
        .enable_video(false)
        .enable_sframe(true)
        .maybe_group(None)
        .build()
}

fn fake_state() -> Arc<Mutex<FakeState>> {
    Arc::new(Mutex::new(FakeState {
        seen: Arc::new(Mutex::new(Vec::new())),
        push_handler: Arc::new(Mutex::new(None)),
        hello_caps: u32::MAX,
        hello_major: abi::ABI_MAJOR,
        hello_minor: abi::ABI_MINOR,
        early_video: None,
        begin: BeginAnswer::Ack,
        fail_reserve: false,
        command_busy: std::sync::atomic::AtomicUsize::new(0),
        command_accepted: Mutex::new(None),
    }))
}

fn push_frame(state: &Arc<Mutex<FakeState>>, frame: &abi::Frame) {
    let handler = state
        .lock()
        .unwrap()
        .push_handler
        .lock()
        .unwrap()
        .clone()
        .expect("push handler registered");
    handler
        .call1(
            &JsValue::UNDEFINED,
            &js_sys::Uint8Array::from(frame.encode().as_slice()).into(),
        )
        .expect("push delivered");
}

fn seen_opcodes(seen: &Seen) -> Vec<u8> {
    seen.lock().unwrap().iter().map(|(op, _)| *op).collect()
}

/// Yields through a real timer. Spawned work behind the spawn drain parks
/// in a 0ms timer, and a `setImmediate` chain never interleaves with the
/// timer phase (Node runs starved timers only once the loop idles), so
/// waiting on spawned sends with `set_timeout_0` parks forever.
async fn sleep_ms(ms: u64) {
    use whatsapp_rust::wacore::runtime::Runtime;
    crate::runtime::WasmRuntime
        .sleep(std::time::Duration::from_millis(ms))
        .await;
}

#[test]
fn rejects_misshapen_backend() {
    let err = VoipBackendCallbacks::from_js(&JsValue::from_str("nope"))
        .err()
        .expect("rejects");
    assert!(matches!(
        err,
        crate::errors::BridgeError::InvalidArgument { ref field, .. } if field == "extensions"
    ));
    let bare = js_sys::Object::new();
    let err = VoipBackendCallbacks::from_js(&bare.into())
        .err()
        .expect("rejects");
    assert!(matches!(
        err,
        crate::errors::BridgeError::InvalidArgument { ref field, .. } if field == "extensions"
    ));
}

#[test]
async fn handshake_major_mismatch_fails_before_any_session() {
    let state = fake_state();
    state.lock().unwrap().hello_major = abi::ABI_MAJOR + 1;
    let callbacks = fake_callbacks(state);
    let err = handshake(&callbacks).await.unwrap_err();
    assert!(matches!(
        err,
        crate::errors::BridgeError::InvalidArgument { ref field, .. } if field == "extensions"
    ));
}

#[test]
async fn reserve_open_event_stats_close_flow() {
    use vc::{VoipMediaBackend, VoipMediaSession};

    let state = fake_state();
    let (backend, seen) = backend_with(state.clone(), None);

    let key = test_key(1);
    let session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let events = session.subscribe();
    backend
        .open(test_spec(key.clone()), test_ctx())
        .await
        .expect("open resolves on OPEN");
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::BeginOpen.as_u8()));

    // A video command crosses as COMMAND with its discriminant. The pump
    // drains it on a spawned task, so wait on a timer (see `sleep_ms`)
    // rather than yielding once with `set_timeout_0`.
    assert!(session.submit(vc::MediaCommand::DisableVideoOutbound));
    for _ in 0..100 {
        let crossed = seen
            .lock()
            .unwrap()
            .iter()
            .any(|(op, _)| *op == abi::Opcode::Command.as_u8());
        if crossed {
            break;
        }
        sleep_ms(2).await;
    }
    let commands: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(op, _)| *op == abi::Opcode::Command.as_u8())
        .map(|(_, payload)| {
            abi::CommandRequest::decode(payload)
                .expect("COMMAND body")
                .command
        })
        .collect();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, abi::MediaCommand::VideoDisableOutbound))
    );

    // An engine event lands in the same stream the handle reads.
    push_frame(
        &state,
        &abi::Frame::request(
            abi::Opcode::Event,
            abi::EventNotification {
                session: abi::SessionId {
                    handle: 1,
                    generation: 1,
                },
                event: abi::AbiEvent::RelayAllocated,
            }
            .encode(),
        ),
    );
    let event = events.recv().await.expect("event arrives");
    assert!(matches!(event, vc::MediaEvent::RelayAllocated));

    // A stats push refreshes the locally cached snapshot.
    let mut stats = abi::StatsData {
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
    };
    stats.rtp_received = 41;
    push_frame(
        &state,
        &abi::Frame::request(
            abi::Opcode::Stats,
            abi::StatsPush {
                session: abi::SessionId {
                    handle: 1,
                    generation: 1,
                },
                stats,
            }
            .encode(),
        ),
    );
    assert_eq!(session.stats().rtp_received, 41);

    // Close releases the wire and ends the stream. The `CLOSE` send is a
    // detached task behind the spawn drain, so wait on a timer rather
    // than `set_timeout_0` (see `sleep_ms`).
    session.close(vc::MediaCloseReason::Local);
    for _ in 0..100 {
        if seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()) {
            break;
        }
        sleep_ms(2).await;
    }
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()));
    assert!(events.recv().await.is_err(), "stream ends on close");
}

#[test]
async fn reserve_refused_is_setup() {
    use vc::VoipMediaBackend;

    let state = fake_state();
    state.lock().unwrap().fail_reserve = true;
    let (backend, _seen) = backend_with(state, None);
    let key = test_key(5);
    backend.reserve(&key, vc::CallDirection::Outgoing);
    let err = backend
        .open(test_spec(key), test_ctx())
        .await
        .expect_err("reserve refused");
    assert!(
        matches!(err, vc::MediaSetupError::Backend(_)),
        "refused reserve is setup, got {err:?}"
    );
}

#[test]
async fn cancel_sent_when_open_dropped() {
    use vc::VoipMediaBackend;

    let state = fake_state();
    state.lock().unwrap().begin = BeginAnswer::AckThenSilence;
    let (backend, seen) = backend_with(state, None);
    let backend = std::rc::Rc::new(backend);

    let key = test_key(2);
    let session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let worker = backend.clone();
    let runtime =
        Arc::new(crate::runtime::WasmRuntime) as Arc<dyn whatsapp_rust::wacore::runtime::Runtime>;
    let guard = runtime.spawn(Box::pin(async move {
        let _ = worker.open(test_spec(key), test_ctx()).await;
    }));

    // Wait until the in-flight open reached the plugin.
    for _ in 0..100 {
        if seen_opcodes(&seen).contains(&abi::Opcode::BeginOpen.as_u8()) {
            break;
        }
        sleep_ms(2).await;
    }
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::BeginOpen.as_u8()));

    // Walking away cancels the in-flight open: the guard's Drop sends it.
    guard.abort();
    for _ in 0..100 {
        if seen_opcodes(&seen).contains(&abi::Opcode::CancelOpen.as_u8()) {
            break;
        }
        sleep_ms(2).await;
    }
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::CancelOpen.as_u8()));
    // Close what the aborted open attached. The `CLOSE` send is detached
    // behind the spawn drain, so wait until it crossed: returning first
    // would let this test's teardown bleed into the next test's window.
    session.close(vc::MediaCloseReason::Local);
    for _ in 0..100 {
        if seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()) {
            break;
        }
        sleep_ms(2).await;
    }
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()));
}

#[test]
async fn stale_push_never_touches_replacement() {
    use vc::{VoipMediaBackend, VoipMediaSession};

    let state = fake_state();
    let (backend, seen) = backend_with(state.clone(), None);

    let old = test_key(1);
    let old_session = backend.reserve(&old, vc::CallDirection::Outgoing);
    let events = old_session.subscribe();
    let sid = |handle, generation| abi::SessionId { handle, generation };
    let event_frame = |handle, generation| {
        abi::Frame::request(
            abi::Opcode::Event,
            abi::EventNotification {
                session: sid(handle, generation),
                event: abi::AbiEvent::RelayAllocated,
            }
            .encode(),
        )
    };

    push_frame(&state, &event_frame(1, 1));
    assert!(matches!(
        events.recv().await.expect("current event arrives"),
        vc::MediaEvent::RelayAllocated
    ));

    old_session.close(vc::MediaCloseReason::Local);
    assert!(
        events.recv().await.is_err(),
        "closing ends the replaced stream"
    );
    // The old session's `CLOSE` send is detached behind the spawn drain;
    // wait until it crossed so this test's teardown cannot bleed into the
    // next test's window.
    for _ in 0..100 {
        if seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()) {
            break;
        }
        sleep_ms(2).await;
    }
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()));
    let new_session = backend.reserve(&test_key(2), vc::CallDirection::Outgoing);
    let new_events = new_session.subscribe();

    // Closed identities are pruned; the replacement gets a fresh handle.
    // Both pushes cross synchronously, so the queue state afterwards is
    // exact: the fresh event arrived and the stale one never queued. That
    // ordering — not a timer — proves the negative.
    push_frame(&state, &event_frame(1, 1));
    push_frame(&state, &event_frame(2, 1));
    push_frame(&state, &event_frame(2, 2));
    assert!(matches!(
        new_events.try_recv(),
        Ok(vc::MediaEvent::RelayAllocated)
    ));
    assert!(
        matches!(
            new_events.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ),
        "stale push never touches the replacement"
    );
}

#[test]
async fn dial_timeout_is_connect() {
    use vc::VoipMediaBackend;

    let state = fake_state();
    state.lock().unwrap().begin = BeginAnswer::AckThenSilence;
    let (backend, seen) = backend_with(state, Some(std::time::Duration::from_millis(50)));

    let key = test_key(3);
    let session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let err = backend
        .open(test_spec(key), test_ctx())
        .await
        .expect_err("media never comes up");
    assert!(
        matches!(err, vc::MediaSetupError::Connect(_)),
        "dial timeout is Connect, got {err:?}"
    );
    // Close what the failed open attached, and wait until the `CLOSE`
    // crossed: the send is detached behind the spawn drain, and returning
    // first would let this test's teardown bleed into the next test.
    session.close(vc::MediaCloseReason::Local);
    for _ in 0..100 {
        if seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()) {
            break;
        }
        sleep_ms(2).await;
    }
    assert!(seen_opcodes(&seen).contains(&abi::Opcode::Close.as_u8()));
}

#[test]
async fn transport_error_is_connect_and_internal_is_setup() {
    use vc::VoipMediaBackend;

    for (code, connects) in [
        (abi::AbiErrorCode::Transport, true),
        (abi::AbiErrorCode::Internal, false),
        (abi::AbiErrorCode::Busy, false),
    ] {
        let state = fake_state();
        state.lock().unwrap().begin = BeginAnswer::Error(code, Some("no relay".to_owned()));
        let (backend, _seen) = backend_with(state, None);
        let key = test_key(4);
        backend.reserve(&key, vc::CallDirection::Outgoing);
        let err = backend
            .open(test_spec(key), test_ctx())
            .await
            .expect_err("plugin refuses");
        assert_eq!(
            matches!(err, vc::MediaSetupError::Connect(_)),
            connects,
            "code {} must {}be Connect, got {err:?}",
            code.name(),
            if connects { "" } else { "not " }
        );
    }
}

fn test_ctx() -> vc::MediaOpenContext {
    let (_mic_tx, mic_rx) = async_channel::bounded(8);
    let (spk_tx, _spk_rx) = async_channel::bounded::<vc::MediaEncodedFrame>(8);
    let (control_tx, control_rx) =
        whatsapp_rust::wacore::voip_control::control::video_control_channel();
    let (_vin_tx, vin_rx) = async_channel::bounded(8);
    let (vout_tx, _vout_rx) = async_channel::bounded::<vc::VideoFrame>(8);
    let channels = vc::MediaVideoChannels::builder()
        .control(control_rx)
        .control_sender(control_tx)
        .video_in(vin_rx)
        .maybe_timed_video_in(None)
        .video_out(vout_tx)
        .build();
    vc::MediaOpenContext::builder()
        .audio(vc::MediaAudioPorts::Encoded {
            source: Arc::new(TestEncodedMic { rx: mic_rx }) as Arc<dyn vc::EncodedAudioSource>,
            sink: Arc::new(TestEncodedSpeaker { tx: spk_tx }) as Arc<dyn vc::EncodedAudioSink>,
        })
        .video_channels(channels)
        .maybe_video_teardown(None)
        .peer_video_orientations(Vec::new())
        .muted(Arc::new(AtomicBool::new(false)))
        .maybe_group_epoch(None)
        .maybe_initial_codec(None)
        .build()
}

#[test]
async fn remote_end_closes_the_stream_and_runs_teardown() {
    use vc::VoipMediaBackend;
    let state = fake_state();
    let (backend, _) = backend_with(state.clone(), None);
    let key = test_key(71);
    let session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let events = session.subscribe();
    let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = ran.clone();
    let mut ctx = test_ctx();
    ctx.video_teardown = Some(Box::new(move || {
        observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    backend.open(test_spec(key), ctx).await.unwrap();
    let id = {
        let seen = state.lock().unwrap().seen.clone();
        let seen = seen.lock().unwrap();
        let body = &seen
            .iter()
            .find(|(op, _)| *op == abi::Opcode::Reserve.as_u8())
            .unwrap()
            .1;
        abi::ReserveRequest::decode(body).unwrap().session
    };
    push_frame(
        &state,
        &abi::Frame::request(
            abi::Opcode::MediaEnded,
            abi::CloseRequest {
                session: id,
                reason: abi::CloseReason::LocalHangup,
                detail: None,
            }
            .encode(),
        ),
    );
    assert!(matches!(events.recv().await, Ok(vc::MediaEvent::Closed(_))));
    assert!(events.recv().await.is_err());
    assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
    session.close(vc::MediaCloseReason::Local);
    assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
async fn asynchronous_setup_failure_keeps_its_transport_detail() {
    use vc::VoipMediaBackend;
    let state = fake_state();
    state.lock().unwrap().begin = BeginAnswer::AckThenSetupFailure;
    let (backend, _) = backend_with(state, None);
    let key = test_key(72);
    let _session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let error = backend.open(test_spec(key), test_ctx()).await.unwrap_err();
    assert!(matches!(error, vc::MediaSetupError::Connect(ref detail) if detail == "relay refused"));
}

#[test]
async fn busy_commands_retry_without_ending_the_session() {
    use vc::VoipMediaBackend;
    let state = fake_state();
    let (accepted_tx, accepted_rx) = futures::channel::oneshot::channel();
    state
        .lock()
        .unwrap()
        .command_busy
        .store(2, std::sync::atomic::Ordering::SeqCst);
    *state.lock().unwrap().command_accepted.lock().unwrap() = Some(accepted_tx);
    let (backend, seen) = backend_with(state, None);
    let key = test_key(73);
    let session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let events = session.subscribe();
    backend.open(test_spec(key), test_ctx()).await.unwrap();
    assert!(session.submit(vc::MediaCommand::DisableVideoOutbound));
    let failure = events.recv();
    futures::pin_mut!(failure);
    match futures::future::select(accepted_rx, failure).await {
        futures::future::Either::Left((Ok(()), _)) => {}
        futures::future::Either::Left((Err(error), _)) => {
            panic!("acceptance signal cancelled: {error}")
        }
        futures::future::Either::Right((event, _)) => panic!(
            "stream settled before command acceptance: {event:?}; requests: {:?}",
            seen_opcodes(&seen)
        ),
    }
    assert_eq!(
        seen_opcodes(&seen)
            .iter()
            .filter(|op| **op == abi::Opcode::Command.as_u8())
            .count(),
        3
    );
    assert!(matches!(
        events.try_recv(),
        Err(async_channel::TryRecvError::Empty)
    ));
    session.close(vc::MediaCloseReason::Local);
}

#[test]
async fn video_capability_requires_timestamp_metadata_minor() {
    let state = fake_state();
    let callbacks = fake_callbacks(state.clone());
    assert_ne!(
        handshake(&callbacks).await.unwrap() & abi::Capabilities::VIDEO,
        0
    );
    state.lock().unwrap().hello_minor = 0;
    let legacy = handshake(&callbacks).await.unwrap();
    assert_eq!(legacy & abi::Capabilities::VIDEO, 0);
    assert_ne!(legacy & abi::Capabilities::ENCODED_AUDIO, 0);
}

#[test]
async fn video_output_preserves_metadata_and_sheds_without_blocking() {
    use vc::{VoipMediaBackend, VoipMediaSession};
    let state = fake_state();
    let (backend, seen) = backend_with(state.clone(), None);
    let key = test_key(912);
    let session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let mut spec = test_spec(key.clone());
    spec.enable_video = true;
    let mut ctx = test_ctx();
    let (sink, received) = async_channel::bounded(1);
    ctx.video_channels.video_out = sink;
    backend.open(spec, ctx).await.unwrap();
    let id = seen
        .lock()
        .unwrap()
        .iter()
        .find_map(|(op, payload)| {
            (*op == abi::Opcode::BeginOpen.as_u8())
                .then(|| abi::BeginOpenRequest::decode(payload).unwrap().session)
        })
        .unwrap();
    let dto = abi::VideoFrameDto {
        data: vec![0, 0, 0, 1, 0x65, 0x80],
        keyframe: 1,
        orientation: 3,
        sender: Some("100000000000001@lid".to_owned()),
        device: Some("100000000000001:2@lid".to_owned()),
        pid: Some(0),
        timestamp: u32::MAX,
    };
    let push = |session, frame: abi::VideoFrameDto| {
        push_frame(
            &state,
            &abi::Frame::request(
                abi::Opcode::VideoOut,
                abi::VideoOut {
                    session,
                    seq: 7,
                    frame,
                }
                .encode(),
            ),
        )
    };
    // Invalid identities and payloads cannot consume the bounded sink.
    push(
        abi::SessionId {
            generation: id.generation + 1,
            ..id
        },
        dto.clone(),
    );
    let mut invalid = dto.clone();
    invalid.sender = Some("invalid jid".to_owned());
    push(id, invalid);
    let mut truncated = abi::VideoOut {
        session: id,
        seq: 0,
        frame: dto.clone(),
    }
    .encode();
    truncated.pop();
    push_frame(
        &state,
        &abi::Frame::request(abi::Opcode::VideoOut, truncated),
    );
    assert!(received.is_empty());
    push(id, dto.clone());
    push(id, dto.clone());
    assert_eq!(received.len(), 1);
    assert_eq!(session.stats().video_sink_dropped, 1);
    let owned = received.try_recv().unwrap();
    assert_eq!(owned.data, dto.data);
    assert!(owned.keyframe);
    assert_eq!(owned.orientation, 3);
    assert_eq!(
        owned.sender.as_ref().unwrap().to_string(),
        dto.sender.unwrap()
    );
    assert_eq!(
        owned.device.as_ref().unwrap().to_string(),
        dto.device.unwrap()
    );
    assert_eq!(owned.pid, Some(0));
    assert_eq!(owned.timestamp, u32::MAX);
    assert_eq!(owned.generation, key.generation);
    let minimal = abi::VideoFrameDto {
        data: vec![0, 0, 0, 1, 0x41],
        keyframe: 0,
        orientation: 0,
        sender: None,
        device: None,
        pid: None,
        timestamp: 0,
    };
    push(id, minimal.clone());
    let direct = received.try_recv().unwrap();
    assert!(direct.sender.is_none() && direct.device.is_none() && direct.pid.is_none());
    assert_eq!(direct.timestamp, 0);
    assert!(!direct.keyframe);
    session.close(vc::MediaCloseReason::Local);
    push(id, minimal);
    assert!(received.is_empty());
    assert!(received.is_closed());
    assert_eq!(owned.data, [0, 0, 0, 1, 0x65, 0x80]);
}

#[test]
async fn video_sink_is_ready_before_open_notification() {
    use vc::VoipMediaBackend;
    let state = fake_state();
    state.lock().unwrap().early_video = Some(abi::VideoFrameDto {
        data: vec![0, 0, 0, 1, 0x65],
        keyframe: 1,
        orientation: 0,
        sender: None,
        device: None,
        pid: None,
        timestamp: 1920,
    });
    let (backend, _) = backend_with(state, None);
    let key = test_key(913);
    let _session = backend.reserve(&key, vc::CallDirection::Outgoing);
    let mut spec = test_spec(key);
    spec.enable_video = true;
    let mut ctx = test_ctx();
    let (sink, frames) = async_channel::bounded(1);
    ctx.video_channels.video_out = sink;
    backend.open(spec, ctx).await.unwrap();
    let first = frames
        .try_recv()
        .expect("first keyframe emitted before OPEN is retained");
    assert_eq!(first.timestamp, 1920);
    assert_eq!(first.generation, 913);
    assert!(first.keyframe);
}
