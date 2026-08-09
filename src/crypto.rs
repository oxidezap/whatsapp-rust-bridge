//! wasm-bindgen exposure for the neutral crypto operations.
//!
//! The logic lives in `whatsapp_rust_bridge_core::crypto`; this file only
//! declares the JS surface and converts at the boundary.

use js_sys::Uint8Array;
use serde::{Deserialize, Serialize};
use tsify::Tsify;
use wasm_bindgen::prelude::*;
use whatsapp_rust_bridge_core::crypto as bridge_core;

use crate::wasm_utils::{byte_array, error_value};

#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct KeyPair {
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub pub_key: Vec<u8>,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub priv_key: Vec<u8>,
}

#[derive(Debug, Clone, Default, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
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
    options: HkdfInfo,
) -> Result<Uint8Array, JsValue> {
    let output = bridge_core::hkdf_sha256(
        input_key_material,
        expanded_length,
        options.salt.as_deref(),
        options.info.as_deref().unwrap_or_default().as_bytes(),
    )
    .map_err(error_value)?;
    Ok(byte_array(&output))
}

#[wasm_bindgen(js_name = generateKeyPair)]
pub fn generate_key_pair() -> KeyPair {
    let pair = bridge_core::generate_key_pair();
    KeyPair {
        pub_key: pair.pub_key,
        priv_key: pair.priv_key,
    }
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

#[wasm_bindgen(js_name = calculateSignature)]
pub fn calculate_signature(private_key: &[u8], message: &[u8]) -> Result<Uint8Array, JsValue> {
    Ok(byte_array(
        &bridge_core::calculate_signature(private_key, message).map_err(error_value)?,
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
