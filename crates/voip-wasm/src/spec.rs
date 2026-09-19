//! `OpenParams` back into the neutral spec the engine builds from.
//!
//! The core performs the forward projection in `src/voip/abi.rs`
//! (`params_from_spec`): a `MediaSessionSpec` plus its opening context
//! becomes `OpenParams`. This module is the inverse, field for field: the
//! direction, audio, relay, key and flag fields map back onto the same
//! neutral types, so `into_engine_parts` receives the spec the core meant.
//! Anything the wire cannot name (a future enum variant, a JID the core
//! would not parse) refuses with the field that was wrong rather than a
//! guess.

use voip_abi::{
    AudioCodecWire, AudioFormatDto, AudioIo, Direction, GroupCallUpdateDto, GroupOpenSpec,
    OpenParams, RtpProfile,
};
use wacore::voip_control::{
    CallDirection, MediaAudioCodec, MediaAudioFormat, MediaAudioIo, MediaAudioRtpProfile,
    MediaAudioSpec, MediaDirectPeer, MediaGroupSpec, MediaSessionKey, MediaSessionSpec,
    MediaSetupError,
};
use wacore_binary::Jid;

/// Rebuilds the neutral spec from decoded open params. The call identity
/// comes from `call_id` (reserved under this handle), not from the params:
/// the wire carries no generation, and inventing one would break the ABA
/// guard the session table enforces.
///
/// Unmappable fields refuse as `Backend` with the wire value named: the
/// relay endpoint and the call key have their own variants because the
/// engine validates them on the way in, but a JID string or a group DTO
/// the core would not parse is this side's refusal, not the engine's.
pub fn spec_from_params(
    params: OpenParams,
    call_id: &str,
    generation: u64,
) -> Result<(MediaSessionSpec, OpenCtx), MediaSetupError> {
    let direction = match params.direction {
        Direction::Outgoing => CallDirection::Outgoing,
        Direction::Incoming => CallDirection::Incoming,
    };
    let io = match params.audio_io {
        AudioIo::Pcm => MediaAudioIo::Pcm,
        AudioIo::Encoded => MediaAudioIo::Encoded,
    };
    let format = format_from_dto(&params.audio_format)?;
    let group = params
        .group
        .clone()
        .map(group_spec_from_dto)
        .transpose()
        .map_err(MediaSetupError::Backend)?;
    let ctx = OpenCtx {
        muted: params.muted != 0,
        initial_codec: params.initial_codec.map(codec_from_wire),
        peer_orientations: params.peer_orientations.clone(),
        epoch: params.group.as_ref().and_then(|g| {
            match (g.epoch_transaction_id, g.epoch.as_ref()) {
                (Some(tx), Some(epoch)) => Some((tx, epoch.as_bytes().to_vec())),
                _ => None,
            }
        }),
    };
    let relay_port = u16::try_from(params.relay_port).map_err(|_| MediaSetupError::BadEndpoint)?;
    Ok((
        MediaSessionSpec::builder()
            .key(
                MediaSessionKey::builder()
                    .call_id(call_id.to_owned())
                    .generation(generation)
                    .build(),
            )
            .direction(direction)
            .self_lid(params.self_lid)
            .peer_lid(params.peer_lid)
            .call_key(params.call_key.as_bytes().to_vec())
            .ssrc(params.ssrc)
            .audio(MediaAudioSpec::builder().format(format).io(io).build())
            .relay_token(params.relay_token.as_bytes().to_vec())
            .auth_token(params.auth_token.as_bytes().to_vec())
            .relay_ip(params.relay_host)
            .relay_port(relay_port)
            .integrity_key(params.integrity_key.as_bytes().to_vec())
            .warp_mi_tag_len(params.warp_mi_tag_len as usize)
            .enable_media(params.enable_media != 0)
            .enable_video(params.enable_video != 0)
            .enable_sframe(params.enable_sframe != 0)
            .maybe_group(group)
            .build(),
        ctx,
    ))
}

/// The opening context that rode beside the spec on the core side: the mute
/// flag, the caller-selected codec, the pre-attach peer rotations, and the
/// pre-authenticated group epoch, if any.
pub struct OpenCtx {
    pub muted: bool,
    pub initial_codec: Option<MediaAudioCodec>,
    /// Rotations peers announced before media attached, replayed as
    /// orientation commands once the drive loop owns the video mailbox.
    pub peer_orientations: Vec<(Option<String>, u8)>,
    pub epoch: Option<(u32, Vec<u8>)>,
}

fn codec_from_wire(codec: AudioCodecWire) -> MediaAudioCodec {
    match codec {
        AudioCodecWire::Mlow => MediaAudioCodec::Mlow,
        AudioCodecWire::Opus => MediaAudioCodec::Opus,
    }
}

fn format_from_dto(dto: &AudioFormatDto) -> Result<MediaAudioFormat, MediaSetupError> {
    let codec = codec_from_wire(dto.codec);
    let rtp_profile = match dto.rtp_profile {
        RtpProfile::Mlow => MediaAudioRtpProfile::Mlow,
        RtpProfile::StandardOpus => MediaAudioRtpProfile::StandardOpus,
    };
    Ok(MediaAudioFormat::builder()
        .codec(codec)
        .rtp_profile(rtp_profile)
        .signaling_rate(dto.signaling_rate)
        .sample_rate(dto.sample_rate)
        .channels(dto.channels)
        .samples_per_frame(dto.samples_per_frame)
        .rtp_clock_rate(dto.rtp_clock_rate)
        .rtp_timestamp_step(dto.rtp_timestamp_step)
        .rtp_payload_type(dto.rtp_payload_type)
        .build())
}

fn group_spec_from_dto(spec: GroupOpenSpec) -> Result<MediaGroupSpec, String> {
    let parse_jid = |raw: &str| {
        raw.parse::<Jid>()
            .map_err(|_| format!("group JID this bridge predates: {raw}"))
    };
    let update = update_from_dto(&spec.initial_update, &parse_jid)?;
    let direct_peer = spec
        .direct_peer
        .map(|peer| {
            Ok::<_, String>(
                MediaDirectPeer::builder()
                    .user_jid(parse_jid(&peer.user_jid)?)
                    .device_jid(parse_jid(&peer.device_jid)?)
                    .call_key(peer.call_key.as_bytes().to_vec())
                    .build(),
            )
        })
        .transpose()?;
    Ok(MediaGroupSpec::builder()
        .call_creator(parse_jid(&spec.call_creator)?)
        .self_jid(parse_jid(&spec.self_jid)?)
        .initial_update(update)
        .maybe_direct_peer(direct_peer)
        .build())
}

fn update_from_dto(
    dto: &GroupCallUpdateDto,
    parse_jid: &dyn Fn(&str) -> Result<Jid, String>,
) -> Result<wacore::types::group_call::GroupCallUpdate, String> {
    use wacore::types::group_call::{
        GroupCallDevice, GroupCallParticipant, GroupCallRelay, GroupCallRelayEndpoint,
        GroupCallUpdate,
    };
    let participants = dto
        .participants
        .iter()
        .map(|p| {
            Ok::<_, String>(
                GroupCallParticipant::builder()
                    .jid(parse_jid(&p.jid)?)
                    .maybe_pn(p.pn.as_deref().map(parse_jid).transpose()?)
                    .maybe_state(p.state.clone())
                    .maybe_participant_type(p.participant_type.clone())
                    .devices(
                        p.devices
                            .iter()
                            .map(|d| {
                                Ok::<_, String>(
                                    GroupCallDevice::builder()
                                        .jid(parse_jid(&d.jid)?)
                                        .maybe_platform(d.platform.clone())
                                        .maybe_pid(d.pid)
                                        .maybe_capability_version(d.capability_version)
                                        .build(),
                                )
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                    .build(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let relay = dto
        .relay
        .as_ref()
        .map(|r| {
            Ok::<_, String>(
                GroupCallRelay::builder()
                    .maybe_transaction_id(r.transaction_id)
                    .maybe_self_pid(r.self_pid)
                    .uuid(r.uuid.clone())
                    .participant_uuid(r.participant_uuid.clone())
                    .attribute_padding(r.attribute_padding != 0)
                    .maybe_warp_mi_tag_len(r.warp_mi_tag_len)
                    .endpoints(
                        r.endpoints
                            .iter()
                            .map(|e| {
                                GroupCallRelayEndpoint::builder()
                                    .relay_id(e.relay_id)
                                    .token_id(e.token_id)
                                    .auth_token_id(e.auth_token_id)
                                    .relay_name(e.relay_name.clone())
                                    .maybe_domain_name(e.domain_name.clone())
                                    .maybe_rtt_ms(e.rtt_ms)
                                    .is_fna(e.is_fna != 0)
                                    .maybe_ipv4(e.ipv4.clone())
                                    .maybe_port(e.port.map(|p| p as u16))
                                    .build()
                            })
                            .collect(),
                    )
                    .build(),
            )
        })
        .transpose()?;
    Ok(GroupCallUpdate::builder()
        .call_id(dto.call_id.clone())
        .call_creator(parse_jid(&dto.call_creator)?)
        .maybe_group_jid(dto.group_jid.as_deref().map(parse_jid).transpose()?)
        .transaction_id(dto.transaction_id)
        .media(dto.media.clone())
        .connected_limit(dto.connected_limit)
        .joinable(dto.joinable != 0)
        .av_upgradable(dto.av_upgradable != 0)
        .rekey_requested(dto.rekey_requested != 0)
        .participants(participants)
        .maybe_relay(relay)
        .build())
}
