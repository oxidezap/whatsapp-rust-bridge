//! Session identity: a `u32` handle plus a `u64` generation, validated on
//! every callback. A stale generation never touches its replacement, which
//! keeps the ABA protection across the WASM boundary.

use std::collections::HashMap;

/// The identity every session message carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId {
    /// Compact handle naming the session slot.
    pub handle: u32,
    /// Monotonic generation: replacement sessions reuse the handle with a
    /// newer generation, so a delayed callback from the old one is stale.
    pub generation: u64,
}

/// What went wrong validating a session identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The handle was never reserved (or was already closed).
    Unknown,
    /// The handle is known but this is not its current generation. The
    /// message must be dropped without touching the replacement.
    Stale {
        /// The generation the handle currently holds.
        current: u64,
    },
    /// `RESERVE` on a handle that is still reserved. Replacement is `CLOSE`
    /// then `RESERVE`, never an overwrite.
    HandleInUse {
        /// The generation currently holding the handle.
        current: u64,
    },
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SessionError::Unknown => write!(f, "unknown voip session handle"),
            SessionError::Stale { current } => write!(
                f,
                "stale voip session generation (handle now holds generation {current})"
            ),
            SessionError::HandleInUse { current } => write!(
                f,
                "voip session handle still reserved at generation {current}"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

/// What a reservation binds a handle to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    /// The call this handle stands for.
    pub call_id: String,
    /// The generation currently holding the handle.
    pub generation: u64,
}

/// Handle-to-reservation registry with generation checks. Both sides keep
/// one: the core validates before dispatching, the engine before applying.
#[derive(Debug, Default)]
pub struct SessionTable {
    reserved: HashMap<u32, Reservation>,
}

impl SessionTable {
    /// An empty registry.
    pub fn new() -> Self {
        SessionTable {
            reserved: HashMap::new(),
        }
    }

    /// Binds `session` to `call_id`. Fails when the handle is still
    /// reserved, whatever the generation: replacement closes first.
    pub fn reserve(&mut self, session: SessionId, call_id: &str) -> Result<(), SessionError> {
        if let Some(current) = self.reserved.get(&session.handle) {
            return Err(SessionError::HandleInUse {
                current: current.generation,
            });
        }
        self.reserved.insert(
            session.handle,
            Reservation {
                call_id: call_id.to_owned(),
                generation: session.generation,
            },
        );
        Ok(())
    }

    /// Checks a callback's identity: unknown handles and stale generations
    /// both fail, and only an exact generation passes.
    pub fn validate(&self, session: SessionId) -> Result<&Reservation, SessionError> {
        match self.reserved.get(&session.handle) {
            None => Err(SessionError::Unknown),
            Some(current) if current.generation != session.generation => Err(SessionError::Stale {
                current: current.generation,
            }),
            Some(current) => Ok(current),
        }
    }

    /// Releases a handle. Closing is idempotent: dropping half-open state
    /// twice is normal on the teardown path.
    pub fn remove(&mut self, handle: u32) {
        self.reserved.remove(&handle);
    }

    /// True while the handle is reserved at any generation.
    pub fn contains(&self, handle: u32) -> bool {
        self.reserved.contains_key(&handle)
    }
}
