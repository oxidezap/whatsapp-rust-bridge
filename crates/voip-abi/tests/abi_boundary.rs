//! The ABI crossed the way the WASMs will cross it: only bytes move.
//!
//! A fake engine side (`FakeBackend`) owns its own [`SessionTable`] and its
//! own open state, and every exchange below travels as an encoded [`Frame`]
//! through a `&[u8]` — no shared struct, no shared memory, no JSON. What a
//! pass proves: the handshake, the open/cancel protocol, and the generation
//! guard all work with nothing but the wire between the two sides.

use std::collections::HashMap;
use voip_abi::*;

/// The engine side of the wire, holding no core type.
struct FakeBackend {
    sessions: SessionTable,
    /// Handles with setup in flight (begun, not yet open).
    opening: HashMap<u32, u64>,
    /// Handles with media flowing.
    open: HashMap<u32, u64>,
    caps: u32,
}

impl FakeBackend {
    fn new(caps: u32) -> Self {
        FakeBackend {
            sessions: SessionTable::new(),
            opening: HashMap::new(),
            open: HashMap::new(),
            caps,
        }
    }

    /// One framed request in, one framed response out. Notifications this
    /// produces (OPEN, EVENT, MEDIA_ENDED) are returned separately by the
    /// helpers that cause them.
    fn handle(&mut self, bytes: &[u8]) -> Vec<u8> {
        let frame = Frame::decode(bytes).expect("fake backend reads the frame");
        assert_eq!(frame.major, ABI_MAJOR, "major checked before dispatch");
        match frame.opcode {
            Opcode::Hello => {
                let req = HelloRequest::decode(&frame.payload).expect("HELLO body");
                let agreed =
                    negotiate(frame.major, frame.minor, ABI_MINOR).expect("major already checked");
                assert_eq!(agreed.minor, ABI_MINOR);
                let common = req.capabilities & self.caps;
                Frame::respond(
                    &frame,
                    HelloResponse {
                        capabilities: common,
                    }
                    .encode(),
                )
                .encode()
            }
            Opcode::Reserve => {
                let req = ReserveRequest::decode(&frame.payload).expect("RESERVE body");
                match self.sessions.reserve(req.session, &req.call_id) {
                    Ok(()) => Frame::respond(&frame, Vec::new()).encode(),
                    Err(_) => {
                        Frame::fail(&frame, AbiErrorCode::Busy, Some("handle in use")).encode()
                    }
                }
            }
            Opcode::BeginOpen => {
                let req = BeginOpenRequest::decode(&frame.payload).expect("BEGIN_OPEN body");
                if let Err(e) = self.sessions.validate(req.session) {
                    return Frame::fail(&frame, stale_or_unknown(&e), None).encode();
                }
                if self.opening.contains_key(&req.session.handle)
                    || self.open.contains_key(&req.session.handle)
                {
                    return Frame::fail(&frame, AbiErrorCode::Busy, Some("already opening"))
                        .encode();
                }
                // Params must survive the crossing intact, secrets included.
                assert_eq!(req.params.audio_format.codec, AudioCodecWire::Mlow);
                assert_eq!(req.params.call_key.as_bytes(), &[0x33; 32]);
                self.opening
                    .insert(req.session.handle, req.session.generation);
                Frame::respond(&frame, Vec::new()).encode()
            }
            Opcode::CancelOpen => {
                let req = CancelOpenRequest::decode(&frame.payload).expect("CANCEL_OPEN body");
                let outcome = if self.open.get(&req.session.handle) == Some(&req.session.generation)
                {
                    CancelOutcome::AlreadyOpen
                } else if self.opening.remove(&req.session.handle).is_some() {
                    CancelOutcome::Aborted
                } else {
                    CancelOutcome::Unknown
                };
                Frame::respond(&frame, CancelOpenResponse { outcome }.encode()).encode()
            }
            Opcode::Close => {
                let req = CloseRequest::decode(&frame.payload).expect("CLOSE body");
                self.opening.remove(&req.session.handle);
                self.open.remove(&req.session.handle);
                self.sessions.remove(req.session.handle);
                Frame::respond(&frame, Vec::new()).encode()
            }
            Opcode::Command | Opcode::GroupFits | Opcode::Stats => {
                // Identity first: a stale generation never reaches the match.
                let session = decode_session_of(frame.opcode, &frame.payload);
                match self.sessions.validate(session) {
                    Ok(_) => Frame::respond(&frame, Vec::new()).encode(),
                    Err(e) => Frame::fail(&frame, stale_or_unknown(&e), None).encode(),
                }
            }
            Opcode::Open
            | Opcode::Event
            | Opcode::MediaEnded
            | Opcode::PcmIn
            | Opcode::PcmOut
            | Opcode::EncodedAudioIn
            | Opcode::EncodedAudioOut
            | Opcode::VideoIn
            | Opcode::VideoOut => {
                panic!("fake backend never receives {}", frame.opcode.name())
            }
        }
    }

    /// Completes setup for an in-flight open, producing the OPEN notice.
    fn complete_open(&mut self, session: SessionId) -> Vec<u8> {
        assert_eq!(
            self.opening.remove(&session.handle),
            Some(session.generation),
            "only the current generation completes"
        );
        self.open.insert(session.handle, session.generation);
        Frame::request(Opcode::Open, OpenNotification { session }.encode()).encode()
    }
}

fn stale_or_unknown(e: &SessionError) -> AbiErrorCode {
    match e {
        SessionError::Stale { .. } => AbiErrorCode::StaleGeneration,
        _ => AbiErrorCode::UnknownSession,
    }
}

fn decode_session_of(opcode: Opcode, payload: &[u8]) -> SessionId {
    let mut r = Reader::new(payload);
    let session = SessionId {
        handle: r.u32_le().expect("handle"),
        generation: r.u64_le().expect("generation"),
    };
    assert!(
        matches!(opcode, Opcode::Command | Opcode::GroupFits | Opcode::Stats),
        "identity-led body"
    );
    session
}

fn test_format() -> AudioFormatDto {
    AudioFormatDto {
        codec: AudioCodecWire::Mlow,
        rtp_profile: RtpProfile::Mlow,
        signaling_rate: 16_000,
        sample_rate: 16_000,
        channels: 1,
        samples_per_frame: 960,
        rtp_clock_rate: 16_000,
        rtp_timestamp_step: 960,
        rtp_payload_type: 120,
    }
}

fn test_params() -> OpenParams {
    OpenParams {
        direction: Direction::Outgoing,
        self_lid: "self:1@test".to_owned(),
        peer_lid: "peer:2@test".to_owned(),
        ssrc: 0x1234_5678,
        audio_io: AudioIo::Encoded,
        audio_format: test_format(),
        relay_token: SecretBytes::new(vec![0x11; 16]),
        auth_token: SecretBytes::new(vec![0x22; 16]),
        call_key: SecretBytes::new(vec![0x33; 32]),
        relay_host: "relay.example".to_owned(),
        relay_port: 3478,
        integrity_key: SecretBytes::new(vec![0x44; 32]),
        warp_mi_tag_len: 16,
        enable_media: 1,
        enable_video: 0,
        enable_sframe: 1,
        muted: 0,
        video: 0,
        initial_codec: None,
        peer_orientations: vec![(None, 1)],
        group: Some(GroupOpenSpec {
            call_creator: "creator@test".to_owned(),
            self_jid: "self:1@test".to_owned(),
            initial_update: test_update(),
            direct_peer: None,
            epoch_transaction_id: Some(11),
            epoch: Some(SecretBytes::new(vec![0x55; 24])),
        }),
    }
}

fn test_update() -> GroupCallUpdateDto {
    GroupCallUpdateDto {
        call_id: "call-g".to_owned(),
        call_creator: "creator@test".to_owned(),
        group_jid: Some("group@test".to_owned()),
        transaction_id: 9,
        media: "audio".to_owned(),
        connected_limit: 8,
        joinable: 1,
        av_upgradable: 0,
        rekey_requested: 0,
        participants: vec![GroupParticipantDto {
            jid: "p@test".to_owned(),
            pn: None,
            state: Some("connected".to_owned()),
            participant_type: None,
            devices: vec![GroupDeviceDto {
                jid: "p:1@test".to_owned(),
                platform: Some("android".to_owned()),
                pid: Some(3),
                capability_version: None,
            }],
        }],
        relay: None,
    }
}

/// Sends one request through the wire and reads the success response.
fn round_trip(backend: &mut FakeBackend, opcode: Opcode, body: Vec<u8>) -> (Frame, Vec<u8>) {
    let req = Frame::request(opcode, body);
    let resp_bytes = backend.handle(&req.encode());
    let resp = Frame::decode(&resp_bytes).expect("response frames");
    assert!(
        resp.is_response(),
        "expected success, got flags {:#06x}",
        resp.flags
    );
    assert_eq!(resp.opcode, opcode, "responses echo the opcode");
    (resp, resp_bytes)
}

#[test]
fn handshake_agrees_on_version_and_capabilities() {
    let mut backend =
        FakeBackend::new(Capabilities::PCM | Capabilities::ENCODED_AUDIO | Capabilities::VIDEO);
    let body = HelloRequest {
        role: Role::Core,
        capabilities: Capabilities::ENCODED_AUDIO | Capabilities::VIDEO | Capabilities::GROUP_CALLS,
    }
    .encode();
    let (resp, _) = round_trip(&mut backend, Opcode::Hello, body);
    let hello = HelloResponse::decode(&resp.payload).expect("HELLO response body");
    // The intersection: VIDEO and ENCODED_AUDIO yes, GROUP_CALLS no.
    assert_eq!(
        hello.capabilities,
        Capabilities::ENCODED_AUDIO | Capabilities::VIDEO
    );
}

#[test]
fn handshake_rejects_major_mismatch_with_no_session_state() {
    // The version check runs on the bare header: no table, no client, no
    // call. A peer speaking major 2 fails here, before anything exists.
    let mut raw = Frame::request(
        Opcode::Hello,
        HelloRequest {
            role: Role::Core,
            capabilities: 0,
        }
        .encode(),
    )
    .encode();
    raw[4] = ABI_MAJOR + 1;
    let frame = Frame::decode(&raw).expect("framing is version-independent");
    let err = negotiate(frame.major, frame.minor, ABI_MINOR).expect_err("major must mismatch");
    assert_eq!(
        err,
        VersionError::MajorMismatch {
            peer_major: ABI_MAJOR + 1
        }
    );

    // And the failure has a typed wire form, not a stacked message.
    let req =
        Frame::decode(&Frame::request(Opcode::Hello, Vec::new()).encode()).expect("request frames");
    let fail = Frame::fail(&req, AbiErrorCode::BadVersion, Some("speak 1"));
    let back = Frame::decode(&fail.encode()).expect("error frames");
    assert!(back.is_error());
    let body = ErrorBody::decode(&back.payload).expect("error body");
    assert_eq!(body.code, AbiErrorCode::BadVersion);
    assert_eq!(body.detail.as_deref(), Some("speak 1"));
}

#[test]
fn fake_backend_drives_reserve_begin_open_and_cancel() {
    let mut backend = FakeBackend::new(u32::MAX);
    let session = SessionId {
        handle: 7,
        generation: 1,
    };

    round_trip(
        &mut backend,
        Opcode::Reserve,
        ReserveRequest {
            session,
            call_id: "call-1".to_owned(),
            direction: Direction::Outgoing,
        }
        .encode(),
    );
    round_trip(
        &mut backend,
        Opcode::BeginOpen,
        BeginOpenRequest {
            session,
            params: test_params(),
        }
        .encode(),
    );

    // Walking away mid-flight aborts; nothing is left half-open.
    let (_, cancel_bytes) = round_trip(
        &mut backend,
        Opcode::CancelOpen,
        CancelOpenRequest { session }.encode(),
    );
    let cancel = Frame::decode(&cancel_bytes).expect("cancel response frames");
    let outcome = CancelOpenResponse::decode(&cancel.payload).expect("cancel body");
    assert_eq!(outcome.outcome, CancelOutcome::Aborted);
    assert!(!backend.opening.contains_key(&7));

    // Cancelling again finds nothing in flight.
    let (_, cancel_bytes) = round_trip(
        &mut backend,
        Opcode::CancelOpen,
        CancelOpenRequest { session }.encode(),
    );
    let cancel = Frame::decode(&cancel_bytes).expect("cancel response frames");
    let outcome = CancelOpenResponse::decode(&cancel.payload).expect("cancel body");
    assert_eq!(outcome.outcome, CancelOutcome::Unknown);
}

#[test]
fn fake_backend_completes_open_then_serves_commands_and_stats() {
    let mut backend = FakeBackend::new(u32::MAX);
    let session = SessionId {
        handle: 9,
        generation: 3,
    };

    round_trip(
        &mut backend,
        Opcode::Reserve,
        ReserveRequest {
            session,
            call_id: "call-9".to_owned(),
            direction: Direction::Incoming,
        }
        .encode(),
    );
    round_trip(
        &mut backend,
        Opcode::BeginOpen,
        BeginOpenRequest {
            session,
            params: test_params(),
        }
        .encode(),
    );
    let open_bytes = backend.complete_open(session);
    let open = Frame::decode(&open_bytes).expect("OPEN frames");
    assert_eq!(open.opcode, Opcode::Open);
    assert!(
        !open.is_response(),
        "OPEN is a notification, not a response"
    );
    let notice = OpenNotification::decode(&open.payload).expect("OPEN body");
    assert_eq!(notice.session, session);

    // Cancelling a completed open reports it, without closing anything.
    let (_, cancel_bytes) = round_trip(
        &mut backend,
        Opcode::CancelOpen,
        CancelOpenRequest { session }.encode(),
    );
    let cancel = Frame::decode(&cancel_bytes).expect("cancel response frames");
    let outcome = CancelOpenResponse::decode(&cancel.payload).expect("cancel body");
    assert_eq!(outcome.outcome, CancelOutcome::AlreadyOpen);

    // A video command and a stats poll cross on the live session.
    round_trip(
        &mut backend,
        Opcode::Command,
        CommandRequest {
            session,
            command: MediaCommand::VideoSetInputGeneration(42),
        }
        .encode(),
    );
    round_trip(
        &mut backend,
        Opcode::Stats,
        StatsRequest { session }.encode(),
    );

    // Close releases the handle for reuse.
    round_trip(
        &mut backend,
        Opcode::Close,
        CloseRequest {
            session,
            reason: CloseReason::LocalHangup,
            detail: None,
        }
        .encode(),
    );
    assert!(!backend.sessions.contains(9));
}

#[test]
fn stale_generation_never_touches_the_replacement() {
    let mut backend = FakeBackend::new(u32::MAX);
    let old = SessionId {
        handle: 4,
        generation: 1,
    };
    let new = SessionId {
        handle: 4,
        generation: 2,
    };

    round_trip(
        &mut backend,
        Opcode::Reserve,
        ReserveRequest {
            session: old,
            call_id: "call-4a".to_owned(),
            direction: Direction::Outgoing,
        }
        .encode(),
    );
    round_trip(
        &mut backend,
        Opcode::Close,
        CloseRequest {
            session: old,
            reason: CloseReason::Replaced,
            detail: None,
        }
        .encode(),
    );
    round_trip(
        &mut backend,
        Opcode::Reserve,
        ReserveRequest {
            session: new,
            call_id: "call-4b".to_owned(),
            direction: Direction::Outgoing,
        }
        .encode(),
    );

    // The old generation's command arrives late: typed stale, no dispatch.
    let req = Frame::request(
        Opcode::Command,
        CommandRequest {
            session: old,
            command: MediaCommand::VideoDisable,
        }
        .encode(),
    );
    let resp = Frame::decode(&backend.handle(&req.encode())).expect("stale response frames");
    assert!(resp.is_error());
    let body = ErrorBody::decode(&resp.payload).expect("stale body");
    assert_eq!(body.code, AbiErrorCode::StaleGeneration);

    // The replacement is untouched and usable.
    round_trip(
        &mut backend,
        Opcode::Command,
        CommandRequest {
            session: new,
            command: MediaCommand::VideoDisable,
        }
        .encode(),
    );
    let entry = backend.sessions.validate(new).expect("replacement holds");
    assert_eq!(entry.call_id, "call-4b");
}

#[test]
fn reserve_refuses_to_overwrite_a_live_handle() {
    let mut backend = FakeBackend::new(u32::MAX);
    let first = SessionId {
        handle: 5,
        generation: 1,
    };
    let second = SessionId {
        handle: 5,
        generation: 2,
    };
    round_trip(
        &mut backend,
        Opcode::Reserve,
        ReserveRequest {
            session: first,
            call_id: "call-5a".to_owned(),
            direction: Direction::Outgoing,
        }
        .encode(),
    );
    let req = Frame::request(
        Opcode::Reserve,
        ReserveRequest {
            session: second,
            call_id: "call-5b".to_owned(),
            direction: Direction::Outgoing,
        }
        .encode(),
    );
    let resp = Frame::decode(&backend.handle(&req.encode())).expect("reserve response frames");
    assert!(
        resp.is_error(),
        "overwrite must fail, close-then-reserve replaces"
    );
    let entry = backend.sessions.validate(first).expect("first still holds");
    assert_eq!(entry.call_id, "call-5a");
}

#[test]
fn newer_minor_fields_are_ignored_but_truncation_fails() {
    let mut params = test_params().encode();
    // A newer minor appends a field: the older reader skips it.
    params.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    let decoded = OpenParams::decode(&params).expect("trailing bytes ignored");
    assert_eq!(decoded.audio_format.sample_rate, 16_000);

    // A short payload is a failure, not a partial read.
    let short = &params[..params.len() - 8];
    OpenParams::decode(short).expect_err("truncated payload must fail");
}

#[test]
fn framing_rejects_garbage_before_any_opcode_runs() {
    let mut garbage = Frame::request(Opcode::Stats, Vec::new()).encode();
    garbage[0] = b'X';
    assert!(matches!(
        Frame::decode(&garbage),
        Err(DecodeError::BadMagic { .. })
    ));
    assert!(matches!(
        Frame::decode(&garbage[..5]),
        Err(DecodeError::TruncatedHeader { .. })
    ));

    let mut unknown = Frame::request(Opcode::Stats, Vec::new()).encode();
    unknown[6] = 0x7F;
    assert!(matches!(
        Frame::decode(&unknown),
        Err(DecodeError::UnknownOpcode(0x7F))
    ));

    let mut short = Frame::request(Opcode::Stats, vec![1, 2, 3]).encode();
    short.truncate(short.len() - 1);
    assert!(matches!(
        Frame::decode(&short),
        Err(DecodeError::TruncatedPayload { .. })
    ));
}

#[test]
fn secret_bytes_never_render() {
    let secret = SecretBytes::new(vec![0xAA; 32]);
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("aa"), "Debug must not leak key material");
    assert!(rendered.contains("32"), "length stays visible");
}

#[test]
fn every_message_round_trips() {
    let session = SessionId {
        handle: 1,
        generation: 1,
    };
    let params = test_params();

    macro_rules! check {
        ($ty:ty, $v:expr) => {{
            let v: $ty = $v;
            let back = <$ty>::decode(&v.encode()).expect("round trip");
            assert_eq!(v, back);
        }};
    }

    check!(
        HelloRequest,
        HelloRequest {
            role: Role::Voip,
            capabilities: 0x3F
        }
    );
    check!(HelloResponse, HelloResponse { capabilities: 0x3F });
    check!(
        ReserveRequest,
        ReserveRequest {
            session,
            call_id: "c".to_owned(),
            direction: Direction::Incoming,
        }
    );
    check!(AudioFormatDto, test_format());
    check!(OpenParams, params.clone());
    check!(GroupCallUpdateDto, test_update());
    check!(
        GroupOpenSpec,
        GroupOpenSpec {
            call_creator: "c@test".to_owned(),
            self_jid: "s@test".to_owned(),
            initial_update: test_update(),
            direct_peer: Some(DirectPeerDto {
                user_jid: "u@test".to_owned(),
                device_jid: "u:1@test".to_owned(),
                call_key: SecretBytes::new(vec![7; 32]),
            }),
            epoch_transaction_id: None,
            epoch: None,
        }
    );
    check!(
        BeginOpenRequest,
        BeginOpenRequest {
            session,
            params: params.clone()
        }
    );
    check!(OpenNotification, OpenNotification { session });
    check!(CancelOpenRequest, CancelOpenRequest { session });
    check!(
        CancelOpenResponse,
        CancelOpenResponse {
            outcome: CancelOutcome::Aborted
        }
    );
    check!(
        CommandRequest,
        CommandRequest {
            session,
            command: MediaCommand::RekeyRecv {
                answering_lid: "a@test".to_owned(),
                audio_codec: Some(AudioCodecWire::Opus),
            }
        }
    );
    check!(
        GroupFitsRequest,
        GroupFitsRequest {
            session,
            update: test_update(),
            is_call_link: 1,
        }
    );
    check!(GroupFitsResponse, GroupFitsResponse { fits: 1, limit: 32 });
    check!(
        CloseRequest,
        CloseRequest {
            session,
            reason: CloseReason::Timeout,
            detail: Some("d".to_owned()),
        }
    );
    check!(
        EventNotification,
        EventNotification {
            session,
            event: AbiEvent::AudioCodecSwitched {
                from: AudioCodecWire::Mlow,
                to: AudioCodecWire::Opus,
                source: CodecSource::Negotiated,
                packets_observed: 12,
            },
        }
    );
    check!(
        StatsData,
        StatsData {
            rtp_received: 100,
            rtp_payload_type_unexpected: 1,
            srtp_unprotect_failed: 0,
            sframe_decrypt_failed: 0,
            audio_frames_decoded: 90,
            audio_frames_delivered: 0,
            audio_frames_concealed: 2,
            mlow_off_point_dropped: 0,
            mlow_inactive_or_sid: 0,
            foreign_frames_decoded: 0,
            audio_frames_without_decoder: 0,
            outbound_frames_without_encoder: 0,
            playout_trimmed_samples: 0,
            inbound_pipe_dropped: 0,
            audio_sink_dropped: 0,
            video_sink_dropped: 0,
            peer_keyframe_requests: 1,
            relay_packet_unclassified: 0,
            forwarding_envelope_rejected: 0,
            codec_switches: 1,
        }
    );
    check!(StatsRequest, StatsRequest { session });
    check!(
        MediaFrame,
        MediaFrame {
            session,
            seq: 12,
            data: vec![1, 2, 3]
        }
    );
    check!(
        EncodedAudioOut,
        EncodedAudioOut {
            session,
            seq: 3,
            frame: EncodedFrameDto {
                codec: AudioCodecWire::Opus,
                data: vec![9, 9],
                payload_type: 111,
                sequence_number: 77,
                timestamp: 123_456,
                marker: 1,
                sender: Some("s@test".to_owned()),
                device: None,
            },
        }
    );
    check!(
        VideoIn,
        VideoIn {
            session,
            seq: 4,
            input: VideoInputDto {
                data: vec![0, 0, 0, 1],
                timestamp: Some(90_000),
                input_generation: Some(2),
            },
        }
    );
    check!(
        VideoOut,
        VideoOut {
            session,
            seq: 5,
            frame: VideoFrameDto {
                data: vec![0, 0, 0, 1],
                keyframe: 1,
                orientation: 0,
                sender: None,
                device: None,
                pid: Some(7),
            },
        }
    );

    // Every command and event variant, so a new one has to extend this list.
    for cmd in [
        MediaCommand::VideoEnable,
        MediaCommand::VideoEnableAwaitingAccept,
        MediaCommand::VideoDisable,
        MediaCommand::VideoDisableOutbound,
        MediaCommand::VideoDisableKeepLegacy,
        MediaCommand::VideoRequireKeyframe,
        MediaCommand::VideoRequestPeerKeyframe(Urgency::Immediate),
        MediaCommand::VideoSetOrientation {
            participant: Some("p@test".to_owned()),
            orientation: 2,
        },
        MediaCommand::VideoSetInputGeneration(7),
        MediaCommand::VideoSetTimestampStride(3_000),
        MediaCommand::RekeyRecv {
            answering_lid: "a".to_owned(),
            audio_codec: None,
        },
        MediaCommand::GroupApplyUpdate(test_update()),
        MediaCommand::GroupApplyTransition {
            update: test_update(),
            transaction_id: 4,
            epoch: SecretBytes::new(vec![5; 24]),
        },
        MediaCommand::GroupApplyEpoch {
            transaction_id: 6,
            epoch: SecretBytes::new(vec![6; 24]),
        },
        MediaCommand::GroupSendReaction("👋".to_owned()),
        MediaCommand::AudioMute(1),
    ] {
        let back = MediaCommand::decode(&cmd.encode()).expect("command round trip");
        assert_eq!(cmd, back);
    }
    for ev in [
        AbiEvent::RelayAllocated,
        AbiEvent::ForeignAudio(vec![1, 2]),
        AbiEvent::ForeignGroupAudio(EncodedFrameDto {
            codec: AudioCodecWire::Mlow,
            data: vec![3],
            payload_type: 120,
            sequence_number: 1,
            timestamp: 960,
            marker: 0,
            sender: None,
            device: None,
        }),
        AbiEvent::AudioFormatMismatch {
            expected_rate: 16_000,
            received_rates: vec![8_000],
        },
        AbiEvent::RelayAllocateFailed(401),
        AbiEvent::RelayAllocateTimedOut,
        AbiEvent::MediaSetupFailed("no relay".to_owned()),
        AbiEvent::RelayReconnectTimedOut,
        AbiEvent::VideoKeyframeNeeded,
        AbiEvent::RtcpReceived {
            packet_types: vec![201],
            sender_ssrc: 11,
            referenced_ssrcs: vec![12],
            reports_audio: 1,
            reports_video: 0,
            report_blocks: vec![RtcpBlockDto {
                ssrc: 12,
                fraction_lost: 0,
                cumulative_lost: -3,
                extended_highest_sequence: 99,
                jitter: 4,
                last_sender_report: 0,
                delay_since_last_sender_report: 0,
                profile_extension: vec![],
            }],
            feedback: vec![],
        },
        AbiEvent::OutboundMediaDropped {
            video_access_units: 2,
            packets: 5,
        },
        AbiEvent::AudioSilent {
            silent_for_ms: 3_000,
            rtp_received: 50,
            frames_produced: 0,
            dominant_reason: SilenceReason::AuthenticationFailing,
        },
        AbiEvent::AudioCodecSwitched {
            from: AudioCodecWire::Mlow,
            to: AudioCodecWire::Opus,
            source: CodecSource::Content,
            packets_observed: 30,
        },
        AbiEvent::AudioCodecSourceIsFixed {
            sending: AudioCodecWire::Opus,
            peer_expects: AudioCodecWire::Mlow,
            source: CodecSource::Negotiated,
        },
        AbiEvent::AudioReceptionStalled {
            silent_for_ms: 5_000,
        },
        AbiEvent::Closed(CloseReason::RelayDropped),
    ] {
        let back = AbiEvent::decode(&ev.encode()).expect("event round trip");
        assert_eq!(ev, back);
    }
}
