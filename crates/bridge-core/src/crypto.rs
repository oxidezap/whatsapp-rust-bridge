//! Cryptographic operations exposed by the bridge without protocol-specific
//! naming or state.

use whatsapp_rust::wacore::crypto as core_crypto;
use whatsapp_rust::wacore::libsignal::protocol::{PrivateKey, PublicKey};

use crate::{CoreError, CoreResult};

/// A serialized curve key pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPair {
    pub pub_key: Vec<u8>,
    pub priv_key: Vec<u8>,
}

pub fn md5_digest(input: &[u8]) -> Vec<u8> {
    core_crypto::md5_digest(input).to_vec()
}

pub fn hkdf_sha256(
    input_key_material: &[u8],
    expanded_length: usize,
    salt: Option<&[u8]>,
    info: &[u8],
) -> CoreResult<Vec<u8>> {
    core_crypto::hkdf_sha256(input_key_material, expanded_length, salt, info)
        .map_err(CoreError::from_display)
}

pub fn generate_key_pair() -> KeyPair {
    let pair = core_crypto::generate_curve_key_pair();
    KeyPair {
        pub_key: pair.public_key.serialize().to_vec(),
        priv_key: pair.private_key.serialize().to_vec(),
    }
}

pub fn public_from_private_key(private_key: &[u8]) -> CoreResult<Vec<u8>> {
    let private = PrivateKey::deserialize(private_key).map_err(CoreError::from_display)?;
    let public = private.public_key().map_err(CoreError::from_display)?;
    Ok(public.serialize().to_vec())
}

pub fn calculate_agreement(public_key: &[u8], private_key: &[u8]) -> CoreResult<Vec<u8>> {
    let public = PublicKey::deserialize(public_key).map_err(CoreError::from_display)?;
    let private = PrivateKey::deserialize(private_key).map_err(CoreError::from_display)?;
    private
        .calculate_agreement(&public)
        .map(|agreement| agreement.to_vec())
        .map_err(CoreError::from_display)
}

pub fn calculate_signature(private_key: &[u8], message: &[u8]) -> CoreResult<Vec<u8>> {
    let private = PrivateKey::deserialize(private_key).map_err(CoreError::from_display)?;
    core_crypto::calculate_curve_signature(&private, message)
        .map(|signature| signature.to_vec())
        .map_err(CoreError::from_display)
}

pub fn verify_signature(public_key: &[u8], message: &[u8], signature: &[u8]) -> CoreResult<bool> {
    let public = PublicKey::deserialize(public_key).map_err(CoreError::from_display)?;
    Ok(public.verify_signature(message, signature))
}
