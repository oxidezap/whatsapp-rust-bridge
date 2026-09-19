//! Payload DTOs: one struct per message body, each with an exact `encode`
//! and `decode`. Decoders reject a short payload and ignore trailing bytes,
//! which is what lets a newer minor append fields.
//!
//! The DTOs project the core's neutral seam types (`MediaSessionSpec`,
//! `MediaCommand`, `CallEvent`, `MediaStats`, `GroupCallUpdate`) field for
//! field, so neither side invents a default the core did not supply. JIDs
//! cross as the strings the core's own `Display` renders; secrets cross as
//! [`SecretBytes`].

use crate::codec::{DecodeError, Reader, Writer};
use crate::protocol::AbiErrorCode;
use crate::secret::SecretBytes;
use crate::session::SessionId;

/// Parses one discriminant byte through `parse`, naming the field on failure.
fn enum_byte<T>(
    field: &'static str,
    r: &mut Reader<'_>,
    parse: impl FnOnce(u8) -> Option<T>,
) -> Result<T, DecodeError> {
    let raw = r.u8()?;
    parse(raw).ok_or(DecodeError::BadValue { field, value: raw })
}

/// Encodes just a session identity (handle + generation).
fn encode_session(session: SessionId) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32_le(session.handle);
    w.u64_le(session.generation);
    w.finish()
}

/// Decodes just a session identity.
fn decode_session(buf: &[u8]) -> Result<SessionId, DecodeError> {
    let mut r = Reader::new(buf);
    Ok(SessionId {
        handle: r.u32_le()?,
        generation: r.u64_le()?,
    })
}

/// `HELLO` request: who is calling, and what it can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloRequest {
    /// Which end of the wire speaks first.
    pub role: crate::protocol::Role,
    /// Capability bits offered.
    pub capabilities: u32,
}

impl HelloRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.role.as_u8());
        w.u32_le(self.capabilities);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let role = enum_byte("role", &mut r, crate::protocol::Role::from_u8)?;
        Ok(HelloRequest {
            role,
            capabilities: r.u32_le()?,
        })
    }
}

/// `HELLO` response: the capability bits the responder brings. The behavior
/// enabled is the intersection; the requester refuses anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloResponse {
    /// Capability bits offered back.
    pub capabilities: u32,
}

impl HelloResponse {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.capabilities);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(HelloResponse {
            capabilities: r.u32_le()?,
        })
    }
}

/// Call direction, mirroring the core's neutral enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    /// This side placed the call.
    Outgoing = 0,
    /// This side received the call.
    Incoming = 1,
}

impl Direction {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Direction::Outgoing),
            1 => Some(Direction::Incoming),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            Direction::Outgoing => "OUTGOING",
            Direction::Incoming => "INCOMING",
        }
    }
}

/// `RESERVE` request: binds a handle to `(call_id, generation, direction)`
/// before any media message names it. Reserving a handle that is still
/// reserved fails; replacement is `CLOSE` then `RESERVE`, never an overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveRequest {
    /// The session identity being bound.
    pub session: SessionId,
    /// The WhatsApp call id this handle stands for.
    pub call_id: String,
    /// Which way the call flows.
    pub direction: Direction,
}

impl ReserveRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.utf8(&self.call_id);
        w.u8(self.direction.as_u8());
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let call_id = r.utf8()?.to_owned();
        let direction = enum_byte("direction", &mut r, Direction::from_u8)?;
        Ok(ReserveRequest {
            session,
            call_id,
            direction,
        })
    }
}

/// Where encoding and decoding happen, mirroring the core's audio I/O mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AudioIo {
    /// 16-bit PCM frames cross; the engine converts with its codec.
    Pcm = 0,
    /// Complete codec payloads cross, transcoded nowhere.
    Encoded = 1,
}

impl AudioIo {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(AudioIo::Pcm),
            1 => Some(AudioIo::Encoded),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            AudioIo::Pcm => "PCM",
            AudioIo::Encoded => "ENCODED",
        }
    }
}

/// Audio codec, mirroring the core's neutral codec enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AudioCodecWire {
    /// MLOW codec audio.
    Mlow = 1,
    /// Opus audio.
    Opus = 2,
}

impl AudioCodecWire {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(AudioCodecWire::Mlow),
            2 => Some(AudioCodecWire::Opus),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            AudioCodecWire::Mlow => "MLOW",
            AudioCodecWire::Opus => "OPUS",
        }
    }
}

/// RTP payload family, mirroring the core's neutral profile enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RtpProfile {
    /// WhatsApp MLOW framing.
    Mlow = 0,
    /// Native Opus bytes.
    StandardOpus = 1,
}

impl RtpProfile {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(RtpProfile::Mlow),
            1 => Some(RtpProfile::StandardOpus),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            RtpProfile::Mlow => "MLOW",
            RtpProfile::StandardOpus => "STANDARD_OPUS",
        }
    }
}

/// Fixed audio timing for one call, mirroring the core's audio format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormatDto {
    /// Codec of the payloads.
    pub codec: AudioCodecWire,
    /// RTP family negotiated with the peer.
    pub rtp_profile: RtpProfile,
    /// `<audio rate=…>` value used by call signaling.
    pub signaling_rate: u32,
    /// PCM rate expected by a codec adapter.
    pub sample_rate: u32,
    /// Channels (1 for mono).
    pub channels: u8,
    /// PCM samples per encoded payload, per channel.
    pub samples_per_frame: u32,
    /// RTP clock rate.
    pub rtp_clock_rate: u32,
    /// RTP clock increment per access unit.
    pub rtp_timestamp_step: u32,
    /// RTP payload type.
    pub rtp_payload_type: u8,
}

impl AudioFormatDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.codec.as_u8());
        w.u8(self.rtp_profile.as_u8());
        w.u32_le(self.signaling_rate);
        w.u32_le(self.sample_rate);
        w.u8(self.channels);
        w.u32_le(self.samples_per_frame);
        w.u32_le(self.rtp_clock_rate);
        w.u32_le(self.rtp_timestamp_step);
        w.u8(self.rtp_payload_type);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let codec = enum_byte("codec", &mut r, AudioCodecWire::from_u8)?;
        let rtp_profile = enum_byte("rtp_profile", &mut r, RtpProfile::from_u8)?;
        Ok(AudioFormatDto {
            codec,
            rtp_profile,
            signaling_rate: r.u32_le()?,
            sample_rate: r.u32_le()?,
            channels: r.u8()?,
            samples_per_frame: r.u32_le()?,
            rtp_clock_rate: r.u32_le()?,
            rtp_timestamp_step: r.u32_le()?,
            rtp_payload_type: r.u8()?,
        })
    }
}

/// Video direction bits in the open params.
pub struct VideoCaps;

impl VideoCaps {
    /// This side may send video.
    pub const SEND: u8 = 0x01;
    /// This side may receive video.
    pub const RECV: u8 = 0x02;
}

/// One direct peer inside a group open, mirroring the core's direct-peer
/// record. The callKey is secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectPeerDto {
    /// User JID string.
    pub user_jid: String,
    /// Device JID string.
    pub device_jid: String,
    /// Direct-call callKey (secret).
    pub call_key: SecretBytes,
}

impl DirectPeerDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.utf8(&self.user_jid);
        w.utf8(&self.device_jid);
        w.bytes_raw(self.call_key.as_bytes());
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(DirectPeerDto {
            user_jid: r.utf8()?.to_owned(),
            device_jid: r.utf8()?.to_owned(),
            call_key: SecretBytes::new(r.bytes_raw()?.to_vec()),
        })
    }
}

/// Group state for an open into a group call, mirroring the core's group
/// spec plus the already-authenticated epoch the caller fanned out, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupOpenSpec {
    /// Creator JID string.
    pub call_creator: String,
    /// Own JID string.
    pub self_jid: String,
    /// The roster snapshot the open starts from.
    pub initial_update: GroupCallUpdateDto,
    /// Direct peer, if any.
    pub direct_peer: Option<DirectPeerDto>,
    /// Transaction id of the pre-authenticated epoch, if any.
    pub epoch_transaction_id: Option<u32>,
    /// The pre-authenticated epoch itself (secret).
    pub epoch: Option<SecretBytes>,
}

impl GroupOpenSpec {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.utf8(&self.call_creator);
        w.utf8(&self.self_jid);
        let raw = self.initial_update.encode();
        w.bytes_raw(&raw);
        match &self.direct_peer {
            None => w.u8(0),
            Some(p) => {
                w.u8(1);
                let raw = p.encode();
                w.bytes_raw(&raw);
            }
        }
        match self.epoch_transaction_id {
            None => w.u8(0),
            Some(t) => {
                w.u8(1);
                w.u32_le(t);
            }
        }
        match &self.epoch {
            None => w.u8(0),
            Some(e) => {
                w.u8(1);
                w.bytes_raw(e.as_bytes());
            }
        }
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let call_creator = r.utf8()?.to_owned();
        let self_jid = r.utf8()?.to_owned();
        let initial_update = GroupCallUpdateDto::decode(r.bytes_raw()?)?;
        let direct_peer = match r.u8()? {
            0 => None,
            1 => Some(DirectPeerDto::decode(r.bytes_raw()?)?),
            b => {
                return Err(DecodeError::BadValue {
                    field: "direct_peer",
                    value: b,
                });
            }
        };
        let epoch_transaction_id = match r.u8()? {
            0 => None,
            1 => Some(r.u32_le()?),
            b => {
                return Err(DecodeError::BadValue {
                    field: "epoch_transaction_id",
                    value: b,
                });
            }
        };
        let epoch = match r.u8()? {
            0 => None,
            1 => Some(SecretBytes::new(r.bytes_raw()?.to_vec())),
            b => {
                return Err(DecodeError::BadValue {
                    field: "epoch",
                    value: b,
                });
            }
        };
        Ok(GroupOpenSpec {
            call_creator,
            self_jid,
            initial_update,
            direct_peer,
            epoch_transaction_id,
            epoch,
        })
    }
}

/// What `BEGIN_OPEN` carries: the flat media-session spec as plain data, so
/// the engine side can build the same media the resident backend would. Key
/// material travels as [`SecretBytes`] and is zeroed on drop on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenParams {
    /// Which way the call flows.
    pub direction: Direction,
    /// This side's LID.
    pub self_lid: String,
    /// The peer's LID.
    pub peer_lid: String,
    /// RTP SSRC.
    pub ssrc: u32,
    /// Where encoding and decoding happen.
    pub audio_io: AudioIo,
    /// Fixed audio timing for the call.
    pub audio_format: AudioFormatDto,
    /// The STUN `RELAY-TOKEN` attribute (secret).
    pub relay_token: SecretBytes,
    /// The `<auth_token>` for the synthetic SDP `ice-ufrag` (secret).
    pub auth_token: SecretBytes,
    /// The 32-byte callKey (secret).
    pub call_key: SecretBytes,
    /// Relay host, as text.
    pub relay_host: String,
    /// Relay port.
    pub relay_port: u32,
    /// The relay `<key>` for STUN MESSAGE-INTEGRITY (secret).
    pub integrity_key: SecretBytes,
    /// WARP integrity tag length.
    pub warp_mi_tag_len: u32,
    /// Bring up the audio path: 0 or 1.
    pub enable_media: u8,
    /// Bring up the video plane: 0 or 1.
    pub enable_video: u8,
    /// Negotiate SFrame: 0 or 1.
    pub enable_sframe: u8,
    /// Start muted: 0 or 1.
    pub muted: u8,
    /// [`VideoCaps`] bits.
    pub video: u8,
    /// A codec the caller selected that the engine adopts before its first
    /// packet, when the negotiated format is not already correct.
    pub initial_codec: Option<AudioCodecWire>,
    /// Rotations peers announced before media attached, in order:
    /// `(participant, orientation)`, with `None` for the peer itself.
    pub peer_orientations: Vec<(Option<String>, u8)>,
    /// Group state, present only for a group call.
    pub group: Option<GroupOpenSpec>,
}

impl OpenParams {
    /// Serializes the body. Field order is the wire order; append-only.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.direction.as_u8());
        w.utf8(&self.self_lid);
        w.utf8(&self.peer_lid);
        w.u32_le(self.ssrc);
        w.u8(self.audio_io.as_u8());
        let format = self.audio_format.encode();
        w.bytes_raw(&format);
        w.bytes_raw(self.relay_token.as_bytes());
        w.bytes_raw(self.auth_token.as_bytes());
        w.bytes_raw(self.call_key.as_bytes());
        w.utf8(&self.relay_host);
        w.u32_le(self.relay_port);
        w.bytes_raw(self.integrity_key.as_bytes());
        w.u32_le(self.warp_mi_tag_len);
        w.u8(self.enable_media);
        w.u8(self.enable_video);
        w.u8(self.enable_sframe);
        w.u8(self.muted);
        w.u8(self.video);
        match self.initial_codec {
            None => w.u8(0),
            Some(c) => {
                w.u8(1);
                w.u8(c.as_u8());
            }
        }
        w.u32_le(self.peer_orientations.len() as u32);
        for (participant, orientation) in &self.peer_orientations {
            w.option_str(participant.as_deref());
            w.u8(*orientation);
        }
        match &self.group {
            None => w.u8(0),
            Some(g) => {
                w.u8(1);
                let raw = g.encode();
                w.bytes_raw(&raw);
            }
        }
        w.finish()
    }

    /// Parses the body. Trailing bytes are ignored (newer minor fields).
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let direction = enum_byte("direction", &mut r, Direction::from_u8)?;
        let self_lid = r.utf8()?.to_owned();
        let peer_lid = r.utf8()?.to_owned();
        let ssrc = r.u32_le()?;
        let audio_io = enum_byte("audio_io", &mut r, AudioIo::from_u8)?;
        let format_raw = r.bytes_raw()?;
        let audio_format = AudioFormatDto::decode(format_raw)?;
        let relay_token = SecretBytes::new(r.bytes_raw()?.to_vec());
        let auth_token = SecretBytes::new(r.bytes_raw()?.to_vec());
        let call_key = SecretBytes::new(r.bytes_raw()?.to_vec());
        let relay_host = r.utf8()?.to_owned();
        let relay_port = r.u32_le()?;
        let integrity_key = SecretBytes::new(r.bytes_raw()?.to_vec());
        let warp_mi_tag_len = r.u32_le()?;
        let enable_media = r.u8()?;
        let enable_video = r.u8()?;
        let enable_sframe = r.u8()?;
        let muted = r.u8()?;
        for (field, v) in [
            ("enable_media", enable_media),
            ("enable_video", enable_video),
            ("enable_sframe", enable_sframe),
            ("muted", muted),
        ] {
            if v > 1 {
                return Err(DecodeError::BadValue { field, value: v });
            }
        }
        let video = r.u8()?;
        let initial_codec = match r.u8()? {
            0 => None,
            1 => Some(enum_byte("initial_codec", &mut r, AudioCodecWire::from_u8)?),
            b => {
                return Err(DecodeError::BadValue {
                    field: "initial_codec",
                    value: b,
                });
            }
        };
        let orientation_count = r.u32_le()?;
        if orientation_count > 4096 {
            return Err(DecodeError::BadLength(orientation_count));
        }
        let mut peer_orientations = Vec::with_capacity(orientation_count.min(32) as usize);
        for _ in 0..orientation_count {
            peer_orientations.push((r.option_str()?.map(str::to_owned), r.u8()?));
        }
        let group = match r.u8()? {
            0 => None,
            1 => Some(GroupOpenSpec::decode(r.bytes_raw()?)?),
            b => {
                return Err(DecodeError::BadValue {
                    field: "group",
                    value: b,
                });
            }
        };
        Ok(OpenParams {
            direction,
            self_lid,
            peer_lid,
            ssrc,
            audio_io,
            audio_format,
            relay_token,
            auth_token,
            call_key,
            relay_host,
            relay_port,
            integrity_key,
            warp_mi_tag_len,
            enable_media,
            enable_video,
            enable_sframe,
            muted,
            video,
            initial_codec,
            peer_orientations,
            group,
        })
    }
}

/// `BEGIN_OPEN` request: start asynchronous setup for a reserved session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginOpenRequest {
    /// The reserved session being opened.
    pub session: SessionId,
    /// What to open it with.
    pub params: OpenParams,
}

impl BeginOpenRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        let params = self.params.encode();
        w.bytes_raw(&params);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let raw = r.bytes_raw()?;
        Ok(BeginOpenRequest {
            session,
            params: OpenParams::decode(raw)?,
        })
    }
}

/// `OPEN` notification (`voip -> core`): setup completed, media is flowing.
/// Fire-and-forget; the session identity is the whole body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenNotification {
    /// The session that is now live.
    pub session: SessionId,
}

impl OpenNotification {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        encode_session(self.session)
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        Ok(OpenNotification {
            session: decode_session(buf)?,
        })
    }
}

/// What aborting an in-flight open found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CancelOutcome {
    /// Setup stopped before media flowed.
    Aborted = 1,
    /// Setup had already completed; the session is open.
    AlreadyOpen = 2,
    /// Nothing was in flight for this session.
    Unknown = 3,
}

impl CancelOutcome {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(CancelOutcome::Aborted),
            2 => Some(CancelOutcome::AlreadyOpen),
            3 => Some(CancelOutcome::Unknown),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            CancelOutcome::Aborted => "ABORTED",
            CancelOutcome::AlreadyOpen => "ALREADY_OPEN",
            CancelOutcome::Unknown => "UNKNOWN",
        }
    }
}

/// `CANCEL_OPEN` request: abort an in-flight `BEGIN_OPEN`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelOpenRequest {
    /// The session whose setup must stop.
    pub session: SessionId,
}

impl CancelOpenRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        encode_session(self.session)
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        Ok(CancelOpenRequest {
            session: decode_session(buf)?,
        })
    }
}

/// `CANCEL_OPEN` response: what the abort found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelOpenResponse {
    /// What aborting found.
    pub outcome: CancelOutcome,
}

impl CancelOpenResponse {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.outcome.as_u8());
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let outcome = enum_byte("outcome", &mut r, CancelOutcome::from_u8)?;
        Ok(CancelOpenResponse { outcome })
    }
}

/// Keyframe urgency for a peer-keyframe request, mirroring the core's
/// neutral urgency enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Urgency {
    /// Coalesce with the next scheduled request.
    Coalesced = 0,
    /// Ask immediately.
    Immediate = 1,
}

impl Urgency {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Urgency::Coalesced),
            1 => Some(Urgency::Immediate),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            Urgency::Coalesced => "COALESCED",
            Urgency::Immediate => "IMMEDIATE",
        }
    }
}

/// One media command, mirroring the core's `MediaCommand`. The discriminant
/// is the wire; each variant's fields follow in order, append-only per
/// variant.
///
/// `AudioMute` is the one command the core never emits: mute travels in the
/// core as a shared flag, which cannot cross WASMs, so the bridge forwards
/// flag changes as this command from its watcher. It is bridge-originated,
/// never mapped from a core `submit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaCommand {
    /// Bring the video plane up.
    VideoEnable,
    /// Bring the video plane up, holding outbound until the peer accepts.
    VideoEnableAwaitingAccept,
    /// Tear the video plane down.
    VideoDisable,
    /// Tear the video plane down, retaining legacy AUs for a reattach.
    VideoDisableKeepLegacy,
    /// Stop sending video while keeping inbound decoding.
    VideoDisableOutbound,
    /// Require the next outbound access unit to be an IDR.
    VideoRequireKeyframe,
    /// Ask the peer for a keyframe, with urgency.
    VideoRequestPeerKeyframe(Urgency),
    /// A device orientation: `None` is the peer itself, `Some` a routed
    /// group participant.
    VideoSetOrientation {
        /// Participant JID string, if any.
        participant: Option<String>,
        /// Rotation (0..3, x90 degrees).
        orientation: u8,
    },
    /// Select the source generation accepted by the input queue.
    VideoSetInputGeneration(u64),
    /// RTP clock increment per access unit.
    VideoSetTimestampStride(u32),
    /// Rekey the recv path to the answering device, adopting the audio codec
    /// its capability selected in the same step.
    RekeyRecv {
        /// Answering device LID.
        answering_lid: String,
        /// Codec to adopt, if the format changes with the rekey.
        audio_codec: Option<AudioCodecWire>,
    },
    /// A newer authoritative group roster/relay snapshot.
    GroupApplyUpdate(GroupCallUpdateDto),
    /// One roster snapshot and its decrypted epoch, indivisible.
    GroupApplyTransition {
        /// The roster snapshot.
        update: GroupCallUpdateDto,
        /// Transaction id pairing the two.
        transaction_id: u32,
        /// The decrypted epoch (secret).
        epoch: SecretBytes,
    },
    /// A decrypted epoch alone.
    GroupApplyEpoch {
        /// Transaction id.
        transaction_id: u32,
        /// The decrypted epoch (secret).
        epoch: SecretBytes,
    },
    /// One authenticated group reaction to broadcast.
    GroupSendReaction(String),
    /// Set the outbound mute flag (bridge-originated; see above).
    AudioMute(u8),
}

impl MediaCommand {
    /// The wire discriminant.
    pub fn kind_u8(&self) -> u8 {
        match self {
            MediaCommand::VideoEnable => 3,
            MediaCommand::VideoEnableAwaitingAccept => 4,
            MediaCommand::VideoDisable => 5,
            MediaCommand::VideoDisableKeepLegacy => 7,
            MediaCommand::VideoDisableOutbound => 6,
            MediaCommand::VideoRequireKeyframe => 13,
            MediaCommand::VideoRequestPeerKeyframe(_) => 11,
            MediaCommand::VideoSetOrientation { .. } => 10,
            MediaCommand::VideoSetInputGeneration(_) => 8,
            MediaCommand::VideoSetTimestampStride(_) => 9,
            MediaCommand::RekeyRecv { .. } => 12,
            MediaCommand::GroupApplyUpdate(_) => 20,
            MediaCommand::GroupApplyTransition { .. } => 21,
            MediaCommand::GroupApplyEpoch { .. } => 22,
            MediaCommand::GroupSendReaction(_) => 23,
            MediaCommand::AudioMute(_) => 30,
        }
    }

    /// Written-down name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            MediaCommand::VideoEnable => "VIDEO_ENABLE",
            MediaCommand::VideoEnableAwaitingAccept => "VIDEO_ENABLE_AWAITING_ACCEPT",
            MediaCommand::VideoDisable => "VIDEO_DISABLE",
            MediaCommand::VideoDisableKeepLegacy => "VIDEO_DISABLE_KEEP_LEGACY",
            MediaCommand::VideoDisableOutbound => "VIDEO_DISABLE_OUTBOUND",
            MediaCommand::VideoRequireKeyframe => "VIDEO_REQUIRE_KEYFRAME",
            MediaCommand::VideoRequestPeerKeyframe(_) => "VIDEO_REQUEST_PEER_KEYFRAME",
            MediaCommand::VideoSetOrientation { .. } => "VIDEO_SET_ORIENTATION",
            MediaCommand::VideoSetInputGeneration(_) => "VIDEO_SET_INPUT_GENERATION",
            MediaCommand::VideoSetTimestampStride(_) => "VIDEO_SET_TIMESTAMP_STRIDE",
            MediaCommand::RekeyRecv { .. } => "REKEY_RECV",
            MediaCommand::GroupApplyUpdate(_) => "GROUP_APPLY_UPDATE",
            MediaCommand::GroupApplyTransition { .. } => "GROUP_APPLY_TRANSITION",
            MediaCommand::GroupApplyEpoch { .. } => "GROUP_APPLY_EPOCH",
            MediaCommand::GroupSendReaction(_) => "GROUP_SEND_REACTION",
            MediaCommand::AudioMute(_) => "AUDIO_MUTE",
        }
    }

    /// Serializes discriminant plus fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.kind_u8());
        match self {
            MediaCommand::VideoSetInputGeneration(g) => w.u64_le(*g),
            MediaCommand::VideoSetTimestampStride(s) => w.u32_le(*s),
            MediaCommand::VideoRequestPeerKeyframe(u) => w.u8(u.as_u8()),
            MediaCommand::VideoSetOrientation {
                participant,
                orientation,
            } => {
                w.option_str(participant.as_deref());
                w.u8(*orientation);
            }
            MediaCommand::RekeyRecv {
                answering_lid,
                audio_codec,
            } => {
                w.utf8(answering_lid);
                match audio_codec {
                    None => w.u8(0),
                    Some(c) => {
                        w.u8(1);
                        w.u8(c.as_u8());
                    }
                }
            }
            MediaCommand::GroupApplyUpdate(update) => {
                let raw = update.encode();
                w.bytes_raw(&raw);
            }
            MediaCommand::GroupApplyTransition {
                update,
                transaction_id,
                epoch,
            } => {
                let raw = update.encode();
                w.bytes_raw(&raw);
                w.u32_le(*transaction_id);
                w.bytes_raw(epoch.as_bytes());
            }
            MediaCommand::GroupApplyEpoch {
                transaction_id,
                epoch,
            } => {
                w.u32_le(*transaction_id);
                w.bytes_raw(epoch.as_bytes());
            }
            MediaCommand::GroupSendReaction(emoji) => w.utf8(emoji),
            MediaCommand::AudioMute(m) => w.u8(*m),
            _ => {}
        }
        w.finish()
    }

    /// Parses discriminant plus fields.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let kind = r.u8()?;
        let cmd = match kind {
            3 => MediaCommand::VideoEnable,
            4 => MediaCommand::VideoEnableAwaitingAccept,
            5 => MediaCommand::VideoDisable,
            6 => MediaCommand::VideoDisableOutbound,
            7 => MediaCommand::VideoDisableKeepLegacy,
            8 => MediaCommand::VideoSetInputGeneration(r.u64_le()?),
            9 => MediaCommand::VideoSetTimestampStride(r.u32_le()?),
            10 => MediaCommand::VideoSetOrientation {
                participant: r.option_str()?.map(str::to_owned),
                orientation: r.u8()?,
            },
            11 => MediaCommand::VideoRequestPeerKeyframe(enum_byte(
                "urgency",
                &mut r,
                Urgency::from_u8,
            )?),
            12 => {
                let answering_lid = r.utf8()?.to_owned();
                let audio_codec = match r.u8()? {
                    0 => None,
                    1 => Some(enum_byte("audio_codec", &mut r, AudioCodecWire::from_u8)?),
                    b => {
                        return Err(DecodeError::BadValue {
                            field: "audio_codec",
                            value: b,
                        });
                    }
                };
                MediaCommand::RekeyRecv {
                    answering_lid,
                    audio_codec,
                }
            }
            13 => MediaCommand::VideoRequireKeyframe,
            20 => MediaCommand::GroupApplyUpdate(GroupCallUpdateDto::decode(r.bytes_raw()?)?),
            21 => {
                let update = GroupCallUpdateDto::decode(r.bytes_raw()?)?;
                MediaCommand::GroupApplyTransition {
                    update,
                    transaction_id: r.u32_le()?,
                    epoch: SecretBytes::new(r.bytes_raw()?.to_vec()),
                }
            }
            22 => MediaCommand::GroupApplyEpoch {
                transaction_id: r.u32_le()?,
                epoch: SecretBytes::new(r.bytes_raw()?.to_vec()),
            },
            23 => MediaCommand::GroupSendReaction(r.utf8()?.to_owned()),
            30 => {
                let muted = r.u8()?;
                if muted > 1 {
                    return Err(DecodeError::BadValue {
                        field: "muted",
                        value: muted,
                    });
                }
                MediaCommand::AudioMute(muted)
            }
            _ => {
                return Err(DecodeError::BadValue {
                    field: "command",
                    value: kind,
                });
            }
        };
        Ok(cmd)
    }
}

/// `COMMAND` request: one media command against a live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    /// The session being commanded.
    pub session: SessionId,
    /// The command.
    pub command: MediaCommand,
}

impl CommandRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        let cmd = self.command.encode();
        w.bytes_raw(&cmd);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let raw = r.bytes_raw()?;
        Ok(CommandRequest {
            session,
            command: MediaCommand::decode(raw)?,
        })
    }
}

/// One group-call device, mirroring the public fields of the core's device
/// record. The raw capability blob is crate-private upstream and does not
/// cross; the engine parses capabilities from the epoch path instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDeviceDto {
    /// Device JID string.
    pub jid: String,
    /// Announced platform, if any.
    pub platform: Option<String>,
    /// Relay participant id, if any.
    pub pid: Option<u32>,
    /// Capability version, if any.
    pub capability_version: Option<u32>,
}

impl GroupDeviceDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.utf8(&self.jid);
        w.option_str(self.platform.as_deref());
        encode_option_u32(&mut w, self.pid);
        encode_option_u32(&mut w, self.capability_version);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(GroupDeviceDto {
            jid: r.utf8()?.to_owned(),
            platform: r.option_str()?.map(str::to_owned),
            pid: decode_option_u32(&mut r, "pid")?,
            capability_version: decode_option_u32(&mut r, "capability_version")?,
        })
    }
}

/// Writes an `Option<u32>` as a 0/1 flag plus value.
fn encode_option_u32(w: &mut Writer, v: Option<u32>) {
    match v {
        None => w.u8(0),
        Some(n) => {
            w.u8(1);
            w.u32_le(n);
        }
    }
}

/// Reads an `Option<u32>` written by [`encode_option_u32`].
fn decode_option_u32(r: &mut Reader<'_>, field: &'static str) -> Result<Option<u32>, DecodeError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(r.u32_le()?)),
        b => Err(DecodeError::BadValue { field, value: b }),
    }
}

/// One group-call participant, mirroring the public fields of the core's
/// participant record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupParticipantDto {
    /// Participant JID string.
    pub jid: String,
    /// Phone-number alias, retained for routing, never exposed in events.
    pub pn: Option<String>,
    /// Presence state (`connected`, ...), if any.
    pub state: Option<String>,
    /// Participant type, if any.
    pub participant_type: Option<String>,
    /// Their devices.
    pub devices: Vec<GroupDeviceDto>,
}

impl GroupParticipantDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.utf8(&self.jid);
        w.option_str(self.pn.as_deref());
        w.option_str(self.state.as_deref());
        w.option_str(self.participant_type.as_deref());
        encode_dto_vec(&mut w, &self.devices, |w, d| {
            let raw = d.encode();
            w.bytes_raw(&raw);
        });
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(GroupParticipantDto {
            jid: r.utf8()?.to_owned(),
            pn: r.option_str()?.map(str::to_owned),
            state: r.option_str()?.map(str::to_owned),
            participant_type: r.option_str()?.map(str::to_owned),
            devices: decode_dto_vec(&mut r, GroupDeviceDto::decode)?,
        })
    }
}

/// Writes a length-prefixed vec with a per-item writer.
fn encode_dto_vec<T>(w: &mut Writer, items: &[T], mut one: impl FnMut(&mut Writer, &T)) {
    w.u32_le(items.len() as u32);
    for item in items {
        one(w, item);
    }
}

/// Reads a length-prefixed vec, bounding the count before allocating.
fn decode_dto_vec<T>(
    r: &mut Reader<'_>,
    mut one: impl FnMut(&[u8]) -> Result<T, DecodeError>,
) -> Result<Vec<T>, DecodeError> {
    let count = r.u32_le()?;
    if count > 4096 {
        return Err(DecodeError::BadLength(count));
    }
    let mut out = Vec::with_capacity(count.min(32) as usize);
    for _ in 0..count {
        out.push(one(r.bytes_raw()?)?);
    }
    Ok(out)
}

/// One address advertised by the shared group relay, mirroring the public
/// fields of the core's endpoint record. The resolved socket address is
/// crate-private upstream and does not cross.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRelayEndpointDto {
    /// Relay id.
    pub relay_id: u32,
    /// Token id.
    pub token_id: u32,
    /// Auth token id.
    pub auth_token_id: u32,
    /// Relay name.
    pub relay_name: String,
    /// Domain name, if any.
    pub domain_name: Option<String>,
    /// Measured RTT, if any.
    pub rtt_ms: Option<u32>,
    /// Whether this is a first-party relay: 0 or 1.
    pub is_fna: u8,
    /// IPv4 text, if any.
    pub ipv4: Option<String>,
    /// Port, if any.
    pub port: Option<u32>,
}

impl GroupRelayEndpointDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.relay_id);
        w.u32_le(self.token_id);
        w.u32_le(self.auth_token_id);
        w.utf8(&self.relay_name);
        w.option_str(self.domain_name.as_deref());
        encode_option_u32(&mut w, self.rtt_ms);
        w.u8(self.is_fna);
        w.option_str(self.ipv4.as_deref());
        encode_option_u32(&mut w, self.port);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        // Wire order: ids, names, rtt, flag, address, port.
        let relay_id = r.u32_le()?;
        let token_id = r.u32_le()?;
        let auth_token_id = r.u32_le()?;
        let relay_name = r.utf8()?.to_owned();
        let domain_name = r.option_str()?.map(str::to_owned);
        let rtt_ms = decode_option_u32(&mut r, "rtt_ms")?;
        let is_fna = r.u8()?;
        if is_fna > 1 {
            return Err(DecodeError::BadValue {
                field: "is_fna",
                value: is_fna,
            });
        }
        let ipv4 = r.option_str()?.map(str::to_owned);
        Ok(GroupRelayEndpointDto {
            relay_id,
            token_id,
            auth_token_id,
            relay_name,
            domain_name,
            rtt_ms,
            is_fna,
            ipv4,
            port: decode_option_u32(&mut r, "port")?,
        })
    }
}

/// Shared relay allocation in a group snapshot, mirroring the public fields
/// of the core's relay record. The relay keys and tokens are crate-private
/// upstream and do not cross; rotation arrives through the epoch path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRelayDto {
    /// Transaction id, if any.
    pub transaction_id: Option<u32>,
    /// Own participant id, if any.
    pub self_pid: Option<u32>,
    /// Relay uuid.
    pub uuid: String,
    /// Participant uuid.
    pub participant_uuid: String,
    /// Attribute padding: 0 or 1.
    pub attribute_padding: u8,
    /// WARP integrity tag length, if any.
    pub warp_mi_tag_len: Option<u32>,
    /// Advertised addresses.
    pub endpoints: Vec<GroupRelayEndpointDto>,
}

impl GroupRelayDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        encode_option_u32(&mut w, self.transaction_id);
        encode_option_u32(&mut w, self.self_pid);
        w.utf8(&self.uuid);
        w.utf8(&self.participant_uuid);
        w.u8(self.attribute_padding);
        encode_option_u32(&mut w, self.warp_mi_tag_len);
        encode_dto_vec(&mut w, &self.endpoints, |w, e| {
            let raw = e.encode();
            w.bytes_raw(&raw);
        });
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let transaction_id = decode_option_u32(&mut r, "transaction_id")?;
        let self_pid = decode_option_u32(&mut r, "self_pid")?;
        let uuid = r.utf8()?.to_owned();
        let participant_uuid = r.utf8()?.to_owned();
        let attribute_padding = r.u8()?;
        if attribute_padding > 1 {
            return Err(DecodeError::BadValue {
                field: "attribute_padding",
                value: attribute_padding,
            });
        }
        Ok(GroupRelayDto {
            transaction_id,
            self_pid,
            uuid,
            participant_uuid,
            attribute_padding,
            warp_mi_tag_len: decode_option_u32(&mut r, "warp_mi_tag_len")?,
            endpoints: decode_dto_vec(&mut r, GroupRelayEndpointDto::decode)?,
        })
    }
}

/// One transaction-ordered authoritative group-call snapshot, mirroring the
/// public fields of the core's update record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupCallUpdateDto {
    /// Call id.
    pub call_id: String,
    /// Creator JID string.
    pub call_creator: String,
    /// Group JID string, if any.
    pub group_jid: Option<String>,
    /// Transaction id.
    pub transaction_id: u32,
    /// Media description.
    pub media: String,
    /// Roster limit the engine was admitted under.
    pub connected_limit: u32,
    /// Joinable: 0 or 1.
    pub joinable: u8,
    /// AV-upgradable: 0 or 1.
    pub av_upgradable: u8,
    /// Rekey requested: 0 or 1.
    pub rekey_requested: u8,
    /// Authoritative roster.
    pub participants: Vec<GroupParticipantDto>,
    /// Shared relay allocation, if any.
    pub relay: Option<GroupRelayDto>,
}

impl GroupCallUpdateDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.utf8(&self.call_id);
        w.utf8(&self.call_creator);
        w.option_str(self.group_jid.as_deref());
        w.u32_le(self.transaction_id);
        w.utf8(&self.media);
        w.u32_le(self.connected_limit);
        w.u8(self.joinable);
        w.u8(self.av_upgradable);
        w.u8(self.rekey_requested);
        encode_dto_vec(&mut w, &self.participants, |w, p| {
            let raw = p.encode();
            w.bytes_raw(&raw);
        });
        match &self.relay {
            None => w.u8(0),
            Some(relay) => {
                w.u8(1);
                let raw = relay.encode();
                w.bytes_raw(&raw);
            }
        }
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let call_id = r.utf8()?.to_owned();
        let call_creator = r.utf8()?.to_owned();
        let group_jid = r.option_str()?.map(str::to_owned);
        let transaction_id = r.u32_le()?;
        let media = r.utf8()?.to_owned();
        let connected_limit = r.u32_le()?;
        let joinable = r.u8()?;
        let av_upgradable = r.u8()?;
        let rekey_requested = r.u8()?;
        for (field, v) in [
            ("joinable", joinable),
            ("av_upgradable", av_upgradable),
            ("rekey_requested", rekey_requested),
        ] {
            if v > 1 {
                return Err(DecodeError::BadValue { field, value: v });
            }
        }
        let participants = decode_dto_vec(&mut r, GroupParticipantDto::decode)?;
        let relay = match r.u8()? {
            0 => None,
            1 => Some(GroupRelayDto::decode(r.bytes_raw()?)?),
            b => {
                return Err(DecodeError::BadValue {
                    field: "relay",
                    value: b,
                });
            }
        };
        Ok(GroupCallUpdateDto {
            call_id,
            call_creator,
            group_jid,
            transaction_id,
            media,
            connected_limit,
            joinable,
            av_upgradable,
            rekey_requested,
            participants,
            relay,
        })
    }
}

/// `GROUP_FITS` request: asks whether the engine fits a committed roster,
/// charging a call-link admission like the pre-attach reservation does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupFitsRequest {
    /// The session asking.
    pub session: SessionId,
    /// The committed roster being probed.
    pub update: GroupCallUpdateDto,
    /// Whether this probes a call-link admission: 0 or 1.
    pub is_call_link: u8,
}

impl GroupFitsRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        let raw = self.update.encode();
        w.bytes_raw(&raw);
        w.u8(self.is_call_link);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let update = GroupCallUpdateDto::decode(r.bytes_raw()?)?;
        let is_call_link = r.u8()?;
        if is_call_link > 1 {
            return Err(DecodeError::BadValue {
                field: "is_call_link",
                value: is_call_link,
            });
        }
        Ok(GroupFitsRequest {
            session,
            update,
            is_call_link,
        })
    }
}

/// `GROUP_FITS` response: the verdict and the engine's own limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupFitsResponse {
    /// Whether the roster fits: 0 or 1.
    pub fits: u8,
    /// The engine's participant limit.
    pub limit: u32,
}

impl GroupFitsResponse {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.fits);
        w.u32_le(self.limit);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let fits = r.u8()?;
        if fits > 1 {
            return Err(DecodeError::BadValue {
                field: "fits",
                value: fits,
            });
        }
        Ok(GroupFitsResponse {
            fits,
            limit: r.u32_le()?,
        })
    }
}

/// Why a session or its media ended. One enum for `CLOSE` and `MEDIA_ENDED`
/// so both sides read the same reason off either path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CloseReason {
    /// This side hung up, or the control plane asked.
    LocalHangup = 1,
    /// The peer ended the call.
    RemoteEnd = 2,
    /// A newer generation replaced this session.
    Replaced = 3,
    /// Setup or media failed.
    Failed = 4,
    /// A deadline passed (dial, open, handshake).
    Timeout = 5,
    /// The relay dropped mid-call.
    RelayDropped = 6,
}

impl CloseReason {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(CloseReason::LocalHangup),
            2 => Some(CloseReason::RemoteEnd),
            3 => Some(CloseReason::Replaced),
            4 => Some(CloseReason::Failed),
            5 => Some(CloseReason::Timeout),
            6 => Some(CloseReason::RelayDropped),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            CloseReason::LocalHangup => "LOCAL_HANGUP",
            CloseReason::RemoteEnd => "REMOTE_END",
            CloseReason::Replaced => "REPLACED",
            CloseReason::Failed => "FAILED",
            CloseReason::Timeout => "TIMEOUT",
            CloseReason::RelayDropped => "RELAY_DROPPED",
        }
    }
}

/// `CLOSE` request (`core -> voip`) and `MEDIA_ENDED` notification
/// (`voip -> core`): one shape for both directions, since both say the same
/// thing — this session's media is over, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseRequest {
    /// The session being released.
    pub session: SessionId,
    /// Why.
    pub reason: CloseReason,
    /// Short detail for the failure reasons, if any.
    pub detail: Option<String>,
}

impl CloseRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u8(self.reason.as_u8());
        w.option_str(self.detail.as_deref());
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let reason = enum_byte("reason", &mut r, CloseReason::from_u8)?;
        Ok(CloseRequest {
            session,
            reason,
            detail: r.option_str()?.map(str::to_owned),
        })
    }
}

/// One RTCP report block, mirroring the core's flattened block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpBlockDto {
    /// Source SSRC.
    pub ssrc: u32,
    /// Fraction lost.
    pub fraction_lost: u8,
    /// Cumulative packets lost (signed upstream).
    pub cumulative_lost: i32,
    /// Extended highest sequence received.
    pub extended_highest_sequence: u32,
    /// Interarrival jitter.
    pub jitter: u32,
    /// Last SR timestamp.
    pub last_sender_report: u32,
    /// Delay since last SR.
    pub delay_since_last_sender_report: u32,
    /// Profile-specific extension bytes.
    pub profile_extension: Vec<u8>,
}

impl RtcpBlockDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.ssrc);
        w.u8(self.fraction_lost);
        w.i32_le(self.cumulative_lost);
        w.u32_le(self.extended_highest_sequence);
        w.u32_le(self.jitter);
        w.u32_le(self.last_sender_report);
        w.u32_le(self.delay_since_last_sender_report);
        w.bytes_raw(&self.profile_extension);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(RtcpBlockDto {
            ssrc: r.u32_le()?,
            fraction_lost: r.u8()?,
            cumulative_lost: r.i32_le()?,
            extended_highest_sequence: r.u32_le()?,
            jitter: r.u32_le()?,
            last_sender_report: r.u32_le()?,
            delay_since_last_sender_report: r.u32_le()?,
            profile_extension: r.bytes_raw()?.to_vec(),
        })
    }
}

/// One RTCP feedback packet, mirroring the core's flattened feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpFeedbackDto {
    /// RTCP packet type.
    pub packet_type: u8,
    /// Format.
    pub fmt: u8,
    /// Sender SSRC.
    pub sender_ssrc: u32,
    /// Media SSRC.
    pub media_ssrc: u32,
    /// Feedback control information bytes.
    pub fci: Vec<u8>,
}

impl RtcpFeedbackDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.packet_type);
        w.u8(self.fmt);
        w.u32_le(self.sender_ssrc);
        w.u32_le(self.media_ssrc);
        w.bytes_raw(&self.fci);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(RtcpFeedbackDto {
            packet_type: r.u8()?,
            fmt: r.u8()?,
            sender_ssrc: r.u32_le()?,
            media_ssrc: r.u32_le()?,
            fci: r.bytes_raw()?.to_vec(),
        })
    }
}

/// Why audio RTP keeps arriving without becoming sound, mirroring the
/// core's neutral silence reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SilenceReason {
    /// No decoder for the negotiated codec.
    NoDecoderForNegotiatedCodec = 0,
    /// Authentication keeps failing.
    AuthenticationFailing = 1,
    /// Unexpected payload type.
    UnexpectedPayloadType = 2,
    /// Codec rejecting frames.
    CodecRejectingFrames = 3,
    /// Codec flapping.
    CodecFlapping = 4,
    /// Unknown.
    Unknown = 5,
}

impl SilenceReason {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(SilenceReason::NoDecoderForNegotiatedCodec),
            1 => Some(SilenceReason::AuthenticationFailing),
            2 => Some(SilenceReason::UnexpectedPayloadType),
            3 => Some(SilenceReason::CodecRejectingFrames),
            4 => Some(SilenceReason::CodecFlapping),
            5 => Some(SilenceReason::Unknown),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            SilenceReason::NoDecoderForNegotiatedCodec => "NO_DECODER_FOR_NEGOTIATED_CODEC",
            SilenceReason::AuthenticationFailing => "AUTHENTICATION_FAILING",
            SilenceReason::UnexpectedPayloadType => "UNEXPECTED_PAYLOAD_TYPE",
            SilenceReason::CodecRejectingFrames => "CODEC_REJECTING_FRAMES",
            SilenceReason::CodecFlapping => "CODEC_FLAPPING",
            SilenceReason::Unknown => "UNKNOWN",
        }
    }
}

/// What decided a codec switch, mirroring the core's neutral source enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodecSource {
    /// Signaling negotiation.
    Negotiated = 0,
    /// Packet content.
    Content = 1,
}

impl CodecSource {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(CodecSource::Negotiated),
            1 => Some(CodecSource::Content),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            CodecSource::Negotiated => "NEGOTIATED",
            CodecSource::Content => "CONTENT",
        }
    }
}

/// One engine-raised event, mirroring the engine-raised subset of the core's
/// public call-event stream. Signaling-born events (peer video states, group
/// snapshots from signaling, reactions, screen share) never cross: the
/// control plane publishes those into the session stream itself, so the same
/// ordered stream carries the whole call either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiEvent {
    /// The relay accepted our allocate; the media path is live.
    RelayAllocated,
    /// One standard Opus packet through the in-profile escape.
    ForeignAudio(Vec<u8>),
    /// One Opus fallback packet from one authenticated group participant.
    ForeignGroupAudio(EncodedFrameDto),
    /// The peer's signaling rates fit no offered profile.
    AudioFormatMismatch {
        /// Rate offered locally.
        expected_rate: u32,
        /// Rates the peer sent.
        received_rates: Vec<u32>,
    },
    /// The relay rejected our allocate, with the STUN error code.
    RelayAllocateFailed(u32),
    /// The relay never acked the allocate within the deadline.
    RelayAllocateTimedOut,
    /// The media path was never built, with why.
    MediaSetupFailed(String),
    /// A migrated relay transport did not finish reconnecting in time.
    RelayReconnectTimedOut,
    /// Outbound video needs an IDR before anything goes on the wire.
    VideoKeyframeNeeded,
    /// Authenticated peer RTCP.
    RtcpReceived {
        /// Packet types.
        packet_types: Vec<u8>,
        /// Sender SSRC.
        sender_ssrc: u32,
        /// Referenced SSRCs.
        referenced_ssrcs: Vec<u32>,
        /// Reports audio: 0 or 1.
        reports_audio: u8,
        /// Reports video: 0 or 1.
        reports_video: u8,
        /// Report blocks.
        report_blocks: Vec<RtcpBlockDto>,
        /// Feedback packets.
        feedback: Vec<RtcpFeedbackDto>,
    },
    /// Relay-send backpressure discarded complete media units.
    OutboundMediaDropped {
        /// Video access units dropped.
        video_access_units: u32,
        /// Packets dropped.
        packets: u32,
    },
    /// Audio RTP keeps arriving and none of it is becoming sound.
    AudioSilent {
        /// Window length in milliseconds.
        silent_for_ms: u64,
        /// RTP packets counted in the window.
        rtp_received: u32,
        /// Frames produced in the window.
        frames_produced: u32,
        /// Dominant reason.
        dominant_reason: SilenceReason,
    },
    /// The payload grammar changed inside the negotiated RTP timing.
    AudioCodecSwitched {
        /// Previous codec.
        from: AudioCodecWire,
        /// New codec.
        to: AudioCodecWire,
        /// What decided the switch.
        source: CodecSource,
        /// Packets observed before switching.
        packets_observed: u32,
    },
    /// The peer speaks one codec and the source emits another, immovably.
    AudioCodecSourceIsFixed {
        /// What the source sends.
        sending: AudioCodecWire,
        /// What the peer expects.
        peer_expects: AudioCodecWire,
        /// What decided.
        source: CodecSource,
    },
    /// Audio RTP has stopped arriving.
    AudioReceptionStalled {
        /// Silence length in milliseconds.
        silent_for_ms: u64,
    },
    /// The media session closed, with why.
    Closed(CloseReason),
}

impl AbiEvent {
    /// The wire discriminant.
    pub fn kind_u8(&self) -> u8 {
        match self {
            AbiEvent::RelayAllocated => 1,
            AbiEvent::ForeignAudio(_) => 2,
            AbiEvent::ForeignGroupAudio(_) => 3,
            AbiEvent::AudioFormatMismatch { .. } => 4,
            AbiEvent::RelayAllocateFailed(_) => 5,
            AbiEvent::RelayAllocateTimedOut => 6,
            AbiEvent::MediaSetupFailed(_) => 7,
            AbiEvent::RelayReconnectTimedOut => 8,
            AbiEvent::VideoKeyframeNeeded => 9,
            AbiEvent::RtcpReceived { .. } => 10,
            AbiEvent::OutboundMediaDropped { .. } => 11,
            AbiEvent::AudioSilent { .. } => 12,
            AbiEvent::AudioCodecSwitched { .. } => 13,
            AbiEvent::AudioCodecSourceIsFixed { .. } => 14,
            AbiEvent::AudioReceptionStalled { .. } => 15,
            AbiEvent::Closed(_) => 16,
        }
    }

    /// Written-down name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            AbiEvent::RelayAllocated => "RELAY_ALLOCATED",
            AbiEvent::ForeignAudio(_) => "FOREIGN_AUDIO",
            AbiEvent::ForeignGroupAudio(_) => "FOREIGN_GROUP_AUDIO",
            AbiEvent::AudioFormatMismatch { .. } => "AUDIO_FORMAT_MISMATCH",
            AbiEvent::RelayAllocateFailed(_) => "RELAY_ALLOCATE_FAILED",
            AbiEvent::RelayAllocateTimedOut => "RELAY_ALLOCATE_TIMED_OUT",
            AbiEvent::MediaSetupFailed(_) => "MEDIA_SETUP_FAILED",
            AbiEvent::RelayReconnectTimedOut => "RELAY_RECONNECT_TIMED_OUT",
            AbiEvent::VideoKeyframeNeeded => "VIDEO_KEYFRAME_NEEDED",
            AbiEvent::RtcpReceived { .. } => "RTCP_RECEIVED",
            AbiEvent::OutboundMediaDropped { .. } => "OUTBOUND_MEDIA_DROPPED",
            AbiEvent::AudioSilent { .. } => "AUDIO_SILENT",
            AbiEvent::AudioCodecSwitched { .. } => "AUDIO_CODEC_SWITCHED",
            AbiEvent::AudioCodecSourceIsFixed { .. } => "AUDIO_CODEC_SOURCE_IS_FIXED",
            AbiEvent::AudioReceptionStalled { .. } => "AUDIO_RECEPTION_STALLED",
            AbiEvent::Closed(_) => "CLOSED",
        }
    }

    /// Serializes discriminant plus fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.kind_u8());
        match self {
            AbiEvent::ForeignAudio(data) => w.bytes_raw(data),
            AbiEvent::ForeignGroupAudio(frame) => {
                let raw = frame.encode();
                w.bytes_raw(&raw);
            }
            AbiEvent::AudioFormatMismatch {
                expected_rate,
                received_rates,
            } => {
                w.u32_le(*expected_rate);
                w.u32_le(received_rates.len() as u32);
                for rate in received_rates {
                    w.u32_le(*rate);
                }
            }
            AbiEvent::RelayAllocateFailed(code) => w.u32_le(*code),
            AbiEvent::MediaSetupFailed(detail) => w.utf8(detail),
            AbiEvent::RtcpReceived {
                packet_types,
                sender_ssrc,
                referenced_ssrcs,
                reports_audio,
                reports_video,
                report_blocks,
                feedback,
            } => {
                w.bytes_raw(packet_types);
                w.u32_le(*sender_ssrc);
                w.u32_le(referenced_ssrcs.len() as u32);
                for ssrc in referenced_ssrcs {
                    w.u32_le(*ssrc);
                }
                w.u8(*reports_audio);
                w.u8(*reports_video);
                encode_dto_vec(&mut w, report_blocks, |w, b| {
                    let raw = b.encode();
                    w.bytes_raw(&raw);
                });
                encode_dto_vec(&mut w, feedback, |w, f| {
                    let raw = f.encode();
                    w.bytes_raw(&raw);
                });
            }
            AbiEvent::OutboundMediaDropped {
                video_access_units,
                packets,
            } => {
                w.u32_le(*video_access_units);
                w.u32_le(*packets);
            }
            AbiEvent::AudioSilent {
                silent_for_ms,
                rtp_received,
                frames_produced,
                dominant_reason,
            } => {
                w.u64_le(*silent_for_ms);
                w.u32_le(*rtp_received);
                w.u32_le(*frames_produced);
                w.u8(dominant_reason.as_u8());
            }
            AbiEvent::AudioCodecSwitched {
                from,
                to,
                source,
                packets_observed,
            } => {
                w.u8(from.as_u8());
                w.u8(to.as_u8());
                w.u8(source.as_u8());
                w.u32_le(*packets_observed);
            }
            AbiEvent::AudioCodecSourceIsFixed {
                sending,
                peer_expects,
                source,
            } => {
                w.u8(sending.as_u8());
                w.u8(peer_expects.as_u8());
                w.u8(source.as_u8());
            }
            AbiEvent::AudioReceptionStalled { silent_for_ms } => w.u64_le(*silent_for_ms),
            AbiEvent::Closed(reason) => w.u8(reason.as_u8()),
            _ => {}
        }
        w.finish()
    }

    /// Parses discriminant plus fields.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let kind = r.u8()?;
        let ev = match kind {
            1 => AbiEvent::RelayAllocated,
            2 => AbiEvent::ForeignAudio(r.bytes_raw()?.to_vec()),
            3 => AbiEvent::ForeignGroupAudio(EncodedFrameDto::decode(r.bytes_raw()?)?),
            4 => {
                let expected_rate = r.u32_le()?;
                let count = r.u32_le()?;
                if count > 64 {
                    return Err(DecodeError::BadLength(count));
                }
                let mut received_rates = Vec::with_capacity(count.min(8) as usize);
                for _ in 0..count {
                    received_rates.push(r.u32_le()?);
                }
                AbiEvent::AudioFormatMismatch {
                    expected_rate,
                    received_rates,
                }
            }
            5 => AbiEvent::RelayAllocateFailed(r.u32_le()?),
            6 => AbiEvent::RelayAllocateTimedOut,
            7 => AbiEvent::MediaSetupFailed(r.utf8()?.to_owned()),
            8 => AbiEvent::RelayReconnectTimedOut,
            9 => AbiEvent::VideoKeyframeNeeded,
            10 => {
                let packet_types = r.bytes_raw()?.to_vec();
                let sender_ssrc = r.u32_le()?;
                let ref_count = r.u32_le()?;
                if ref_count > 256 {
                    return Err(DecodeError::BadLength(ref_count));
                }
                let mut referenced_ssrcs = Vec::with_capacity(ref_count.min(8) as usize);
                for _ in 0..ref_count {
                    referenced_ssrcs.push(r.u32_le()?);
                }
                let reports_audio = r.u8()?;
                let reports_video = r.u8()?;
                for (field, v) in [
                    ("reports_audio", reports_audio),
                    ("reports_video", reports_video),
                ] {
                    if v > 1 {
                        return Err(DecodeError::BadValue { field, value: v });
                    }
                }
                AbiEvent::RtcpReceived {
                    packet_types,
                    sender_ssrc,
                    referenced_ssrcs,
                    reports_audio,
                    reports_video,
                    report_blocks: decode_dto_vec(&mut r, RtcpBlockDto::decode)?,
                    feedback: decode_dto_vec(&mut r, RtcpFeedbackDto::decode)?,
                }
            }
            11 => AbiEvent::OutboundMediaDropped {
                video_access_units: r.u32_le()?,
                packets: r.u32_le()?,
            },
            12 => AbiEvent::AudioSilent {
                silent_for_ms: r.u64_le()?,
                rtp_received: r.u32_le()?,
                frames_produced: r.u32_le()?,
                dominant_reason: enum_byte("dominant_reason", &mut r, SilenceReason::from_u8)?,
            },
            13 => AbiEvent::AudioCodecSwitched {
                from: enum_byte("from", &mut r, AudioCodecWire::from_u8)?,
                to: enum_byte("to", &mut r, AudioCodecWire::from_u8)?,
                source: enum_byte("source", &mut r, CodecSource::from_u8)?,
                packets_observed: r.u32_le()?,
            },
            14 => AbiEvent::AudioCodecSourceIsFixed {
                sending: enum_byte("sending", &mut r, AudioCodecWire::from_u8)?,
                peer_expects: enum_byte("peer_expects", &mut r, AudioCodecWire::from_u8)?,
                source: enum_byte("source", &mut r, CodecSource::from_u8)?,
            },
            15 => AbiEvent::AudioReceptionStalled {
                silent_for_ms: r.u64_le()?,
            },
            16 => AbiEvent::Closed(enum_byte("reason", &mut r, CloseReason::from_u8)?),
            _ => {
                return Err(DecodeError::BadValue {
                    field: "event",
                    value: kind,
                });
            }
        };
        Ok(ev)
    }
}

/// One decrypted codec payload from the peer, mirroring the core's flattened
/// encoded frame. The audio format (timing, profile) does not travel per
/// packet: the bridge fills it from the negotiated open params, refreshed on
/// `AudioCodecSwitched`, so the wire carries only what changes per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrameDto {
    /// Codec of these bytes.
    pub codec: AudioCodecWire,
    /// Codec payload bytes.
    pub data: Vec<u8>,
    /// RTP payload type.
    pub payload_type: u8,
    /// RTP sequence number.
    pub sequence_number: u32,
    /// RTP timestamp.
    pub timestamp: u32,
    /// Marker bit: 0 or 1.
    pub marker: u8,
    /// Sender JID string, if any.
    pub sender: Option<String>,
    /// Sender device JID string, if any.
    pub device: Option<String>,
}

impl EncodedFrameDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.codec.as_u8());
        w.bytes_raw(&self.data);
        w.u8(self.payload_type);
        w.u32_le(self.sequence_number);
        w.u32_le(self.timestamp);
        w.u8(self.marker);
        w.option_str(self.sender.as_deref());
        w.option_str(self.device.as_deref());
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let codec = enum_byte("codec", &mut r, AudioCodecWire::from_u8)?;
        let data = r.bytes_raw()?.to_vec();
        let payload_type = r.u8()?;
        let sequence_number = r.u32_le()?;
        let timestamp = r.u32_le()?;
        let marker = r.u8()?;
        if marker > 1 {
            return Err(DecodeError::BadValue {
                field: "marker",
                value: marker,
            });
        }
        Ok(EncodedFrameDto {
            codec,
            data,
            payload_type,
            sequence_number,
            timestamp,
            marker,
            sender: r.option_str()?.map(str::to_owned),
            device: r.option_str()?.map(str::to_owned),
        })
    }
}

/// One reassembled peer access unit, mirroring the core's video frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrameDto {
    /// Annex-B access unit bytes.
    pub data: Vec<u8>,
    /// Carries an IDR/SPS/PPS NAL: 0 or 1.
    pub keyframe: u8,
    /// Rotation bits (0..3) from RTP metadata.
    pub orientation: u8,
    /// Group sender JID string. Absent on 1:1 video.
    pub sender: Option<String>,
    /// Group sender device JID string. Absent on 1:1 video.
    pub device: Option<String>,
    /// Relay participant id, if any.
    pub pid: Option<u32>,
}

impl VideoFrameDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes_raw(&self.data);
        w.u8(self.keyframe);
        w.u8(self.orientation);
        w.option_str(self.sender.as_deref());
        w.option_str(self.device.as_deref());
        encode_option_u32(&mut w, self.pid);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let data = r.bytes_raw()?.to_vec();
        let keyframe = r.u8()?;
        if keyframe > 1 {
            return Err(DecodeError::BadValue {
                field: "keyframe",
                value: keyframe,
            });
        }
        let orientation = r.u8()?;
        if orientation > 3 {
            return Err(DecodeError::BadValue {
                field: "orientation",
                value: orientation,
            });
        }
        Ok(VideoFrameDto {
            data,
            keyframe,
            orientation,
            sender: r.option_str()?.map(str::to_owned),
            device: r.option_str()?.map(str::to_owned),
            pid: decode_option_u32(&mut r, "pid")?,
        })
    }
}

/// One outbound video access unit with its capture metadata, mirroring the
/// core's timed video input. The untimed source feed has no timestamp or
/// input generation; `None` tells the engine to pace and accept freely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInputDto {
    /// Annex-B access unit bytes.
    pub data: Vec<u8>,
    /// Capture timestamp in the 90 kHz RTP clock, if timed.
    pub timestamp: Option<u32>,
    /// Source generation, if the source assigns one.
    pub input_generation: Option<u64>,
}

impl VideoInputDto {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes_raw(&self.data);
        encode_option_u32(&mut w, self.timestamp);
        match self.input_generation {
            None => w.u8(0),
            Some(g) => {
                w.u8(1);
                w.u64_le(g);
            }
        }
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let data = r.bytes_raw()?.to_vec();
        let timestamp = decode_option_u32(&mut r, "timestamp")?;
        let input_generation = match r.u8()? {
            0 => None,
            1 => Some(r.u64_le()?),
            b => {
                return Err(DecodeError::BadValue {
                    field: "input_generation",
                    value: b,
                });
            }
        };
        Ok(VideoInputDto {
            data,
            timestamp,
            input_generation,
        })
    }
}

/// `EVENT` notification (`voip -> core`): one engine-raised event against a
/// session, published into the same ordered stream as signaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventNotification {
    /// The session the event belongs to.
    pub session: SessionId,
    /// The event.
    pub event: AbiEvent,
}

impl EventNotification {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        let ev = self.event.encode();
        w.bytes_raw(&ev);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let raw = r.bytes_raw()?;
        Ok(EventNotification {
            session,
            event: AbiEvent::decode(raw)?,
        })
    }
}

/// Per-call media counters, mirroring the core's neutral stats record field
/// for field. The core caches the latest push per session and serves reads
/// locally, so stats never poll across the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsData {
    /// RTP packets received.
    pub rtp_received: u32,
    /// RTP packets with an unexpected payload type.
    pub rtp_payload_type_unexpected: u32,
    /// SRTP unprotect failures.
    pub srtp_unprotect_failed: u32,
    /// SFrame decrypt failures.
    pub sframe_decrypt_failed: u32,
    /// Audio frames decoded.
    pub audio_frames_decoded: u32,
    /// Audio frames delivered.
    pub audio_frames_delivered: u32,
    /// Concealed audio frames.
    pub audio_frames_concealed: u32,
    /// MLOW off-point frames dropped.
    pub mlow_off_point_dropped: u32,
    /// MLOW inactive/SID frames.
    pub mlow_inactive_or_sid: u32,
    /// Foreign (Opus-escape) frames decoded.
    pub foreign_frames_decoded: u32,
    /// Audio frames with no decoder.
    pub audio_frames_without_decoder: u32,
    /// Outbound frames with no encoder.
    pub outbound_frames_without_encoder: u32,
    /// Trimmed playout samples.
    pub playout_trimmed_samples: u32,
    /// Inbound pipe drops.
    pub inbound_pipe_dropped: u32,
    /// Audio sink drops.
    pub audio_sink_dropped: u32,
    /// Video sink drops.
    pub video_sink_dropped: u32,
    /// Peer keyframe requests.
    pub peer_keyframe_requests: u32,
    /// Unclassified relay packets.
    pub relay_packet_unclassified: u32,
    /// Rejected forwarding envelopes.
    pub forwarding_envelope_rejected: u32,
    /// Codec switches (u16 upstream; saturated on mapping).
    pub codec_switches: u32,
}

impl StatsData {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.rtp_received);
        w.u32_le(self.rtp_payload_type_unexpected);
        w.u32_le(self.srtp_unprotect_failed);
        w.u32_le(self.sframe_decrypt_failed);
        w.u32_le(self.audio_frames_decoded);
        w.u32_le(self.audio_frames_delivered);
        w.u32_le(self.audio_frames_concealed);
        w.u32_le(self.mlow_off_point_dropped);
        w.u32_le(self.mlow_inactive_or_sid);
        w.u32_le(self.foreign_frames_decoded);
        w.u32_le(self.audio_frames_without_decoder);
        w.u32_le(self.outbound_frames_without_encoder);
        w.u32_le(self.playout_trimmed_samples);
        w.u32_le(self.inbound_pipe_dropped);
        w.u32_le(self.audio_sink_dropped);
        w.u32_le(self.video_sink_dropped);
        w.u32_le(self.peer_keyframe_requests);
        w.u32_le(self.relay_packet_unclassified);
        w.u32_le(self.forwarding_envelope_rejected);
        w.u32_le(self.codec_switches);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(StatsData {
            rtp_received: r.u32_le()?,
            rtp_payload_type_unexpected: r.u32_le()?,
            srtp_unprotect_failed: r.u32_le()?,
            sframe_decrypt_failed: r.u32_le()?,
            audio_frames_decoded: r.u32_le()?,
            audio_frames_delivered: r.u32_le()?,
            audio_frames_concealed: r.u32_le()?,
            mlow_off_point_dropped: r.u32_le()?,
            mlow_inactive_or_sid: r.u32_le()?,
            foreign_frames_decoded: r.u32_le()?,
            audio_frames_without_decoder: r.u32_le()?,
            outbound_frames_without_encoder: r.u32_le()?,
            playout_trimmed_samples: r.u32_le()?,
            inbound_pipe_dropped: r.u32_le()?,
            audio_sink_dropped: r.u32_le()?,
            video_sink_dropped: r.u32_le()?,
            peer_keyframe_requests: r.u32_le()?,
            relay_packet_unclassified: r.u32_le()?,
            forwarding_envelope_rejected: r.u32_le()?,
            codec_switches: r.u32_le()?,
        })
    }
}

/// `STATS` request (`core -> voip`): poll one session's counters. The
/// response body is [`StatsData`]. The bridge itself never polls — it serves
/// the locally cached push — but the shape stays for backends that do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsRequest {
    /// The session being polled.
    pub session: SessionId,
}

impl StatsRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        encode_session(self.session)
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        Ok(StatsRequest {
            session: decode_session(buf)?,
        })
    }
}

/// `STATS` push (`voip -> core`, no response flag): one session's counters
/// for the core's local cache. Same body shape as the poll response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsPush {
    /// The session the counters belong to.
    pub session: SessionId,
    /// The counters.
    pub stats: StatsData,
}

impl StatsPush {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        let stats = self.stats.encode();
        w.bytes_raw(&stats);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let raw = r.bytes_raw()?;
        Ok(StatsPush {
            session,
            stats: StatsData::decode(raw)?,
        })
    }
}

/// One PCM or encoded-audio-in frame: session, sequence, and the bytes
/// inline (copy semantics, v1). PCM data is little-endian `i16` samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFrame {
    /// The session the frame belongs to.
    pub session: SessionId,
    /// Per-session sequence; the pumps use it to order and spot loss.
    pub seq: u32,
    /// The frame bytes.
    pub data: Vec<u8>,
}

impl MediaFrame {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u32_le(self.seq);
        w.bytes_raw(&self.data);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        Ok(MediaFrame {
            session,
            seq: r.u32_le()?,
            data: r.bytes_raw()?.to_vec(),
        })
    }
}

/// One encoded audio packet out of the engine, with the RTP metadata the
/// core's encoded sink carries alongside the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedAudioOut {
    /// The session the packet belongs to.
    pub session: SessionId,
    /// Per-session sequence.
    pub seq: u32,
    /// The packet with its metadata.
    pub frame: EncodedFrameDto,
}

impl EncodedAudioOut {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u32_le(self.seq);
        let raw = self.frame.encode();
        w.bytes_raw(&raw);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let seq = r.u32_le()?;
        Ok(EncodedAudioOut {
            session,
            seq,
            frame: EncodedFrameDto::decode(r.bytes_raw()?)?,
        })
    }
}

/// One outbound video access unit with its capture metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoIn {
    /// The session the unit belongs to.
    pub session: SessionId,
    /// Per-session sequence.
    pub seq: u32,
    /// The unit with its metadata.
    pub input: VideoInputDto,
}

impl VideoIn {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u32_le(self.seq);
        let raw = self.input.encode();
        w.bytes_raw(&raw);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let seq = r.u32_le()?;
        Ok(VideoIn {
            session,
            seq,
            input: VideoInputDto::decode(r.bytes_raw()?)?,
        })
    }
}

/// One reassembled peer access unit with its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoOut {
    /// The session the unit belongs to.
    pub session: SessionId,
    /// Per-session sequence.
    pub seq: u32,
    /// The unit with its metadata.
    pub frame: VideoFrameDto,
}

impl VideoOut {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u32_le(self.seq);
        let raw = self.frame.encode();
        w.bytes_raw(&raw);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        let seq = r.u32_le()?;
        Ok(VideoOut {
            session,
            seq,
            frame: VideoFrameDto::decode(r.bytes_raw()?)?,
        })
    }
}

/// An `ERROR`-flagged response body: one code plus an optional short detail.
/// The detail names what failed; it never stacks a second message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorBody {
    /// What failed.
    pub code: AbiErrorCode,
    /// Short detail, if any.
    pub detail: Option<String>,
}

impl ErrorBody {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.code.as_u8());
        w.option_str(self.detail.as_deref());
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let code = enum_byte("code", &mut r, AbiErrorCode::from_u8)?;
        Ok(ErrorBody {
            code,
            detail: r.option_str()?.map(str::to_owned),
        })
    }
}
