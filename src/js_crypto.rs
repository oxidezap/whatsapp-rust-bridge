//! `SignalCryptoProvider` backed by host JS crypto callbacks.
//!
//! When installed, large AES-CBC / AES-GCM operations are delegated to the
//! host (e.g. `node:crypto`), which runs them in native OpenSSL. Small AES,
//! HMAC-SHA256, and the high-frequency Noise transport stay in Rust: crossing
//! JS and constructing a native cipher for every small frame costs more than
//! the soft implementation and creates avoidable V8 allocation pressure.
//!
//! Expected JS interface (see `JsCryptoCallbacks` in the generated `.d.ts`):
//! ```ts
//! interface JsCryptoCallbacks {
//!   aesCbc256Encrypt(key: Uint8Array, iv: Uint8Array, pt: Uint8Array): Uint8Array;
//!   aesCbc256Decrypt(key: Uint8Array, iv: Uint8Array, ct: Uint8Array): Uint8Array;
//!   aesGcm256Encrypt(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, pt: Uint8Array): Uint8Array; // ct||tag
//!   aesGcm256Decrypt(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, ct: Uint8Array): Uint8Array; // throws on auth fail
//!   hmacSha256(key: Uint8Array, data: Uint8Array): Uint8Array; // exactly 32 bytes
//!   hmacSha256TwoPart?(key: Uint8Array, first: Uint8Array, second: Uint8Array): Uint8Array;
//! }
//! ```

use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::libsignal::crypto::{
    CryptoProviderError, GcmInPlaceBuffer, RustCryptoProvider, SignalCryptoProvider, TransportAead,
    set_crypto_provider,
};

/// Below this payload size the JS/FFI/native-cipher setup costs more than the
/// software AES operation. This boundary was measured on Node/Linux and keeps
/// ordinary Signal messages on the allocation-free Rust path while retaining
/// native acceleration for large payloads.
const NATIVE_AES_MIN_BYTES: usize = 512;
const NATIVE_HMAC_MIN_BYTES: usize = NATIVE_AES_MIN_BYTES;
const AES_CBC_BLOCK_BYTES: usize = 16;
const GCM_AUTH_TAG_BYTES: usize = 16;
const HMAC_SHA256_BYTES: usize = 32;

#[wasm_bindgen(typescript_custom_section)]
const TS_CRYPTO: &str = r#"
/**
 * Native crypto callbacks installed via `initWasmEngine` for large AES operations.
 * Supply an implementation backed by the host platform's native crypto API.
 */
export interface JsCryptoCallbacks {
    aesCbc256Encrypt(key: Uint8Array, iv: Uint8Array, plaintext: Uint8Array): Uint8Array;
    aesCbc256Decrypt(key: Uint8Array, iv: Uint8Array, ciphertext: Uint8Array): Uint8Array;
    aesGcm256Encrypt(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, plaintext: Uint8Array): Uint8Array;
    aesGcm256Decrypt(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, ciphertextWithTag: Uint8Array): Uint8Array;
    hmacSha256(key: Uint8Array, data: Uint8Array): Uint8Array;
    hmacSha256TwoPart?(key: Uint8Array, first: Uint8Array, second: Uint8Array): Uint8Array;
}
"#;

/// Adapter that stores raw JS function handles and dispatches to them.
pub struct JsCryptoAdapter {
    aes_cbc_encrypt: js_sys::Function,
    aes_cbc_decrypt: js_sys::Function,
    aes_gcm_encrypt: js_sys::Function,
    aes_gcm_decrypt: js_sys::Function,
    hmac_sha256: js_sys::Function,
    hmac_sha256_two_part: Option<js_sys::Function>,
}

crate::wasm_send_sync!(JsCryptoAdapter);

impl JsCryptoAdapter {
    pub fn from_js(obj: &JsValue) -> Result<Self, JsValue> {
        fn extract(obj: &JsValue, name: &str) -> Result<js_sys::Function, JsValue> {
            js_sys::Reflect::get(obj, &JsValue::from_str(name))?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| JsValue::from_str(&format!("crypto.{name} must be a function")))
        }
        fn extract_optional(
            obj: &JsValue,
            name: &str,
        ) -> Result<Option<js_sys::Function>, JsValue> {
            let value = js_sys::Reflect::get(obj, &JsValue::from_str(name))?;
            if value.is_null() || value.is_undefined() {
                return Ok(None);
            }
            value
                .dyn_into::<js_sys::Function>()
                .map(Some)
                .map_err(|_| JsValue::from_str(&format!("crypto.{name} must be a function")))
        }
        Ok(Self {
            aes_cbc_encrypt: extract(obj, "aesCbc256Encrypt")?,
            aes_cbc_decrypt: extract(obj, "aesCbc256Decrypt")?,
            aes_gcm_encrypt: extract(obj, "aesGcm256Encrypt")?,
            aes_gcm_decrypt: extract(obj, "aesGcm256Decrypt")?,
            hmac_sha256: extract(obj, "hmacSha256")?,
            hmac_sha256_two_part: extract_optional(obj, "hmacSha256TwoPart")?,
        })
    }
}

/// Append the bytes of a JS `Uint8Array` to `out` without pre-zeroing the tail.
///
/// `Vec::resize(new_len, 0)` would memset-zero the grown region before `copy_to`
/// overwrites it — pure waste. We use `reserve` + `spare_capacity_mut` +
/// `set_len` to let `copy_to` write straight into the uninit tail.
#[inline]
fn append_returned_bytes(out: &mut Vec<u8>, value: JsValue) -> Result<(), CryptoProviderError> {
    let arr: Uint8Array = value
        .dyn_into()
        .map_err(|_| CryptoProviderError::BackendFailed)?;
    let len = arr.length() as usize;
    let start = out.len();
    out.reserve(len);
    // SAFETY: reserve() above guarantees capacity for `len` more bytes. We form
    // a &mut [u8] over the uninit tail, call copy_to which does a JS→WASM
    // memcpy fully initializing those bytes, then commit via set_len. We never
    // read the uninit region before the memcpy completes.
    unsafe {
        let dst = std::slice::from_raw_parts_mut(out.as_mut_ptr().add(start), len);
        arr.copy_to(dst);
        out.set_len(start + len);
    }
    Ok(())
}

#[inline]
fn copy_hmac_result(value: JsValue) -> [u8; HMAC_SHA256_BYTES] {
    let arr: Uint8Array = value
        .dyn_into()
        .expect("HMAC-SHA256 callback must return Uint8Array");
    assert_eq!(
        arr.length() as usize,
        HMAC_SHA256_BYTES,
        "HMAC-SHA256 callback returned the wrong length"
    );
    let mut out = [0u8; HMAC_SHA256_BYTES];
    arr.copy_to(&mut out);
    out
}

/// Invoke a 3-arg crypto callback with zero-copy `Uint8Array::view`s.
///
/// # Safety
/// The views must not outlive the call — if JS retains them and WASM memory
/// grows, the views become detached. Since we only use them for the duration
/// of `call3` and immediately copy the result, this is safe.
#[inline]
unsafe fn call3_bytes(
    f: &js_sys::Function,
    a: &[u8],
    b: &[u8],
    c: &[u8],
) -> Result<JsValue, CryptoProviderError> {
    let a_view = unsafe { Uint8Array::view(a) };
    let b_view = unsafe { Uint8Array::view(b) };
    let c_view = unsafe { Uint8Array::view(c) };
    f.call3(&JsValue::NULL, &a_view, &b_view, &c_view)
        .map_err(|_| CryptoProviderError::BackendFailed)
}

/// Invoke a 4-arg crypto callback with zero-copy `Uint8Array::view`s.
#[inline]
unsafe fn call4_bytes(
    f: &js_sys::Function,
    a: &[u8],
    b: &[u8],
    c: &[u8],
    d: &[u8],
) -> Result<JsValue, CryptoProviderError> {
    let a_view = unsafe { Uint8Array::view(a) };
    let b_view = unsafe { Uint8Array::view(b) };
    let c_view = unsafe { Uint8Array::view(c) };
    let d_view = unsafe { Uint8Array::view(d) };
    let args = js_sys::Array::of4(&a_view, &b_view, &c_view, &d_view);
    f.apply(&JsValue::NULL, &args)
        .map_err(|_| CryptoProviderError::BackendFailed)
}

impl SignalCryptoProvider for JsCryptoAdapter {
    fn aes_256_cbc_encrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        let ret = unsafe { call3_bytes(&self.aes_cbc_encrypt, key, iv, plaintext)? };
        append_returned_bytes(out, ret)
    }

    fn aes_256_cbc_decrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        ciphertext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        out.clear();
        let ret = unsafe { call3_bytes(&self.aes_cbc_decrypt, key, iv, ciphertext)? };
        append_returned_bytes(out, ret)
    }

    fn aes_256_cbc_decrypt_in_place(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        buffer: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        let ciphertext_len = buffer.len();
        if ciphertext_len == 0 || !ciphertext_len.is_multiple_of(AES_CBC_BLOCK_BYTES) {
            return Err(CryptoProviderError::BadInput);
        }

        let ret = unsafe { call3_bytes(&self.aes_cbc_decrypt, key, iv, buffer)? };
        let plaintext: Uint8Array = ret
            .dyn_into()
            .map_err(|_| CryptoProviderError::BackendFailed)?;
        let plaintext_len = plaintext.length() as usize;
        let valid_padding = ciphertext_len
            .checked_sub(plaintext_len)
            .filter(|padding| (1..=AES_CBC_BLOCK_BYTES).contains(padding))
            .is_some();
        if !valid_padding {
            return Err(CryptoProviderError::BackendFailed);
        }

        plaintext.copy_to(&mut buffer[..plaintext_len]);
        buffer.truncate(plaintext_len);
        Ok(())
    }

    fn aes_256_gcm_encrypt(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        let ret = unsafe { call4_bytes(&self.aes_gcm_encrypt, key, nonce, aad, plaintext)? };
        append_returned_bytes(out, ret)
    }

    fn aes_256_gcm_decrypt(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext_with_tag: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        // Native decryptFinal throws on auth failure; map that to AuthFailed.
        // We can't distinguish BadInput from AuthFailed from a thrown JsValue
        // alone — the JS side is expected to throw a consistent error type.
        let ret =
            unsafe { call4_bytes(&self.aes_gcm_decrypt, key, nonce, aad, ciphertext_with_tag) }
                .map_err(|_| CryptoProviderError::AuthFailed)?;
        append_returned_bytes(out, ret)
    }

    fn hmac_sha256(&self, key: &[u8], input: &[u8]) -> [u8; 32] {
        // Zero alloc on Rust side: stack [u8; 32] + single memcpy from JS Buffer.
        // Input path is zero-copy via views. Trait signature is infallible —
        // the JS callback is expected to never throw for valid byte inputs
        // (node:crypto's HMAC never throws).
        let key_view = unsafe { Uint8Array::view(key) };
        let data_view = unsafe { Uint8Array::view(input) };
        let ret = self
            .hmac_sha256
            .call2(&JsValue::NULL, &key_view, &data_view)
            .expect("hmacSha256 callback threw");
        copy_hmac_result(ret)
    }

    fn hmac_sha256_two_part(&self, key: &[u8], first: &[u8], second: &[u8]) -> [u8; 32] {
        let Some(hmac) = &self.hmac_sha256_two_part else {
            return RustCryptoProvider.hmac_sha256_two_part(key, first, second);
        };
        let ret = unsafe { call3_bytes(hmac, key, first, second) }
            .expect("hmacSha256TwoPart callback threw");
        copy_hmac_result(ret)
    }

    /// Override: write JS result directly into the caller's buffer instead of
    /// through a temporary `Vec` + copy_from_slice (what the default impl does).
    /// Saves one Vec alloc + one memcpy on every Noise frame encrypt.
    fn aes_256_gcm_encrypt_in_place(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut dyn GcmInPlaceBuffer,
    ) -> Result<(), CryptoProviderError> {
        let pt_len = buffer.len();
        let ret =
            unsafe { call4_bytes(&self.aes_gcm_encrypt, key, nonce, aad, buffer.as_slice())? };
        let arr: Uint8Array = ret
            .dyn_into()
            .map_err(|_| CryptoProviderError::BackendFailed)?;
        let new_len = arr.length() as usize;
        if new_len != pt_len + GCM_AUTH_TAG_BYTES {
            return Err(CryptoProviderError::BackendFailed);
        }
        buffer.resize(new_len, 0);
        arr.copy_to(buffer.as_mut_slice());
        Ok(())
    }

    /// Override: same rationale as encrypt_in_place — skip the Vec hop.
    fn aes_256_gcm_decrypt_in_place(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut dyn GcmInPlaceBuffer,
    ) -> Result<(), CryptoProviderError> {
        let ret = unsafe { call4_bytes(&self.aes_gcm_decrypt, key, nonce, aad, buffer.as_slice()) }
            .map_err(|_| CryptoProviderError::AuthFailed)?;
        let arr: Uint8Array = ret
            .dyn_into()
            .map_err(|_| CryptoProviderError::BackendFailed)?;
        let new_len = arr.length() as usize;
        if new_len + GCM_AUTH_TAG_BYTES != buffer.len() {
            return Err(CryptoProviderError::BackendFailed);
        }
        arr.copy_to(&mut buffer.as_mut_slice()[..new_len]);
        buffer.truncate(new_len);
        Ok(())
    }
}

/// Routes only operations large enough to amortize the JS boundary through
/// the host callbacks. In particular, Noise transport encryption always uses
/// Rust's connection-lifetime `TransportAead`, which precomputes the AES key
/// schedule once and avoids allocating a host cipher and several Buffers for
/// every stanza.
struct HybridCryptoAdapter {
    js: JsCryptoAdapter,
    rust: RustCryptoProvider,
}

#[inline]
fn use_native_aes(payload_len: usize) -> bool {
    payload_len >= NATIVE_AES_MIN_BYTES
}

#[inline]
fn gcm_plaintext_len(ciphertext_len: usize) -> usize {
    ciphertext_len.saturating_sub(GCM_AUTH_TAG_BYTES)
}

impl SignalCryptoProvider for HybridCryptoAdapter {
    fn aes_256_cbc_encrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        if use_native_aes(plaintext.len()) {
            self.js.aes_256_cbc_encrypt(key, iv, plaintext, out)
        } else {
            self.rust.aes_256_cbc_encrypt(key, iv, plaintext, out)
        }
    }

    fn aes_256_cbc_decrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        ciphertext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        if use_native_aes(ciphertext.len()) {
            self.js.aes_256_cbc_decrypt(key, iv, ciphertext, out)
        } else {
            self.rust.aes_256_cbc_decrypt(key, iv, ciphertext, out)
        }
    }

    fn aes_256_cbc_decrypt_in_place(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        buffer: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        // Node's crypto API cannot decrypt into caller-owned memory: routing
        // this path through OpenSSL allocates a second full-payload Buffer and
        // leaves its pages resident after a large history sync. The Rust
        // provider preserves the caller's allocation; native acceleration is
        // still used for out-of-place CBC and large multi-part HMAC below.
        self.rust.aes_256_cbc_decrypt_in_place(key, iv, buffer)
    }

    fn aes_256_gcm_encrypt(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        if use_native_aes(plaintext.len()) {
            self.js.aes_256_gcm_encrypt(key, nonce, aad, plaintext, out)
        } else {
            self.rust
                .aes_256_gcm_encrypt(key, nonce, aad, plaintext, out)
        }
    }
    fn aes_256_gcm_decrypt(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext_with_tag: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        let plaintext_len = gcm_plaintext_len(ciphertext_with_tag.len());
        if use_native_aes(plaintext_len) {
            self.js
                .aes_256_gcm_decrypt(key, nonce, aad, ciphertext_with_tag, out)
        } else {
            self.rust
                .aes_256_gcm_decrypt(key, nonce, aad, ciphertext_with_tag, out)
        }
    }

    fn hmac_sha256(&self, key: &[u8], input: &[u8]) -> [u8; 32] {
        self.rust.hmac_sha256(key, input)
    }

    fn hmac_sha256_two_part(&self, key: &[u8], first: &[u8], second: &[u8]) -> [u8; 32] {
        if first.len().saturating_add(second.len()) >= NATIVE_HMAC_MIN_BYTES {
            self.js.hmac_sha256_two_part(key, first, second)
        } else {
            self.rust.hmac_sha256_two_part(key, first, second)
        }
    }

    fn aes_256_gcm_encrypt_in_place(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut dyn GcmInPlaceBuffer,
    ) -> Result<(), CryptoProviderError> {
        if use_native_aes(buffer.len()) {
            self.js
                .aes_256_gcm_encrypt_in_place(key, nonce, aad, buffer)
        } else {
            self.rust
                .aes_256_gcm_encrypt_in_place(key, nonce, aad, buffer)
        }
    }

    fn aes_256_gcm_decrypt_in_place(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut dyn GcmInPlaceBuffer,
    ) -> Result<(), CryptoProviderError> {
        let plaintext_len = gcm_plaintext_len(buffer.len());
        if use_native_aes(plaintext_len) {
            self.js
                .aes_256_gcm_decrypt_in_place(key, nonce, aad, buffer)
        } else {
            self.rust
                .aes_256_gcm_decrypt_in_place(key, nonce, aad, buffer)
        }
    }

    fn transport_aead(
        &'static self,
        key: &[u8; 32],
    ) -> Result<Box<dyn TransportAead>, CryptoProviderError> {
        self.rust.transport_aead(key)
    }
}

/// Install `JsCryptoAdapter` as the global crypto provider, if `cfg` is a JS
/// object implementing the callback interface. No-op if `cfg` is null/undefined.
pub fn try_install_from_js(cfg: &JsValue) -> Result<bool, JsValue> {
    if cfg.is_null() || cfg.is_undefined() {
        return Ok(false);
    }
    let adapter = HybridCryptoAdapter {
        js: JsCryptoAdapter::from_js(cfg)?,
        rust: RustCryptoProvider,
    };
    // If a provider was already installed (e.g. hot-reload), surface the error
    // as a warning and keep the existing provider.
    match set_crypto_provider(adapter) {
        Ok(()) => Ok(true),
        Err(_) => {
            log::warn!("crypto provider already set; ignoring JsCryptoAdapter install");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn native_aes_threshold_has_an_explicit_boundary() {
        assert!(!use_native_aes(NATIVE_AES_MIN_BYTES - 1));
        assert!(use_native_aes(NATIVE_AES_MIN_BYTES));
    }

    #[test]
    fn gcm_routing_uses_plaintext_size_not_tagged_size() {
        assert_eq!(gcm_plaintext_len(GCM_AUTH_TAG_BYTES - 1), 0);
        assert_eq!(gcm_plaintext_len(GCM_AUTH_TAG_BYTES), 0);
        assert_eq!(
            gcm_plaintext_len(NATIVE_AES_MIN_BYTES + GCM_AUTH_TAG_BYTES),
            NATIVE_AES_MIN_BYTES
        );
    }
}
