//! Backend-neutral bridge logic.
//!
//! Nothing here knows about wasm-bindgen or BoltFFI. Both exposure layers call
//! into these functions, so a behaviour change lands in one place and reaches
//! both generated artifacts.

pub mod addon_crypto;
pub mod compression;
pub mod crypto;

use core::fmt;

/// The single failure shape the neutral layer reports.
///
/// The free-standing utilities historically crossed as a plain string (see
/// `error_value` in the wasm-bindgen layer), so carrying the rendered message
/// keeps both backends reporting exactly what they reported before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    message: String,
}

impl CoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn from_display(error: impl fmt::Display) -> Self {
        Self::new(error.to_string())
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}

pub type CoreResult<T> = Result<T, CoreError>;
