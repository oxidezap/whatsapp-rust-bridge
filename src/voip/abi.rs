//! Conversions between the core's neutral seam types and the OZVP DTOs.
//!
//! Every function here projects a core decision into wire bytes, or reads
//! wire bytes back into the core's vocabulary. Nothing is renamed and no
//! default is invented: a core enum gains a future variant, the mapping
//! returns `None` and the caller refuses rather than guesses.

use bytes::Bytes;
use voip_abi as abi;
use whatsapp_rust::voip_control as vc;
use whatsapp_rust::wacore_binary::jid::Jid;

/// Projects the call direction. `None` on a variant this bridge predates.
pub fn map_direction(d: vc::CallDirection) -> Option<abi::Direction> {
    match d {
        vc::CallDirection::Outgoing => Some(abi::Direction::Outgoing),
        vc::CallDirection::Incoming => Some(abi::Direction::Incoming),
        _ => None,
    }
}

/// Projects the audio I/O mode.
pub fn map_audio_io(io: vc::MediaAudioIo) -> Option<abi::AudioIo> {
    match io {
        vc::MediaAudioIo::Pcm => Some(abi::AudioIo::Pcm),
        vc::MediaAudioIo::Encoded => Some(abi::AudioIo::Encoded),
        _ => None,
    }
}

/// Projects the audio codec.
pub fn map_audio_codec(codec: vc::MediaAudioCodec) -> Option<abi::AudioCodecWire> {
    match codec {
        vc::MediaAudioCodec::Mlow => Some(abi::AudioCodecWire::Mlow),
        vc::MediaAudioCodec::Opus => Some(abi::AudioCodecWire::Opus),
        _ => None,
    }
}

/// Reads a wire codec back into the core vocabulary. Total: the wire names
/// only codecs the core named first.
pub fn unmap_audio_codec(codec: abi::AudioCodecWire) -> vc::MediaAudioCodec {
    match codec {
        abi::AudioCodecWire::Mlow => vc::MediaAudioCodec::Mlow,
        abi::AudioCodecWire::Opus => vc::MediaAudioCodec::Opus,
    }
}

/// Projects the RTP profile.
pub fn map_rtp_profile(profile: vc::MediaAudioRtpProfile) -> Option<abi::RtpProfile> {
    match profile {
        vc::MediaAudioRtpProfile::Mlow => Some(abi::RtpProfile::Mlow),
        vc::MediaAudioRtpProfile::StandardOpus => Some(abi::RtpProfile::StandardOpus),
        _ => None,
    }
}

/// Projects the full audio timing record.
pub fn map_audio_format(format: &vc::MediaAudioFormat) -> Option<abi::AudioFormatDto> {
    Some(abi::AudioFormatDto {
        codec: map_audio_codec(format.codec)?,
        rtp_profile: map_rtp_profile(format.rtp_profile)?,
        signaling_rate: format.signaling_rate,
        sample_rate: format.sample_rate,
        channels: format.channels,
        samples_per_frame: format.samples_per_frame,
        rtp_clock_rate: format.rtp_clock_rate,
        rtp_timestamp_step: format.rtp_timestamp_step,
        rtp_payload_type: format.rtp_payload_type,
    })
}

/// Projects keyframe urgency.
pub fn map_urgency(u: vc::MediaKeyframeUrgency) -> Option<abi::Urgency> {
    match u {
        vc::MediaKeyframeUrgency::Coalesced => Some(abi::Urgency::Coalesced),
        vc::MediaKeyframeUrgency::Immediate => Some(abi::Urgency::Immediate),
        _ => None,
    }
}

/// Bits of the opening context that join the spec in the open params.
pub struct CtxBits<'a> {
    /// Microphone mute flag at open time.
    pub muted: bool,
    /// Video direction bits.
    pub video_caps: u8,
    /// Caller-selected codec adopted before the first packet, if any.
    pub initial_codec: Option<vc::MediaAudioCodec>,
    /// Peer rotations announced before media attached.
    pub peer_orientations: Vec<(Option<Jid>, u8)>,
    /// Pre-authenticated group epoch the caller fanned out, if any.
    pub group_epoch: Option<(u32, &'a vc::MediaGroupEpoch)>,
}

/// Projects a session spec plus its opening context into open params.
pub fn params_from_spec(
    spec: &vc::MediaSessionSpec,
    ctx: &CtxBits<'_>,
) -> Result<abi::OpenParams, vc::MediaSetupError> {
    let direction = map_direction(spec.direction)
        .ok_or_else(|| backend("call direction this bridge predates"))?;
    let audio_io =
        map_audio_io(spec.audio.io).ok_or_else(|| backend("audio I/O this bridge predates"))?;
    let audio_format = map_audio_format(&spec.audio.format)
        .ok_or_else(|| backend("audio format this bridge predates"))?;
    let initial_codec = match ctx.initial_codec {
        None => None,
        Some(codec) => Some(
            map_audio_codec(codec).ok_or_else(|| backend("initial codec this bridge predates"))?,
        ),
    };
    let peer_orientations = ctx
        .peer_orientations
        .iter()
        .map(|(jid, orientation)| (jid.as_ref().map(Jid::to_string), *orientation))
        .collect();
    let group = spec
        .group
        .as_ref()
        .map(|g| group_open_spec(g, ctx.group_epoch.as_ref()))
        .transpose()?;
    Ok(abi::OpenParams {
        direction,
        self_lid: spec.self_lid.clone(),
        peer_lid: spec.peer_lid.clone(),
        ssrc: spec.ssrc,
        audio_io,
        audio_format,
        relay_token: abi::SecretBytes::new(spec.relay_token.clone()),
        auth_token: abi::SecretBytes::new(spec.auth_token.clone()),
        call_key: abi::SecretBytes::new(spec.call_key.clone()),
        relay_host: spec.relay_ip.clone(),
        relay_port: u32::from(spec.relay_port),
        integrity_key: abi::SecretBytes::new(spec.integrity_key.clone()),
        warp_mi_tag_len: u32::try_from(spec.warp_mi_tag_len).unwrap_or(u32::MAX),
        enable_media: u8::from(spec.enable_media),
        enable_video: u8::from(spec.enable_video),
        enable_sframe: u8::from(spec.enable_sframe),
        muted: u8::from(ctx.muted),
        video: ctx.video_caps,
        initial_codec,
        peer_orientations,
        group,
    })
}

/// Projects the group spec plus a fanned-out epoch into the group open.
fn group_open_spec(
    group: &vc::MediaGroupSpec,
    epoch: Option<&(u32, &vc::MediaGroupEpoch)>,
) -> Result<abi::GroupOpenSpec, vc::MediaSetupError> {
    Ok(abi::GroupOpenSpec {
        call_creator: group.call_creator.to_string(),
        self_jid: group.self_jid.to_string(),
        initial_update: group_update_to_dto(&group.initial_update),
        direct_peer: group.direct_peer.as_ref().map(|p| abi::DirectPeerDto {
            user_jid: p.user_jid.to_string(),
            device_jid: p.device_jid.to_string(),
            call_key: abi::SecretBytes::new(p.call_key.clone()),
        }),
        epoch_transaction_id: epoch.map(|(tx, _)| *tx),
        epoch: epoch.map(|(_, e)| abi::SecretBytes::new(e.as_bytes().to_vec())),
    })
}

/// Projects a committed roster snapshot. The relay keys, device capability
/// blobs, and resolved socket addresses are crate-private upstream and do
/// not cross; rotation arrives through the epoch path.
pub fn group_update_to_dto(update: &vc::GroupCallUpdate) -> abi::GroupCallUpdateDto {
    abi::GroupCallUpdateDto {
        call_id: update.call_id.clone(),
        call_creator: update.call_creator.to_string(),
        group_jid: update.group_jid.as_ref().map(Jid::to_string),
        transaction_id: update.transaction_id,
        media: update.media.clone(),
        connected_limit: update.connected_limit,
        joinable: u8::from(update.joinable),
        av_upgradable: u8::from(update.av_upgradable),
        rekey_requested: u8::from(update.rekey_requested),
        participants: update
            .participants
            .iter()
            .map(|p| abi::GroupParticipantDto {
                jid: p.jid.to_string(),
                pn: p.pn.as_ref().map(Jid::to_string),
                state: p.state.clone(),
                participant_type: p.participant_type.clone(),
                devices: p
                    .devices
                    .iter()
                    .map(|d| abi::GroupDeviceDto {
                        jid: d.jid.to_string(),
                        platform: d.platform.clone(),
                        pid: d.pid,
                        capability_version: d.capability_version,
                    })
                    .collect(),
            })
            .collect(),
        relay: update.relay.as_ref().map(|r| abi::GroupRelayDto {
            transaction_id: r.transaction_id,
            self_pid: r.self_pid,
            uuid: r.uuid.clone(),
            participant_uuid: r.participant_uuid.clone(),
            attribute_padding: u8::from(r.attribute_padding),
            warp_mi_tag_len: r.warp_mi_tag_len,
            endpoints: r
                .endpoints
                .iter()
                .map(|e| abi::GroupRelayEndpointDto {
                    relay_id: e.relay_id,
                    token_id: e.token_id,
                    auth_token_id: e.auth_token_id,
                    relay_name: e.relay_name.clone(),
                    domain_name: e.domain_name.clone(),
                    rtt_ms: e.rtt_ms,
                    is_fna: u8::from(e.is_fna),
                    ipv4: e.ipv4.clone(),
                    port: e.port.map(u32::from),
                })
                .collect(),
        }),
    }
}

/// A setup failure the bridge raises itself. Always `Backend`: the plugin
/// link is up, what failed is building the request.
fn backend(detail: &str) -> vc::MediaSetupError {
    vc::MediaSetupError::Backend(detail.to_owned())
}

/// Projects one control-plane command. `None` is a variant this bridge
/// predates; the caller refuses rather than guesses.
#[allow(clippy::too_many_lines)]
pub fn command_to_abi(cmd: &vc::MediaCommand) -> Option<abi::MediaCommand> {
    match cmd {
        vc::MediaCommand::EnableVideo { awaiting_accept } => Some(if *awaiting_accept {
            abi::MediaCommand::VideoEnableAwaitingAccept
        } else {
            abi::MediaCommand::VideoEnable
        }),
        vc::MediaCommand::DisableVideo { keep_legacy } => Some(if *keep_legacy {
            abi::MediaCommand::VideoDisableKeepLegacy
        } else {
            abi::MediaCommand::VideoDisable
        }),
        vc::MediaCommand::DisableVideoOutbound => Some(abi::MediaCommand::VideoDisableOutbound),
        vc::MediaCommand::RequireVideoKeyframe => Some(abi::MediaCommand::VideoRequireKeyframe),
        vc::MediaCommand::RequestPeerKeyframe(u) => Some(
            abi::MediaCommand::VideoRequestPeerKeyframe(map_urgency(*u)?),
        ),
        vc::MediaCommand::SetVideoOrientation {
            participant,
            orientation,
        } => Some(abi::MediaCommand::VideoSetOrientation {
            participant: participant.as_ref().map(Jid::to_string),
            orientation: *orientation,
        }),
        vc::MediaCommand::SetVideoInputGeneration(g) => {
            Some(abi::MediaCommand::VideoSetInputGeneration(*g))
        }
        vc::MediaCommand::SetVideoTimestampStride(s) => {
            Some(abi::MediaCommand::VideoSetTimestampStride(*s))
        }
        vc::MediaCommand::RekeyRecv {
            answering_lid,
            audio_codec,
        } => Some(abi::MediaCommand::RekeyRecv {
            answering_lid: answering_lid.clone(),
            audio_codec: match audio_codec {
                None => None,
                Some(codec) => Some(map_audio_codec(*codec)?),
            },
        }),
        vc::MediaCommand::ApplyGroupUpdate(update) => Some(abi::MediaCommand::GroupApplyUpdate(
            group_update_to_dto(update),
        )),
        vc::MediaCommand::ApplyGroupTransition(t) => {
            Some(abi::MediaCommand::GroupApplyTransition {
                update: group_update_to_dto(&t.update),
                transaction_id: t.transaction_id,
                epoch: abi::SecretBytes::new(t.raw_epoch.as_bytes().to_vec()),
            })
        }
        vc::MediaCommand::ApplyGroupEpoch {
            transaction_id,
            raw_epoch,
        } => Some(abi::MediaCommand::GroupApplyEpoch {
            transaction_id: *transaction_id,
            epoch: abi::SecretBytes::new(raw_epoch.as_bytes().to_vec()),
        }),
        vc::MediaCommand::SendGroupReaction(emoji) => {
            Some(abi::MediaCommand::GroupSendReaction(emoji.clone()))
        }
        _ => None,
    }
}

/// Projects one drive-loop video command. Same fail-closed `None`.
pub fn video_control_to_abi(cmd: &vc::VideoControl) -> Option<abi::MediaCommand> {
    match cmd {
        vc::VideoControl::SetInputGeneration(g) => {
            Some(abi::MediaCommand::VideoSetInputGeneration(*g))
        }
        vc::VideoControl::SetTimestampStride(s) => {
            Some(abi::MediaCommand::VideoSetTimestampStride(*s))
        }
        vc::VideoControl::Enable => Some(abi::MediaCommand::VideoEnable),
        vc::VideoControl::EnableAwaitingAccept => {
            Some(abi::MediaCommand::VideoEnableAwaitingAccept)
        }
        vc::VideoControl::Disable => Some(abi::MediaCommand::VideoDisable),
        vc::VideoControl::DisableOutbound => Some(abi::MediaCommand::VideoDisableOutbound),
        vc::VideoControl::DisableKeepLegacy => Some(abi::MediaCommand::VideoDisableKeepLegacy),
        vc::VideoControl::RequireKeyframe => Some(abi::MediaCommand::VideoRequireKeyframe),
        vc::VideoControl::RequestPeerKeyframe(u) => Some(
            abi::MediaCommand::VideoRequestPeerKeyframe(map_urgency(*u)?),
        ),
        vc::VideoControl::SetOrientation(o) => Some(abi::MediaCommand::VideoSetOrientation {
            participant: None,
            orientation: *o,
        }),
        vc::VideoControl::SetParticipantOrientation {
            participant,
            orientation,
        } => Some(abi::MediaCommand::VideoSetOrientation {
            participant: Some(participant.to_string()),
            orientation: *orientation,
        }),
        _ => None,
    }
}

/// Maps the plugin's typed failure into the setup grammar: transport stays
/// `Connect`, everything else is setup. No stacking: one code, one detail.
pub fn setup_error(code: abi::AbiErrorCode, detail: Option<&str>) -> vc::MediaSetupError {
    let text = detail
        .filter(|d| !d.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| code.name().to_owned());
    match code {
        abi::AbiErrorCode::Transport => vc::MediaSetupError::Connect(text),
        _ => vc::MediaSetupError::Backend(text),
    }
}

/// Projects a control-plane close reason onto the wire.
pub fn close_reason_to_abi(reason: &vc::MediaCloseReason) -> (abi::CloseReason, Option<String>) {
    match reason {
        vc::MediaCloseReason::Local => (abi::CloseReason::LocalHangup, None),
        vc::MediaCloseReason::RelayDisconnected => (abi::CloseReason::RelayDropped, None),
        vc::MediaCloseReason::SendFailed(detail) => {
            (abi::CloseReason::Failed, Some(detail.clone()))
        }
        vc::MediaCloseReason::SetupFailed(detail) => {
            (abi::CloseReason::Failed, Some(detail.clone()))
        }
        _ => (abi::CloseReason::Failed, None),
    }
}

/// Reads an engine-side ending into the session's close reason. The wire has
/// no relay variant for a remote hangup and no detail channel beyond the
/// failure text, so a remote end lands where a local teardown does and a
/// failure names its wire reason.
pub fn close_reason_from_abi(
    reason: abi::CloseReason,
    detail: Option<&str>,
) -> vc::MediaCloseReason {
    match reason {
        abi::CloseReason::LocalHangup
        | abi::CloseReason::RemoteEnd
        | abi::CloseReason::Replaced => vc::MediaCloseReason::Local,
        abi::CloseReason::RelayDropped => vc::MediaCloseReason::RelayDisconnected,
        abi::CloseReason::Failed | abi::CloseReason::Timeout => vc::MediaCloseReason::SetupFailed(
            detail
                .filter(|d| !d.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| reason.name().to_owned()),
        ),
    }
}

/// Projects one engine-raised event into the public stream vocabulary. The
/// negotiated audio format fills the timing the wire omits per packet.
/// `None` is a JID the core would not parse or a variant this bridge
/// predates; the caller drops the event rather than invents it.
pub fn event_to_call_event(
    ev: &abi::AbiEvent,
    format: &vc::MediaAudioFormat,
) -> Option<vc::MediaEvent> {
    match ev {
        abi::AbiEvent::RelayAllocated => Some(vc::MediaEvent::RelayAllocated),
        abi::AbiEvent::ForeignAudio(data) => {
            Some(vc::MediaEvent::ForeignAudio(Bytes::from(data.clone())))
        }
        abi::AbiEvent::ForeignGroupAudio(frame) => Some(vc::MediaEvent::ForeignGroupAudio(
            encoded_frame(frame, format)?,
        )),
        abi::AbiEvent::AudioFormatMismatch {
            expected_rate,
            received_rates,
        } => Some(vc::MediaEvent::AudioFormatMismatch {
            expected_rate: *expected_rate,
            received_rates: received_rates.clone(),
        }),
        abi::AbiEvent::RelayAllocateFailed(code) => Some(vc::MediaEvent::RelayAllocateFailed(
            u16::try_from(*code).unwrap_or(u16::MAX),
        )),
        abi::AbiEvent::RelayAllocateTimedOut => Some(vc::MediaEvent::RelayAllocateTimedOut),
        abi::AbiEvent::MediaSetupFailed(detail) => {
            Some(vc::MediaEvent::MediaSetupFailed(detail.clone()))
        }
        abi::AbiEvent::RelayReconnectTimedOut => Some(vc::MediaEvent::RelayReconnectTimedOut),
        abi::AbiEvent::VideoKeyframeNeeded => Some(vc::MediaEvent::VideoKeyframeNeeded),
        abi::AbiEvent::RtcpReceived {
            packet_types,
            sender_ssrc,
            referenced_ssrcs,
            reports_audio,
            reports_video,
            report_blocks,
            feedback,
        } => Some(vc::MediaEvent::RtcpReceived {
            packet_types: packet_types.clone(),
            sender_ssrc: *sender_ssrc,
            referenced_ssrcs: referenced_ssrcs.clone(),
            reports_audio: *reports_audio == 1,
            reports_video: *reports_video == 1,
            report_blocks: report_blocks
                .iter()
                .map(|b| {
                    vc::MediaRtcpReportBlock::builder()
                        .ssrc(b.ssrc)
                        .fraction_lost(b.fraction_lost)
                        .cumulative_lost(b.cumulative_lost)
                        .extended_highest_sequence(b.extended_highest_sequence)
                        .jitter(b.jitter)
                        .last_sender_report(b.last_sender_report)
                        .delay_since_last_sender_report(b.delay_since_last_sender_report)
                        .profile_extension(b.profile_extension.clone())
                        .build()
                })
                .collect(),
            feedback: feedback
                .iter()
                .map(|f| {
                    vc::MediaRtcpFeedback::builder()
                        .packet_type(f.packet_type)
                        .fmt(f.fmt)
                        .sender_ssrc(f.sender_ssrc)
                        .media_ssrc(f.media_ssrc)
                        .fci(f.fci.clone())
                        .build()
                })
                .collect(),
        }),
        abi::AbiEvent::OutboundMediaDropped {
            video_access_units,
            packets,
        } => Some(vc::MediaEvent::OutboundMediaDropped {
            video_access_units: *video_access_units,
            packets: *packets,
        }),
        abi::AbiEvent::AudioSilent {
            silent_for_ms,
            rtp_received,
            frames_produced,
            dominant_reason,
        } => Some(vc::MediaEvent::AudioSilent {
            silent_for_ms: *silent_for_ms,
            rtp_received: *rtp_received,
            frames_produced: *frames_produced,
            dominant_reason: map_silence_reason(*dominant_reason)?,
        }),
        abi::AbiEvent::AudioCodecSwitched {
            from,
            to,
            source,
            packets_observed,
        } => Some(vc::MediaEvent::AudioCodecSwitched {
            from: unmap_audio_codec(*from),
            to: unmap_audio_codec(*to),
            source: map_codec_source(*source)?,
            packets_observed: *packets_observed,
        }),
        abi::AbiEvent::AudioCodecSourceIsFixed {
            sending,
            peer_expects,
            source,
        } => Some(vc::MediaEvent::AudioCodecSourceIsFixed {
            sending: unmap_audio_codec(*sending),
            peer_expects: unmap_audio_codec(*peer_expects),
            source: map_codec_source(*source)?,
        }),
        abi::AbiEvent::AudioReceptionStalled { silent_for_ms } => {
            Some(vc::MediaEvent::AudioReceptionStalled {
                silent_for_ms: *silent_for_ms,
            })
        }
        // `Closed` never becomes an event here: the session raises it
        // itself when the media ends, on both the push and close paths.
        abi::AbiEvent::Closed(_) => None,
    }
}

/// Builds the core's encoded frame around wire bytes, with the negotiated
/// format and parsed sender identities.
fn encoded_frame(
    frame: &abi::EncodedFrameDto,
    format: &vc::MediaAudioFormat,
) -> Option<vc::MediaEncodedFrame> {
    Some(
        vc::MediaEncodedFrame::builder()
            .format(*format)
            .codec(unmap_audio_codec(frame.codec))
            .data(Bytes::from(frame.data.clone()))
            .payload_type(frame.payload_type)
            // RTP sequence arithmetic is modulo 2^16; wrapping is the
            // semantic, not a truncation.
            .sequence_number(frame.sequence_number as u16)
            .timestamp(frame.timestamp)
            .marker(frame.marker == 1)
            .maybe_sender(frame.sender.as_deref().map(str::parse).transpose().ok()?)
            .maybe_device(frame.device.as_deref().map(str::parse).transpose().ok()?)
            .build(),
    )
}

/// Projects the silence reason.
fn map_silence_reason(reason: abi::SilenceReason) -> Option<vc::MediaSilenceReason> {
    match reason {
        abi::SilenceReason::NoDecoderForNegotiatedCodec => {
            Some(vc::MediaSilenceReason::NoDecoderForNegotiatedCodec)
        }
        abi::SilenceReason::AuthenticationFailing => {
            Some(vc::MediaSilenceReason::AuthenticationFailing)
        }
        abi::SilenceReason::UnexpectedPayloadType => {
            Some(vc::MediaSilenceReason::UnexpectedPayloadType)
        }
        abi::SilenceReason::CodecRejectingFrames => {
            Some(vc::MediaSilenceReason::CodecRejectingFrames)
        }
        abi::SilenceReason::CodecFlapping => Some(vc::MediaSilenceReason::CodecFlapping),
        abi::SilenceReason::Unknown => Some(vc::MediaSilenceReason::Unknown),
    }
}

/// Projects the codec-switch source.
fn map_codec_source(source: abi::CodecSource) -> Option<vc::MediaCodecDecisionSource> {
    match source {
        abi::CodecSource::Negotiated => Some(vc::MediaCodecDecisionSource::Negotiated),
        abi::CodecSource::Content => Some(vc::MediaCodecDecisionSource::Content),
    }
}

/// Projects pushed counters into the cached snapshot. `codec_switches` is
/// `u16` upstream; saturation keeps a runaway counter from wrapping the
/// snapshot it lands in.
pub fn stats_to_core(stats: &abi::StatsData) -> vc::MediaStats {
    vc::MediaStats::builder()
        .rtp_received(stats.rtp_received)
        .rtp_payload_type_unexpected(stats.rtp_payload_type_unexpected)
        .srtp_unprotect_failed(stats.srtp_unprotect_failed)
        .sframe_decrypt_failed(stats.sframe_decrypt_failed)
        .audio_frames_decoded(stats.audio_frames_decoded)
        .audio_frames_delivered(stats.audio_frames_delivered)
        .audio_frames_concealed(stats.audio_frames_concealed)
        .mlow_off_point_dropped(stats.mlow_off_point_dropped)
        .mlow_inactive_or_sid(stats.mlow_inactive_or_sid)
        .foreign_frames_decoded(stats.foreign_frames_decoded)
        .audio_frames_without_decoder(stats.audio_frames_without_decoder)
        .outbound_frames_without_encoder(stats.outbound_frames_without_encoder)
        .playout_trimmed_samples(stats.playout_trimmed_samples)
        .inbound_pipe_dropped(stats.inbound_pipe_dropped)
        .audio_sink_dropped(stats.audio_sink_dropped)
        .video_sink_dropped(stats.video_sink_dropped)
        .peer_keyframe_requests(stats.peer_keyframe_requests)
        .relay_packet_unclassified(stats.relay_packet_unclassified)
        .forwarding_envelope_rejected(stats.forwarding_envelope_rejected)
        .codec_switches(u16::try_from(stats.codec_switches).unwrap_or(u16::MAX))
        .build()
}
