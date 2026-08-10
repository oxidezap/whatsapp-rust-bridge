//! BoltFFI exposure of the neutral bridge core.
//!
//! Every function here forwards to `whatsapp_rust_bridge_core`, the same crate
//! the wasm-bindgen artifact calls. JS names match the wasm-bindgen surface,
//! so the two artifacts answer the same call with the same value.

use boltffi::*;
use whatsapp_rust_bridge_core as bridge_core;

/// The free-standing utilities cross as a plain message on the wasm-bindgen
/// path (`error_value` renders `Display`). Carrying the same rendered string
/// keeps the two backends reporting identically.
#[error]
#[derive(Clone, Debug, PartialEq)]
#[repr(i32)]
pub enum BridgeUtilError {
    Failed(String) = 0,
}

impl std::fmt::Display for BridgeUtilError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for BridgeUtilError {}

impl From<UnexpectedFfiCallbackError> for BridgeUtilError {
    fn from(error: UnexpectedFfiCallbackError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<bridge_core::CoreError> for BridgeUtilError {
    fn from(error: bridge_core::CoreError) -> Self {
        Self::Failed(error.message().to_owned())
    }
}

type Result<T> = bridge_core::CoreResult<T>;

fn lift<T>(result: Result<T>) -> std::result::Result<T, BridgeUtilError> {
    result.map_err(BridgeUtilError::from)
}

#[export]
pub fn md5(input: Vec<u8>) -> Vec<u8> {
    bridge_core::crypto::md5_digest(&input)
}

/// Mirrors the wasm-bindgen `HkdfInfo` parameter, so `hkdf` takes the same
/// third argument on both backends rather than two positional ones here.
#[data]
#[derive(Clone, Debug, Default)]
pub struct HkdfInfo {
    pub salt: Option<Vec<u8>>,
    pub info: Option<String>,
}

#[export]
pub fn hkdf(
    input_key_material: Vec<u8>,
    expanded_length: u32,
    options: HkdfInfo,
) -> std::result::Result<Vec<u8>, BridgeUtilError> {
    lift(bridge_core::crypto::hkdf_sha256(
        &input_key_material,
        expanded_length as usize,
        options.salt.as_deref(),
        options.info.unwrap_or_default().as_bytes(),
    ))
}

// `generateKeyPair` and `calculateSignature` are deliberately absent. They are
// the operations on this surface that draw randomness, and the repository's
// `.cargo/config.toml` builds `getrandom` with `getrandom_backend="wasm_js"`
// for every wasm target. That backend calls `__wbg_getRandomValues_*`, a
// wasm-bindgen import the BoltFFI runtime does not provide, so the call fails
// on import resolution. One `getrandom` is built per target for the whole
// workspace, so this crate cannot select a different backend while the cfg is
// set repository-wide.
//
// `tests/backend-equivalence.test.ts` asserts the absence rather than leaving
// it to be discovered.

#[export]
pub fn get_public_from_private_key(
    private_key: Vec<u8>,
) -> std::result::Result<Vec<u8>, BridgeUtilError> {
    lift(bridge_core::crypto::public_from_private_key(&private_key))
}

#[export]
pub fn calculate_agreement(
    public_key: Vec<u8>,
    private_key: Vec<u8>,
) -> std::result::Result<Vec<u8>, BridgeUtilError> {
    lift(bridge_core::crypto::calculate_agreement(
        &public_key,
        &private_key,
    ))
}

#[export]
pub fn verify_signature(
    public_key: Vec<u8>,
    message: Vec<u8>,
    signature: Vec<u8>,
) -> std::result::Result<bool, BridgeUtilError> {
    lift(bridge_core::crypto::verify_signature(
        &public_key,
        &message,
        &signature,
    ))
}

#[export]
pub fn inflate_zlib(
    data: Vec<u8>,
    max_output_bytes: Option<f64>,
) -> std::result::Result<Vec<u8>, BridgeUtilError> {
    lift(bridge_core::compression::inflate_zlib(
        &data,
        max_output_bytes,
    ))
}

#[export]
pub fn decrypt_poll_vote_payload(
    enc_payload: Vec<u8>,
    enc_iv: Vec<u8>,
    message_secret: Vec<u8>,
    stanza_id: String,
    poll_creator_jid: String,
    voter_jid: String,
) -> std::result::Result<Vec<u8>, BridgeUtilError> {
    lift(bridge_core::addon_crypto::decrypt_poll_vote_payload(
        &enc_payload,
        &enc_iv,
        &message_secret,
        &stanza_id,
        &poll_creator_jid,
        &voter_jid,
    ))
}

#[export]
pub fn decrypt_event_response_payload(
    enc_payload: Vec<u8>,
    enc_iv: Vec<u8>,
    message_secret: Vec<u8>,
    stanza_id: String,
    event_creator_jid: String,
    responder_jid: String,
) -> std::result::Result<Vec<u8>, BridgeUtilError> {
    lift(bridge_core::addon_crypto::decrypt_event_response_payload(
        &enc_payload,
        &enc_iv,
        &message_secret,
        &stanza_id,
        &event_creator_jid,
        &responder_jid,
    ))
}
