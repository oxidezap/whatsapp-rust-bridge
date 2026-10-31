//! Chat actions, labels and quick replies (app state).
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

include!("generated/chat_actions.rs");
