//! Header layout, opcodes, flags, capabilities, and error codes.
//!
//! The header is fixed at 13 bytes so either side can frame without parsing:
//! `magic[4] | major u8 | minor u8 | opcode u8 | flags u16 LE | payload_len u32 LE`.

use crate::codec::DecodeError;
use crate::{ABI_MAJOR, ABI_MINOR};

/// `OZVP`: every message starts here, so garbage fails on the first 4 bytes.
pub const MAGIC: [u8; 4] = *b"OZVP";
/// Header bytes: 4 magic + major + minor + opcode + 2 flags + 4 length.
pub const HEADER_LEN: usize = 13;

/// Set on every response; the opcode echoes the request's.
pub const FLAG_RESPONSE: u16 = 0x0001;
/// Set with `RESPONSE` when the request failed; the payload is an error body.
pub const FLAG_ERROR: u16 = 0x0002;

/// The operation a message carries. Frame opcodes (`>= 0x10`) are media;
/// everything below is control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    /// `core -> voip`: version probe and capability exchange. Response
    /// carries the responder's capabilities.
    Hello = 0x01,
    /// `core -> voip`: bind a handle to `(call_id, generation)` before any
    /// media message names it.
    Reserve = 0x02,
    /// `core -> voip`: start asynchronous setup with open params. Ack means
    /// setup started, not finished; completion arrives as [`Opcode::Open`].
    BeginOpen = 0x03,
    /// `voip -> core`: setup completed, media is flowing. A notification,
    /// never a request.
    Open = 0x04,
    /// `core -> voip`: abort an in-flight `BeginOpen`. The ack carries the
    /// [`CancelOutcome`].
    CancelOpen = 0x05,
    /// `core -> voip`: mute, video control, rekey, keyframe. See
    /// [`crate::MediaCommandKind`].
    Command = 0x06,
    /// `core -> voip`: admission probe for a group roster. Response carries
    /// whether the engine fits it and its limit.
    GroupFits = 0x07,
    /// `core -> voip`: release a session. Carries the [`crate::CloseReason`].
    Close = 0x08,
    /// `voip -> core`: media-side event on the same ordered stream as
    /// signaling. A notification, never a request.
    Event = 0x09,
    /// Both directions: `core -> voip` polls, `voip -> core` pushes when the
    /// `STATS_PUSH` capability was negotiated. Pushes carry no response flag.
    Stats = 0x0A,
    /// `voip -> core`: the media path ended. A notification, never a request.
    MediaEnded = 0x0B,
    /// `core -> voip`: one decoded PCM frame, inline copy.
    PcmIn = 0x10,
    /// `voip -> core`: one decoded PCM frame, inline copy.
    PcmOut = 0x11,
    /// `core -> voip`: one encoded audio packet, inline copy.
    EncodedAudioIn = 0x12,
    /// `voip -> core`: one encoded audio packet, inline copy.
    EncodedAudioOut = 0x13,
    /// `core -> voip`: one encoded video frame, inline copy.
    VideoIn = 0x14,
    /// `voip -> core`: one encoded video frame, inline copy.
    VideoOut = 0x15,
}

impl Opcode {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte; unknown values fail rather than alias.
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        let op = match v {
            0x01 => Opcode::Hello,
            0x02 => Opcode::Reserve,
            0x03 => Opcode::BeginOpen,
            0x04 => Opcode::Open,
            0x05 => Opcode::CancelOpen,
            0x06 => Opcode::Command,
            0x07 => Opcode::GroupFits,
            0x08 => Opcode::Close,
            0x09 => Opcode::Event,
            0x0A => Opcode::Stats,
            0x0B => Opcode::MediaEnded,
            0x10 => Opcode::PcmIn,
            0x11 => Opcode::PcmOut,
            0x12 => Opcode::EncodedAudioIn,
            0x13 => Opcode::EncodedAudioOut,
            0x14 => Opcode::VideoIn,
            0x15 => Opcode::VideoOut,
            other => return Err(DecodeError::UnknownOpcode(other)),
        };
        Ok(op)
    }

    /// Written-down name, so no `Debug` impl upstream can rename the wire.
    pub fn name(self) -> &'static str {
        match self {
            Opcode::Hello => "HELLO",
            Opcode::Reserve => "RESERVE",
            Opcode::BeginOpen => "BEGIN_OPEN",
            Opcode::Open => "OPEN",
            Opcode::CancelOpen => "CANCEL_OPEN",
            Opcode::Command => "COMMAND",
            Opcode::GroupFits => "GROUP_FITS",
            Opcode::Close => "CLOSE",
            Opcode::Event => "EVENT",
            Opcode::Stats => "STATS",
            Opcode::MediaEnded => "MEDIA_ENDED",
            Opcode::PcmIn => "PCM_IN",
            Opcode::PcmOut => "PCM_OUT",
            Opcode::EncodedAudioIn => "ENCODED_AUDIO_IN",
            Opcode::EncodedAudioOut => "ENCODED_AUDIO_OUT",
            Opcode::VideoIn => "VIDEO_IN",
            Opcode::VideoOut => "VIDEO_OUT",
        }
    }
}

/// Which end of the wire a `HELLO` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    /// The WhatsApp client side.
    Core = 0,
    /// The media engine side.
    Voip = 1,
}

impl Role {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Role::Core),
            1 => Some(Role::Voip),
            _ => None,
        }
    }
}

/// Capability bits exchanged in `HELLO`. A bit set by both sides enables the
/// behavior; anything else is refused with `NOT_SUPPORTED`, never half-run.
pub struct Capabilities;

impl Capabilities {
    /// 16 kHz mono PCM frame pumps.
    pub const PCM: u32 = 0x01;
    /// Opaque encoded audio packets (codec negotiated in the open params).
    pub const ENCODED_AUDIO: u32 = 0x02;
    /// Encoded video frames plus the video control commands.
    pub const VIDEO: u32 = 0x04;
    /// Group roster operations (`GROUP_FITS`, group events).
    pub const GROUP_CALLS: u32 = 0x08;
    /// Call-link joins.
    pub const CALL_LINKS: u32 = 0x10;
    /// Unsolicited `STATS` pushes from the engine side.
    pub const STATS_PUSH: u32 = 0x20;
    /// Relay reconnect signalling through the engine side.
    pub const RELAY_RECONNECT: u32 = 0x40;
}

/// Typed failure a response can carry. The grammar is fixed: a code plus an
/// optional detail, never a stacked message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AbiErrorCode {
    /// Peer major differs. Handshake only; fails before any call exists.
    BadVersion = 1,
    /// The handle was never reserved.
    UnknownSession = 2,
    /// The handle is known but the generation is not current. The message
    /// touched nothing; a stale session never touches its replacement.
    StaleGeneration = 3,
    /// The payload fails its own shape (truncated field, bad enum, ...).
    BadPayload = 4,
    /// The operation needs a capability the handshake did not agree on.
    NotSupported = 5,
    /// A second open is already in flight for this session.
    Busy = 6,
    /// Anything else on the responder side.
    Internal = 7,
    /// The media transport failed: refused connect, expired dial, dropped
    /// socket. Maps to `MediaSetupError::Connect`, never to setup.
    Transport = 8,
}

impl AbiErrorCode {
    /// The wire byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses a wire byte; unknown codes fail rather than alias.
    pub fn from_u8(v: u8) -> Option<Self> {
        let code = match v {
            1 => AbiErrorCode::BadVersion,
            2 => AbiErrorCode::UnknownSession,
            3 => AbiErrorCode::StaleGeneration,
            4 => AbiErrorCode::BadPayload,
            5 => AbiErrorCode::NotSupported,
            6 => AbiErrorCode::Busy,
            7 => AbiErrorCode::Internal,
            8 => AbiErrorCode::Transport,
            _ => return None,
        };
        Some(code)
    }

    /// Written-down name for logs on either side.
    pub fn name(self) -> &'static str {
        match self {
            AbiErrorCode::BadVersion => "BAD_VERSION",
            AbiErrorCode::UnknownSession => "UNKNOWN_SESSION",
            AbiErrorCode::StaleGeneration => "STALE_GENERATION",
            AbiErrorCode::BadPayload => "BAD_PAYLOAD",
            AbiErrorCode::NotSupported => "NOT_SUPPORTED",
            AbiErrorCode::Busy => "BUSY",
            AbiErrorCode::Internal => "INTERNAL",
            AbiErrorCode::Transport => "TRANSPORT",
        }
    }
}

/// A framed message: header plus the payload bytes that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The peer's major, checked before anything else is read.
    pub major: u8,
    /// The peer's minor; the lower of the two sides wins.
    pub minor: u8,
    /// The operation.
    pub opcode: Opcode,
    /// `RESPONSE` / `ERROR` bits.
    pub flags: u16,
    /// The payload bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Frames a request: current versions, no flags.
    pub fn request(opcode: Opcode, payload: Vec<u8>) -> Self {
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode,
            flags: 0,
            payload,
        }
    }

    /// Frames the success response to `request`: opcode echoed, `RESPONSE`.
    pub fn respond(request: &Frame, payload: Vec<u8>) -> Self {
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: request.opcode,
            flags: FLAG_RESPONSE,
            payload,
        }
    }

    /// Frames the failure response: opcode echoed, `RESPONSE | ERROR`, with
    /// the code and optional detail as the body.
    pub fn fail(request: &Frame, code: AbiErrorCode, detail: Option<&str>) -> Self {
        let mut w = crate::codec::Writer::new();
        w.u8(code.as_u8());
        w.option_str(detail);
        Frame {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
            opcode: request.opcode,
            flags: FLAG_RESPONSE | FLAG_ERROR,
            payload: w.finish(),
        }
    }

    /// True for a success response.
    pub fn is_response(&self) -> bool {
        self.flags & FLAG_RESPONSE != 0 && self.flags & FLAG_ERROR == 0
    }

    /// True for a failure response.
    pub fn is_error(&self) -> bool {
        self.flags & FLAG_RESPONSE != 0 && self.flags & FLAG_ERROR != 0
    }

    /// Serializes header plus payload into one buffer.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(self.major);
        out.push(self.minor);
        out.push(self.opcode.as_u8());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Parses one framed message. The major is returned unchecked for the
    /// handshake to judge; every other caller rejects a mismatch up front.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < HEADER_LEN {
            return Err(DecodeError::TruncatedHeader { len: buf.len() });
        }
        let magic: [u8; 4] = [buf[0], buf[1], buf[2], buf[3]];
        if magic != MAGIC {
            return Err(DecodeError::BadMagic { got: magic });
        }
        let opcode = Opcode::from_u8(buf[6])?;
        let flags = u16::from_le_bytes([buf[7], buf[8]]);
        let len = u32::from_le_bytes([buf[9], buf[10], buf[11], buf[12]]) as usize;
        if buf.len() - HEADER_LEN < len {
            return Err(DecodeError::TruncatedPayload {
                want: len,
                got: buf.len() - HEADER_LEN,
            });
        }
        Ok(Frame {
            major: buf[4],
            minor: buf[5],
            opcode,
            flags,
            payload: buf[HEADER_LEN..HEADER_LEN + len].to_vec(),
        })
    }
}
