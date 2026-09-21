//! Cryptographic operations exposed by the bridge without protocol-specific
//! naming or state.

use js_sys::Uint8Array;
use serde::{Deserialize, Serialize};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::crypto as core_crypto;
use whatsapp_rust::wacore::libsignal::crypto as signal_crypto;
use whatsapp_rust::wacore::libsignal::protocol::{PrivateKey, PublicKey};

use crate::errors::BridgeError;

use crate::wasm_utils::{byte_array, error_value};

#[derive(Debug, Clone, Serialize, Tsify)]
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
#[serde(rename_all = "camelCase", default)]
pub struct HkdfInfo {
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub salt: Option<Vec<u8>>,
    pub info: Option<String>,
}

#[wasm_bindgen(js_name = md5)]
pub fn md5_digest(input: &[u8]) -> Uint8Array {
    byte_array(&core_crypto::md5_digest(input))
}

fn fixed_bytes<const N: usize>(field: &'static str, input: &[u8]) -> Result<[u8; N], BridgeError> {
    input.try_into().map_err(|_| BridgeError::InvalidArgument {
        field: field.into(),
        reason: format!("must be exactly {N} bytes"),
    })
}

fn crypto_failure(
    operation: &'static str,
    error: signal_crypto::CryptoProviderError,
) -> BridgeError {
    BridgeError::Crypto {
        operation: format!("{operation}: {error}"),
    }
}

/// Encrypt with the active signal crypto provider. The returned bytes are
/// `ciphertext || authentication tag`.
#[wasm_bindgen(js_name = aesGcm256Encrypt)]
pub fn aes_gcm_256_encrypt(
    key: &[u8],
    nonce: &[u8],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Uint8Array, BridgeError> {
    let key = fixed_bytes::<32>("key", key)?;
    let nonce = fixed_bytes::<12>("nonce", nonce)?;
    let mut output = Vec::with_capacity(plaintext.len() + 16);
    signal_crypto::aes_256_gcm_encrypt(&key, &nonce, aad, plaintext, &mut output)
        .map_err(|error| crypto_failure("AES-256-GCM encrypt", error))?;
    Ok(byte_array(&output))
}

/// Decrypt bytes returned by [`aes_gcm_256_encrypt`].
#[wasm_bindgen(js_name = aesGcm256Decrypt)]
pub fn aes_gcm_256_decrypt(
    key: &[u8],
    nonce: &[u8],
    aad: &[u8],
    ciphertext_with_tag: &[u8],
) -> Result<Uint8Array, BridgeError> {
    let key = fixed_bytes::<32>("key", key)?;
    let nonce = fixed_bytes::<12>("nonce", nonce)?;
    if ciphertext_with_tag.len() < 16 {
        return Err(BridgeError::InvalidArgument {
            field: "ciphertextWithTag".into(),
            reason: "must include a 16-byte authentication tag".into(),
        });
    }
    let mut output = Vec::with_capacity(ciphertext_with_tag.len() - 16);
    signal_crypto::aes_256_gcm_decrypt(&key, &nonce, aad, ciphertext_with_tag, &mut output)
        .map_err(|error| crypto_failure("AES-256-GCM decrypt", error))?;
    Ok(byte_array(&output))
}

/// Hash arbitrary bytes with SHA-256.
#[wasm_bindgen(js_name = sha256)]
pub fn sha256_digest(input: &[u8]) -> Result<Uint8Array, BridgeError> {
    let mut hash = signal_crypto::CryptographicHash::new("SHA-256").map_err(|error| {
        BridgeError::Internal {
            message: format!("initialize SHA-256: {error}"),
        }
    })?;
    hash.update(input);
    Ok(byte_array(&hash.finalize()))
}

#[wasm_bindgen(js_name = hkdf)]
pub fn hkdf_sha256(
    input_key_material: &[u8],
    expanded_length: usize,
    options: Ts<HkdfInfo>,
) -> Result<Uint8Array, JsValue> {
    let options = options.to_rust().map_err(error_value)?;
    let output = core_crypto::hkdf_sha256(
        input_key_material,
        expanded_length,
        options.salt.as_deref(),
        options.info.as_deref().unwrap_or_default().as_bytes(),
    )
    .map_err(error_value)?;
    Ok(byte_array(&output))
}

#[wasm_bindgen(js_name = generateKeyPair)]
pub fn generate_key_pair() -> Result<Ts<KeyPair>, JsValue> {
    let pair = core_crypto::generate_curve_key_pair();
    KeyPair {
        pub_key: pair.public_key.serialize().to_vec(),
        priv_key: pair.private_key.serialize().to_vec(),
    }
    .into_ts()
    .map_err(error_value)
}

#[wasm_bindgen(js_name = getPublicFromPrivateKey)]
pub fn public_from_private_key(private_key: &[u8]) -> Result<Uint8Array, JsValue> {
    let private = PrivateKey::deserialize(private_key).map_err(error_value)?;
    let public = private.public_key().map_err(error_value)?;
    Ok(byte_array(&public.serialize()))
}

#[wasm_bindgen(js_name = calculateAgreement)]
pub fn calculate_agreement(public_key: &[u8], private_key: &[u8]) -> Result<Uint8Array, JsValue> {
    let public = PublicKey::deserialize(public_key).map_err(error_value)?;
    let private = PrivateKey::deserialize(private_key).map_err(error_value)?;
    Ok(byte_array(
        &private.calculate_agreement(&public).map_err(error_value)?,
    ))
}

#[wasm_bindgen(js_name = calculateSignature)]
pub fn calculate_signature(private_key: &[u8], message: &[u8]) -> Result<Uint8Array, JsValue> {
    let private = PrivateKey::deserialize(private_key).map_err(error_value)?;
    Ok(byte_array(
        &core_crypto::calculate_curve_signature(&private, message).map_err(error_value)?,
    ))
}

#[wasm_bindgen(js_name = verifySignature)]
pub fn verify_signature(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<bool, JsValue> {
    let public = PublicKey::deserialize(public_key).map_err(error_value)?;
    Ok(public.verify_signature(message, signature))
}
