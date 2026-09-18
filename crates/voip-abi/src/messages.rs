//! Payload DTOs: one struct per message body, each with an exact `encode`
//! and `decode`. Decoders reject a short payload and ignore trailing bytes,
//! which is what lets a newer minor append fields.

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

/// `RESERVE` request: binds a handle to `(call_id, generation)` before any
/// media message names it. Reserving a handle that is still reserved fails;
/// replacement is `CLOSE` then `RESERVE`, never an overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveRequest {
    /// The session identity being bound.
    pub session: SessionId,
    /// The WhatsApp call id this handle stands for.
    pub call_id: String,
}

impl ReserveRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.utf8(&self.call_id);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        Ok(ReserveRequest {
            session,
            call_id: r.utf8()?.to_owned(),
        })
    }
}

/// Audio codec named in the open params. The bytes on the frame opcodes are
/// opaque to the ABI; this only says which pump the engine must attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AudioCodec {
    /// No audio path (video-only or signaling-only open).
    None = 0,
    /// MLOW codec audio.
    Mlow = 1,
    /// Opus audio.
    Opus = 2,
    /// Raw PCM audio.
    Pcm = 3,
}

impl AudioCodec {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(AudioCodec::None),
            1 => Some(AudioCodec::Mlow),
            2 => Some(AudioCodec::Opus),
            3 => Some(AudioCodec::Pcm),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            AudioCodec::None => "NONE",
            AudioCodec::Mlow => "MLOW",
            AudioCodec::Opus => "OPUS",
            AudioCodec::Pcm => "PCM",
        }
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

/// What `BEGIN_OPEN` carries: everything the engine needs to build the media
/// path, as plain data. Key material travels as [`SecretBytes`] and is
/// zeroed on drop on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenParams {
    /// Which audio pump to attach.
    pub audio_codec: AudioCodec,
    /// Samples per second (e.g. 16000).
    pub sample_rate_hz: u32,
    /// Audio channels (1 for mono).
    pub channels: u8,
    /// Start muted: 0 or 1.
    pub muted: u8,
    /// Relay host, as text.
    pub relay_host: String,
    /// Relay port.
    pub relay_port: u32,
    /// Relay token (secret).
    pub relay_token: SecretBytes,
    /// Relay auth token (secret).
    pub auth_token: SecretBytes,
    /// Call key (secret).
    pub call_key: SecretBytes,
    /// Integrity key (secret).
    pub integrity_key: SecretBytes,
    /// [`VideoCaps`] bits.
    pub video: u8,
    /// Group call id, when opening into a group.
    pub group_id: Option<String>,
    /// Group epoch, when opening into a group.
    pub group_epoch: Option<u64>,
}

impl OpenParams {
    /// Serializes the body. Field order is the wire order; append-only.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.audio_codec.as_u8());
        w.u32_le(self.sample_rate_hz);
        w.u8(self.channels);
        w.u8(self.muted);
        w.utf8(&self.relay_host);
        w.u32_le(self.relay_port);
        w.bytes_raw(self.relay_token.as_bytes());
        w.bytes_raw(self.auth_token.as_bytes());
        w.bytes_raw(self.call_key.as_bytes());
        w.bytes_raw(self.integrity_key.as_bytes());
        w.u8(self.video);
        w.option_str(self.group_id.as_deref());
        match self.group_epoch {
            None => w.u8(0),
            Some(e) => {
                w.u8(1);
                w.u64_le(e);
            }
        }
        w.finish()
    }

    /// Parses the body. Trailing bytes are ignored (newer minor fields).
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let audio_codec = enum_byte("audio_codec", &mut r, AudioCodec::from_u8)?;
        let sample_rate_hz = r.u32_le()?;
        let channels = r.u8()?;
        let muted = r.u8()?;
        if muted > 1 {
            return Err(DecodeError::BadValue {
                field: "muted",
                value: muted,
            });
        }
        let relay_host = r.utf8()?.to_owned();
        let relay_port = r.u32_le()?;
        let relay_token = SecretBytes::new(r.bytes_raw()?.to_vec());
        let auth_token = SecretBytes::new(r.bytes_raw()?.to_vec());
        let call_key = SecretBytes::new(r.bytes_raw()?.to_vec());
        let integrity_key = SecretBytes::new(r.bytes_raw()?.to_vec());
        let video = r.u8()?;
        let group_id = r.option_str()?.map(str::to_owned);
        let epoch_flag = r.u8()?;
        let group_epoch = match epoch_flag {
            0 => None,
            1 => Some(r.u64_le()?),
            _ => {
                return Err(DecodeError::BadValue {
                    field: "group_epoch",
                    value: epoch_flag,
                });
            }
        };
        Ok(OpenParams {
            audio_codec,
            sample_rate_hz,
            channels,
            muted,
            relay_host,
            relay_port,
            relay_token,
            auth_token,
            call_key,
            integrity_key,
            video,
            group_id,
            group_epoch,
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

/// One media command. The discriminant is the wire; each variant's fields
/// follow in order, append-only per variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaCommand {
    /// Mute the outbound audio path.
    MuteAudio,
    /// Unmute the outbound audio path.
    UnmuteAudio,
    /// Enable video sending.
    VideoEnable,
    /// Enable video sending once the peer accepts.
    VideoEnableAwaitingAccept,
    /// Disable video entirely.
    VideoDisable,
    /// Stop sending, keep receiving.
    VideoDisableOutbound,
    /// Disable video but keep the legacy fallback path.
    VideoDisableKeepLegacy,
    /// Move the video input to a new generation.
    VideoSetInputGeneration(u64),
    /// Stride between video timestamps.
    VideoSetTimestampStride(u32),
    /// Camera orientation in degrees.
    VideoSetOrientation(u32),
    /// Ask the peer for a keyframe.
    VideoRequestKeyframe,
    /// Replace the frame encryption key (secret, zeroed on drop).
    Rekey(SecretBytes),
}

impl MediaCommand {
    /// The wire discriminant.
    pub fn kind_u8(&self) -> u8 {
        match self {
            MediaCommand::MuteAudio => 1,
            MediaCommand::UnmuteAudio => 2,
            MediaCommand::VideoEnable => 3,
            MediaCommand::VideoEnableAwaitingAccept => 4,
            MediaCommand::VideoDisable => 5,
            MediaCommand::VideoDisableOutbound => 6,
            MediaCommand::VideoDisableKeepLegacy => 7,
            MediaCommand::VideoSetInputGeneration(_) => 8,
            MediaCommand::VideoSetTimestampStride(_) => 9,
            MediaCommand::VideoSetOrientation(_) => 10,
            MediaCommand::VideoRequestKeyframe => 11,
            MediaCommand::Rekey(_) => 12,
        }
    }

    /// Written-down name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            MediaCommand::MuteAudio => "MUTE_AUDIO",
            MediaCommand::UnmuteAudio => "UNMUTE_AUDIO",
            MediaCommand::VideoEnable => "VIDEO_ENABLE",
            MediaCommand::VideoEnableAwaitingAccept => "VIDEO_ENABLE_AWAITING_ACCEPT",
            MediaCommand::VideoDisable => "VIDEO_DISABLE",
            MediaCommand::VideoDisableOutbound => "VIDEO_DISABLE_OUTBOUND",
            MediaCommand::VideoDisableKeepLegacy => "VIDEO_DISABLE_KEEP_LEGACY",
            MediaCommand::VideoSetInputGeneration(_) => "VIDEO_SET_INPUT_GENERATION",
            MediaCommand::VideoSetTimestampStride(_) => "VIDEO_SET_TIMESTAMP_STRIDE",
            MediaCommand::VideoSetOrientation(_) => "VIDEO_SET_ORIENTATION",
            MediaCommand::VideoRequestKeyframe => "VIDEO_REQUEST_KEYFRAME",
            MediaCommand::Rekey(_) => "REKEY",
        }
    }

    /// Serializes discriminant plus fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.kind_u8());
        match self {
            MediaCommand::VideoSetInputGeneration(g) => w.u64_le(*g),
            MediaCommand::VideoSetTimestampStride(s) => w.u32_le(*s),
            MediaCommand::VideoSetOrientation(o) => w.u32_le(*o),
            MediaCommand::Rekey(k) => w.bytes_raw(k.as_bytes()),
            _ => {}
        }
        w.finish()
    }

    /// Parses discriminant plus fields.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let kind = r.u8()?;
        let cmd = match kind {
            1 => MediaCommand::MuteAudio,
            2 => MediaCommand::UnmuteAudio,
            3 => MediaCommand::VideoEnable,
            4 => MediaCommand::VideoEnableAwaitingAccept,
            5 => MediaCommand::VideoDisable,
            6 => MediaCommand::VideoDisableOutbound,
            7 => MediaCommand::VideoDisableKeepLegacy,
            8 => MediaCommand::VideoSetInputGeneration(r.u64_le()?),
            9 => MediaCommand::VideoSetTimestampStride(r.u32_le()?),
            10 => MediaCommand::VideoSetOrientation(r.u32_le()?),
            11 => MediaCommand::VideoRequestKeyframe,
            12 => MediaCommand::Rekey(SecretBytes::new(r.bytes_raw()?.to_vec())),
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

/// `GROUP_FITS` request: asks whether the engine fits a roster this large.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupFitsRequest {
    /// The session asking.
    pub session: SessionId,
    /// Roster size being probed.
    pub participant_count: u32,
}

impl GroupFitsRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u32_le(self.participant_count);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let session = SessionId {
            handle: r.u32_le()?,
            generation: r.u64_le()?,
        };
        Ok(GroupFitsRequest {
            session,
            participant_count: r.u32_le()?,
        })
    }
}

/// `GROUP_FITS` response: the verdict and the engine's own limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupFitsResponse {
    /// Whether the probed roster fits: 0 or 1.
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
    /// This side hung up.
    LocalHangup = 1,
    /// The peer ended the call.
    RemoteEnd = 2,
    /// A newer generation replaced this session.
    Replaced = 3,
    /// Setup or media failed.
    Failed = 4,
    /// A deadline passed (dial, open, handshake).
    Timeout = 5,
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
        }
    }
}

/// `CLOSE` request: release a session for the given reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseRequest {
    /// The session being released.
    pub session: SessionId,
    /// Why.
    pub reason: CloseReason,
}

impl CloseRequest {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u8(self.reason.as_u8());
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
        Ok(CloseRequest { session, reason })
    }
}

/// Media-side call state reported in events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MediaState {
    /// Reserved, no media yet.
    Dormant = 1,
    /// Setup in flight.
    Opening = 2,
    /// Media flowing.
    Active = 3,
    /// Media ended (the `MEDIA_ENDED` reason says why).
    Ended = 4,
}

impl MediaState {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(MediaState::Dormant),
            2 => Some(MediaState::Opening),
            3 => Some(MediaState::Active),
            4 => Some(MediaState::Ended),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            MediaState::Dormant => "DORMANT",
            MediaState::Opening => "OPENING",
            MediaState::Active => "ACTIVE",
            MediaState::Ended => "ENDED",
        }
    }
}

/// A peer's video state reported in events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PeerVideoState {
    /// Not sending.
    Off = 0,
    /// Sending.
    On = 1,
    /// Sending but paused.
    Paused = 2,
}

impl PeerVideoState {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(PeerVideoState::Off),
            1 => Some(PeerVideoState::On),
            2 => Some(PeerVideoState::Paused),
            _ => None,
        }
    }

    /// Written-down name.
    pub fn name(self) -> &'static str {
        match self {
            PeerVideoState::Off => "OFF",
            PeerVideoState::On => "ON",
            PeerVideoState::Paused => "PAUSED",
        }
    }
}

/// One media-side event. The discriminant is the wire; each variant's fields
/// follow in order, append-only per variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaEvent {
    /// The session changed media state.
    State(MediaState),
    /// A peer's video state changed.
    PeerVideo {
        /// Who changed, as text.
        peer: String,
        /// Their new state.
        state: PeerVideoState,
    },
    /// The media path failed. The code is engine-local; the detail is short.
    Error {
        /// Engine-local code.
        code: u8,
        /// Short detail, never stacked.
        detail: String,
    },
    /// Group media membership moved.
    GroupMedia {
        /// The epoch this describes.
        epoch: u64,
        /// Roster size at that epoch.
        count: u32,
    },
}

impl MediaEvent {
    /// The wire discriminant.
    pub fn kind_u8(&self) -> u8 {
        match self {
            MediaEvent::State(_) => 1,
            MediaEvent::PeerVideo { .. } => 2,
            MediaEvent::Error { .. } => 3,
            MediaEvent::GroupMedia { .. } => 4,
        }
    }

    /// Written-down name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            MediaEvent::State(_) => "STATE",
            MediaEvent::PeerVideo { .. } => "PEER_VIDEO",
            MediaEvent::Error { .. } => "ERROR",
            MediaEvent::GroupMedia { .. } => "GROUP_MEDIA",
        }
    }

    /// Serializes discriminant plus fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(self.kind_u8());
        match self {
            MediaEvent::State(s) => w.u8(s.as_u8()),
            MediaEvent::PeerVideo { peer, state } => {
                w.utf8(peer);
                w.u8(state.as_u8());
            }
            MediaEvent::Error { code, detail } => {
                w.u8(*code);
                w.utf8(detail);
            }
            MediaEvent::GroupMedia { epoch, count } => {
                w.u64_le(*epoch);
                w.u32_le(*count);
            }
        }
        w.finish()
    }

    /// Parses discriminant plus fields.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let kind = r.u8()?;
        let ev = match kind {
            1 => MediaEvent::State(enum_byte("state", &mut r, MediaState::from_u8)?),
            2 => MediaEvent::PeerVideo {
                peer: r.utf8()?.to_owned(),
                state: enum_byte("peer_state", &mut r, PeerVideoState::from_u8)?,
            },
            3 => MediaEvent::Error {
                code: r.u8()?,
                detail: r.utf8()?.to_owned(),
            },
            4 => MediaEvent::GroupMedia {
                epoch: r.u64_le()?,
                count: r.u32_le()?,
            },
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

/// `EVENT` notification (`voip -> core`): one media-side event against a
/// session, published into the same ordered stream as signaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventNotification {
    /// The session the event belongs to.
    pub session: SessionId,
    /// The event.
    pub event: MediaEvent,
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
            event: MediaEvent::decode(raw)?,
        })
    }
}

/// Coarse media counters. The core caches the latest push per session and
/// serves reads locally, so stats never poll across the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsData {
    /// Round-trip milliseconds.
    pub rtt_ms: u32,
    /// Send bitrate, bits per second.
    pub tx_bitrate_bps: u32,
    /// Receive bitrate, bits per second.
    pub rx_bitrate_bps: u32,
    /// Packets lost since the last report.
    pub packets_lost: u32,
    /// Jitter milliseconds.
    pub jitter_ms: u32,
}

impl StatsData {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.rtt_ms);
        w.u32_le(self.tx_bitrate_bps);
        w.u32_le(self.rx_bitrate_bps);
        w.u32_le(self.packets_lost);
        w.u32_le(self.jitter_ms);
        w.finish()
    }

    /// Parses the body.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        Ok(StatsData {
            rtt_ms: r.u32_le()?,
            tx_bitrate_bps: r.u32_le()?,
            rx_bitrate_bps: r.u32_le()?,
            packets_lost: r.u32_le()?,
            jitter_ms: r.u32_le()?,
        })
    }
}

/// `STATS` request (`core -> voip`): poll one session's counters. The
/// response body is [`StatsData`].
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

/// `MEDIA_ENDED` notification (`voip -> core`): the media path ended.
/// Fire-and-forget; the core tears the session down on receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEndedNotification {
    /// The session whose media ended.
    pub session: SessionId,
    /// Why.
    pub reason: CloseReason,
}

impl MediaEndedNotification {
    /// Serializes the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_le(self.session.handle);
        w.u64_le(self.session.generation);
        w.u8(self.reason.as_u8());
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
        Ok(MediaEndedNotification { session, reason })
    }
}

/// One media frame on any of the six frame opcodes: session, sequence, and
/// the bytes inline (copy semantics, v1).
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
