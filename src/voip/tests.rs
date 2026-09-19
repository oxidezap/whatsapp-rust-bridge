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
}

struct FakeState {
    seen: Seen,
    push_handler: Arc<Mutex<Option<js_sys::Function>>>,
    hello_caps: u32,
    hello_major: u8,
    begin: BeginAnswer,
    fail_reserve: bool,
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
                BeginAnswer::Ack | BeginAnswer::AckThenSilence => {
                    let ack = abi::Frame::respond(frame, Vec::new());
                    if matches!(self.begin, BeginAnswer::Ack) {
                        // Complete the open like an engine would: read the
                        // session out of the request and push `OPEN` for it.
                        let req =
                            abi::BeginOpenRequest::decode(&frame.payload).expect("BEGIN_OPEN body");
                        let open = abi::Frame::request(
                            abi::Opcode::Open,
                            abi::OpenNotification {
                                session: req.session,
                            }
                            .encode(),
                        );
                        self.push(&open.encode());
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
            abi::Opcode::Command => abi::Frame::respond(frame, Vec::new()),
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
        begin: BeginAnswer::Ack,
        fail_reserve: false,
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
    let backend = Arc::new(backend);

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
    let sid = |generation| abi::SessionId {
        handle: 1,
        generation,
    };
    let event_frame = |generation| {
        abi::Frame::request(
            abi::Opcode::Event,
            abi::EventNotification {
                session: sid(generation),
                event: abi::AbiEvent::RelayAllocated,
            }
            .encode(),
        )
    };

    push_frame(&state, &event_frame(1));
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

    // The replacement keeps the call's handle under the new generation.
    // Both pushes cross synchronously, so the queue state afterwards is
    // exact: the fresh event arrived and the stale one never queued. That
    // ordering — not a timer — proves the negative.
    push_frame(&state, &event_frame(1));
    push_frame(&state, &event_frame(2));
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
