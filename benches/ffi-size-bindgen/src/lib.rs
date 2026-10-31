//! Size fixture for the eight-operation API shared with BoltFFI.
//! scripts/measure-ffi-size.ts verifies the copied wrappers against production.
use crate::wasm_utils::{byte_array, error_value};
use js_sys::Uint8Array;
use serde::Deserialize;
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;
use whatsapp_rust_bridge_core::crypto as bridge_core;
#[derive(Debug, Clone, Default, Deserialize, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct HkdfInfo {
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub salt: Option<Vec<u8>>,
    pub info: Option<String>,
}
#[wasm_bindgen(js_name = md5)]
pub fn md5_digest(input: &[u8]) -> Uint8Array {
    byte_array(&bridge_core::md5_digest(input))
}
#[wasm_bindgen(js_name = hkdf)]
pub fn hkdf_sha256(
    input_key_material: &[u8],
    expanded_length: usize,
    options: Ts<HkdfInfo>,
) -> Result<Uint8Array, JsValue> {
    let options = options.to_rust().map_err(error_value)?;
    let output = bridge_core::hkdf_sha256(
        input_key_material,
        expanded_length,
        options.salt.as_deref(),
        options.info.as_deref().unwrap_or_default().as_bytes(),
    )
    .map_err(error_value)?;
    Ok(byte_array(&output))
}
#[wasm_bindgen(js_name = getPublicFromPrivateKey)]
pub fn public_from_private_key(private_key: &[u8]) -> Result<Uint8Array, JsValue> {
    Ok(byte_array(
        &bridge_core::public_from_private_key(private_key).map_err(error_value)?,
    ))
}
#[wasm_bindgen(js_name = calculateAgreement)]
pub fn calculate_agreement(public_key: &[u8], private_key: &[u8]) -> Result<Uint8Array, JsValue> {
    Ok(byte_array(
        &bridge_core::calculate_agreement(public_key, private_key).map_err(error_value)?,
    ))
}
#[wasm_bindgen(js_name = verifySignature)]
pub fn verify_signature(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<bool, JsValue> {
    bridge_core::verify_signature(public_key, message, signature).map_err(error_value)
}

#[path = "../../../src/addon_crypto.rs"]
mod addon_crypto;
#[path = "../../../src/compression.rs"]
mod compression;
#[path = "../../../src/wasm_utils.rs"]
mod wasm_utils;
