//! Key material that zeroes itself. Any field the spec calls a token or a
//! key crosses as [`SecretBytes`] and is wiped on drop on both sides, so a
//! heap dump after the call holds no call key, relay token, auth token, or
//! integrity key.

/// Owned secret bytes, redacted in `Debug` and zeroed on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretBytes {
    bytes: Vec<u8>,
}

impl SecretBytes {
    /// Wraps caller-owned bytes as secret.
    pub fn new(bytes: Vec<u8>) -> Self {
        SecretBytes { bytes }
    }

    /// An empty secret.
    pub fn empty() -> Self {
        SecretBytes { bytes: Vec::new() }
    }

    /// The secret bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl core::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SecretBytes([redacted; {} bytes])", self.bytes.len())
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        // Volatile so the wipe survives dead-store elimination.
        for b in self.bytes.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
}
