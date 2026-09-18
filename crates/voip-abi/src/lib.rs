//! Versioned binary ABI between `core.wasm` and `voip.wasm`.
//!
//! The two modules never share memory: no pointer crosses, no `repr(C)`
//! struct layout is ABI, and no `Arc`, trait object, `Client`, or call handle
//! travels. What crosses is bytes — a 13-byte header and a compact payload —
//! encoded and decoded by hand in [`codec`], with no serialization framework
//! on either side (see `Cargo.toml`: zero dependencies).
//!
//! ```text
//! header: magic[4] | major u8 | minor u8 | opcode u8 | flags u16 LE | payload_len u32 LE
//! ```
//!
//! `major` equal is mandatory: a peer whose major differs fails the handshake
//! before any call exists (see [`negotiate`]). `minor` is additive-only: a
//! decoder rejects a truncated payload but ignores trailing bytes, so a newer
//! minor may append fields an older reader skips. [`messages`] pins the
//! trailing-tolerance with a test.
//!
//! Responses echo the request opcode with the `RESPONSE` flag; failures add
//! the `ERROR` flag and carry an [`AbiErrorCode`] plus an optional detail.
//! Three notifications flow `voip -> core` without a response: [`Opcode::Open`]
//! (setup completed), [`Opcode::Event`], [`Opcode::MediaEnded`], and
//! [`Opcode::Stats`] pushes. Everything else is a request.
//!
//! The open handshake is three messages, because setup is asynchronous and
//! the core may walk away mid-flight:
//!
//! ```text
//! core -> voip   BEGIN_OPEN  (session, generation, params; ack or ERROR)
//! voip -> core   OPEN        (setup complete, media flowing)
//! core -> voip   CANCEL_OPEN (abort; ack carries the outcome)
//! ```
//!
//! Identity on every session message is [`SessionId`]: a `u32` handle plus a
//! `u64` generation. A callback that arrives with a stale generation never
//! touches the replacement ([`session::SessionTable`] enforces this, keeping
//! the ABA protection across the WASM boundary).
//!
//! Media travels on six separate frame opcodes ([`Opcode::PcmIn`] etc.):
//! `_In` flows `core -> voip`, `_Out` flows `voip -> core`. Copy semantics
//! (v1): each frame carries its bytes inline. A shared-memory ring is a
//! later ABI_MAJOR only if measurement earns it.

pub mod codec;
pub mod messages;
pub mod protocol;
pub mod secret;
pub mod session;

pub use codec::{DecodeError, Reader, Writer};
pub use messages::*;
pub use protocol::{
    AbiErrorCode, Capabilities, FLAG_ERROR, FLAG_RESPONSE, Frame, HEADER_LEN, MAGIC, Opcode, Role,
};
pub use secret::SecretBytes;
pub use session::{SessionError, SessionId, SessionTable};

/// Wire major. Any peer whose major differs is incompatible, full stop.
pub const ABI_MAJOR: u8 = 1;
/// Wire minor. Additive fields only; decoders ignore trailing payload bytes.
pub const ABI_MINOR: u8 = 0;

/// The minor both sides agree to speak after a successful handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agreed {
    /// `min(local_minor, peer_minor)`: the newer side speaks down.
    pub minor: u8,
}

/// Failure to agree on a wire version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionError {
    /// `peer_major != ABI_MAJOR`. Fails before any client exists.
    MajorMismatch { peer_major: u8 },
}

impl core::fmt::Display for VersionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VersionError::MajorMismatch { peer_major } => write!(
                f,
                "incompatible voip ABI major: peer speaks {peer_major}, this side speaks {}",
                ABI_MAJOR
            ),
        }
    }
}

impl std::error::Error for VersionError {}

/// Agree on a wire version from the two headers' majors/minors.
///
/// The major must match exactly; the minor settles on the lower of the two,
/// whose reader both sides already satisfy by ignoring trailing bytes.
pub fn negotiate(peer_major: u8, peer_minor: u8, local_minor: u8) -> Result<Agreed, VersionError> {
    if peer_major != ABI_MAJOR {
        return Err(VersionError::MajorMismatch { peer_major });
    }
    Ok(Agreed {
        minor: peer_minor.min(local_minor),
    })
}
