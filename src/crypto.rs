//! Cryptographic operations exposed by the bridge without protocol-specific
//! naming or state.

use js_sys::Uint8Array;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
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

static ENGINE_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub(crate) fn mark_engine_initialized() {
    ENGINE_INITIALIZED.store(true, Ordering::Release);
}

fn require_engine_initialized() -> Result<(), BridgeError> {
    if ENGINE_INITIALIZED.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(BridgeError::InvalidArgument {
            field: "initWasmEngine".into(),
            reason: "must be called before AES-GCM operations".into(),
        })
    }
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
    require_engine_initialized()?;
    let key = fixed_bytes::<32>("key", key)?;
    let nonce = fixed_bytes::<12>("nonce", nonce)?;
    let mut output = Vec::with_capacity(plaintext.len() + 16);
    crate::js_crypto::default_provider()
        .aes_256_gcm_encrypt(&key, &nonce, aad, plaintext, &mut output)
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
    require_engine_initialized()?;
    let key = fixed_bytes::<32>("key", key)?;
    let nonce = fixed_bytes::<12>("nonce", nonce)?;
    if ciphertext_with_tag.len() < 16 {
        return Err(BridgeError::InvalidArgument {
            field: "ciphertextWithTag".into(),
            reason: "must include a 16-byte authentication tag".into(),
        });
    }
    let mut output = Vec::with_capacity(ciphertext_with_tag.len() - 16);
    crate::js_crypto::default_provider()
        .aes_256_gcm_decrypt(&key, &nonce, aad, ciphertext_with_tag, &mut output)
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
    let digest = hash
        .finalize_sha256_array()
        .map_err(|error| BridgeError::Internal {
            message: format!("finalize SHA-256: {error}"),
        })?;
    Ok(byte_array(&digest))
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

#[cfg(test)]
mod signal_checkpoint_tests {
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::wacore::libsignal::{
        core::{PreparedSigningKey, PrivateKey},
        engine::{CryptoProvider, Error, RandomSource, Result, RustCrypto},
    };

    struct FixedRandom;
    impl RandomSource for FixedRandom {
        fn fill(&mut self, destination: &mut [u8]) -> Result<()> {
            destination.fill(17);
            Ok(())
        }
    }
    struct FailedRandom;
    impl RandomSource for FailedRandom {
        fn fill(&mut self, _: &mut [u8]) -> Result<()> {
            Err(Error::Crypto)
        }
    }

    #[test]
    fn prepared_signature_matches_private_backend_and_rejects_tampering() {
        let private = PrivateKey::deserialize(&[7; 32]).unwrap();
        let public = private.public_key().unwrap();
        let other = PrivateKey::deserialize(&[8; 32])
            .unwrap()
            .public_key()
            .unwrap();
        let prepared = PreparedSigningKey::new(&private);
        let message = b"synthetic bridge signature";
        let signature = prepared
            .calculate_signature_with_random_source(message, &mut FixedRandom)
            .unwrap();
        assert_eq!(
            signature,
            RustCrypto
                .sign(private.serialize(), message, &mut FixedRandom)
                .unwrap()
        );
        assert!(public.verify_signature(message, &signature));
        assert!(!other.verify_signature(message, &signature));
        assert!(!public.verify_signature(b"changed", &signature));
        assert!(
            prepared
                .calculate_signature_with_random_source(message, &mut FailedRandom)
                .is_err()
        );
    }

    /// Pin an independent RFC5869 vector and the 255-block acceptance boundary.
    #[test]
    fn zero_salt_hkdf_matches_rfc5869_and_output_boundaries() {
        let mut actual = [0; 42];
        RustCrypto
            .hkdf_sha256_zero_salt(&[11; 22], b"", &mut actual)
            .unwrap();
        let expected = [
            0x8d, 0xa4, 0xe7, 0x75, 0xa5, 0x63, 0xc1, 0x8f, 0x71, 0x5f, 0x80, 0x2a, 0x06, 0x3c,
            0x5a, 0x31, 0xb8, 0xa1, 0x1f, 0x5c, 0x5e, 0xe1, 0x87, 0x9e, 0xc3, 0x45, 0x4e, 0x5f,
            0x3c, 0x73, 0x8d, 0x2d, 0x9d, 0x20, 0x13, 0x95, 0xfa, 0xa4, 0xb6, 0x1a, 0x96, 0xc8,
        ];
        assert_eq!(actual, expected);
        for input_length in [0, 63, 64, 65, 1024] {
            for length in [0, 31, 32, 33, 80, 255 * 32, 255 * 32 + 1] {
                let input = vec![9; input_length];
                let mut specialized = vec![0xa5; length];
                let mut generic = specialized.clone();
                let first = RustCrypto.hkdf_sha256_zero_salt(
                    &input,
                    b"WhisperMessageKeys",
                    &mut specialized,
                );
                let second =
                    RustCrypto.hkdf_sha256(&[0; 32], &input, b"WhisperMessageKeys", &mut generic);
                assert_eq!(first.is_ok(), length <= 255 * 32);
                assert_eq!(first, second);
                assert_eq!(specialized, generic);
            }
        }
    }
    /// Compare both CBC routes with independent Node vectors and existing prefixes.
    #[test]
    fn native_and_selected_cbc_append_match_node_vectors_and_preserve_prefix() {
        use whatsapp_rust::wacore::libsignal::crypto::{ProviderCrypto, RustCryptoProvider};
        let provider = RustCryptoProvider;
        let selected = ProviderCrypto::new(&provider);
        // Digests generated independently with Node crypto AES-256-CBC/PKCS#7.
        for (length, expected) in [
            (
                0,
                "0f37715d5a35131aa7dd1f4ca32a0a27db73e54f76fea45db11d9a6704ae39d7",
            ),
            (
                1,
                "3b6ec739359e2b30ba639d60dbe1f4f667b62ae80acf303fc81434b06abdc06d",
            ),
            (
                15,
                "345ba30db042c277a9da567140ad278c237e3be641bf069c0eaaff564b1a8e60",
            ),
            (
                16,
                "05ff56598eb2ce8e9b1cc6083ca02c2ef6b4b36416c629f087b13006134bfb98",
            ),
            (
                17,
                "d598d8f5667c50334a06f191aff5b998613cab41989f53d9407ba5ab3f0a6af7",
            ),
            (
                127,
                "4767cbbf2275bd36429fd3146ab8935b1bcd47f377d2fdbaeef5d76a86982109",
            ),
            (
                128,
                "fc26e51de9f41d6b149d618a75580051e7e84947745108bfde715d5bde23bc9d",
            ),
            (
                129,
                "9a36d70c011fa7f98805e532029b1cb72dd9b0cc763da5fb60fa291f8ea3183e",
            ),
            (
                512,
                "6b4a539221e18690426c19c21cfac6d1fe7bf623a8735ecaed97eb282292f001",
            ),
            (
                8192,
                "49c5b864161b14e0145b53789adc16e092698733fba58333a2c43eb255f80a8b",
            ),
        ] {
            let plaintext = vec![(length % 251) as u8; length];
            for prefix_length in [0, 1, 47, 64, 129] {
                for crypto in [&RustCrypto as &dyn CryptoProvider, &selected] {
                    let mut output = Vec::with_capacity(prefix_length + (length / 16 + 1) * 16);
                    output.resize(prefix_length, 9);
                    let pointer = output.as_ptr();
                    crypto
                        .encrypt_cbc_into(&[3; 32], &[4; 16], &plaintext, &mut output)
                        .unwrap();
                    assert_eq!(output.as_ptr(), pointer);
                    assert_eq!(&output[..prefix_length], vec![9; prefix_length]);
                    assert_eq!(output.len() - prefix_length, (length / 16 + 1) * 16);
                    let digest = crypto.sha256(&output[prefix_length..]);
                    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
                    assert_eq!(hex, expected);
                }
            }
        }
    }
    /// A selected provider must see the real destination and restore failed writes.
    #[test]
    fn selected_provider_receives_destination_and_restores_partial_failures() {
        use whatsapp_rust::wacore::libsignal::crypto::{
            CryptoProviderError, ProviderCrypto, RustCryptoProvider, SymmetricCryptoProvider,
        };
        struct Append {
            mode: u8,
        }
        impl SymmetricCryptoProvider for Append {
            /// Write partial bytes before injected failure to test envelope rollback.
            fn aes_256_cbc_encrypt(
                &self,
                key: &[u8; 32],
                iv: &[u8; 16],
                plaintext: &[u8],
                output: &mut Vec<u8>,
            ) -> std::result::Result<(), CryptoProviderError> {
                assert_eq!(
                    output, &[9; 47],
                    "provider must receive the existing envelope prefix"
                );
                if self.mode == 0 {
                    RustCryptoProvider.aes_256_cbc_encrypt(key, iv, plaintext, output)
                } else {
                    output.extend_from_slice(plaintext);
                    if self.mode == 1 {
                        Err(CryptoProviderError::BackendFailed)
                    } else {
                        Ok(())
                    }
                }
            }
        }
        for mode in [0, 1, 2] {
            let provider = Append { mode };
            let mut output = Vec::with_capacity(47 + 32);
            output.extend_from_slice(&[9; 47]);
            let pointer = output.as_ptr();
            let result = ProviderCrypto::new(&provider).encrypt_cbc_into(
                &[3; 32],
                &[4; 16],
                b"partial plaintext",
                &mut output,
            );
            assert_eq!(&output[..47], &[9; 47]);
            if mode == 0 {
                assert!(result.is_ok());
                assert_eq!(output.len(), 47 + 32);
                assert_eq!(output.as_ptr(), pointer);
            } else {
                assert!(result.is_err());
                assert_eq!(
                    output, [9; 47],
                    "failed or malformed append must discard its suffix"
                );
            }
        }
    }
}
