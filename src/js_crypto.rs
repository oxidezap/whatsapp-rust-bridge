//! Host crypto callbacks retained by each client runtime.
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

use crate::js_bytes;
use js_sys::Uint8Array;
use std::{cell::RefCell, sync::Arc};
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::libsignal::crypto::{
    CryptoProviderError, GcmInPlaceBuffer, RustCryptoProvider, SymmetricCryptoProvider,
};

/// Payload size at which the host's native cipher starts winning, per operation.
///
/// A `node:crypto` call has a floor of roughly 9 microseconds regardless of size:
/// constructing the cipher object and its Buffers, not encrypting. Below the
/// crossover the WASM software implementation is several times faster, so routing
/// there early is a pessimisation, and the crossover is NOT the same for every
/// primitive — CBC encrypt is serial per block while its decrypt, GCM and HMAC
/// parallelise, so they stay ahead of the native call for much longer.
///
/// Measured per operation (see the `crypto-hw-bench` project: same provider, wasm
/// built with the bridge's own flags, one cipher per call as a real caller does):
///
/// | operation         | crossover  |
/// |-------------------|------------|
/// | CBC encrypt       | 512 B–1 KB |
/// | CBC decrypt       | 1–4 KB     |
/// | GCM encrypt/decr. | 1–4 KB     |
/// | HMAC-SHA256       | 4–16 KB    |
///
/// Values sit at the low end of each range: past the crossover the native side
/// wins by a wide margin, so erring early costs little while erring late costs a
/// lot on media payloads.
const NATIVE_CBC_ENCRYPT_MIN_BYTES: usize = 512;
const NATIVE_CBC_DECRYPT_MIN_BYTES: usize = 2048;
const NATIVE_GCM_MIN_BYTES: usize = 2048;
const NATIVE_HMAC_MIN_BYTES: usize = 8192;

// The ordering between the thresholds is the measurement's conclusion: CBC
// encrypt is serial per block and crosses over first, while the primitives that
// parallelise stay ahead of the host call for longer. Asserting it in a const
// block fails the build rather than a test, so a future edit cannot reorder them
// on a target the test suite does not run.
const _: () = assert!(NATIVE_CBC_DECRYPT_MIN_BYTES > NATIVE_CBC_ENCRYPT_MIN_BYTES);
const _: () = assert!(NATIVE_HMAC_MIN_BYTES > NATIVE_GCM_MIN_BYTES);
const AES_CBC_BLOCK_BYTES: usize = 16;
const GCM_AUTH_TAG_BYTES: usize = 16;
const HMAC_SHA256_BYTES: usize = 32;

generated_crypto_callbacks!();

crate::wasm_send_sync!(JsCryptoAdapter);

/// Append the bytes of a JS `Uint8Array` to `out` without pre-zeroing the tail.
///
/// `Vec::resize(new_len, 0)` would memset-zero the grown region before the copy
/// overwrites it — pure waste. We use `reserve` + `spare_capacity_mut` +
/// `set_len` to let the copy write straight into the uninit tail.
#[inline]
fn append_exact_returned_bytes(
    out: &mut Vec<u8>,
    value: JsValue,
    expected_len: usize,
) -> Result<(), CryptoProviderError> {
    let arr: Uint8Array = value
        .dyn_into()
        .map_err(|_| CryptoProviderError::BackendFailed)?;
    if arr.length() as usize != expected_len {
        return Err(CryptoProviderError::BackendFailed);
    }
    js_bytes::append(out, &arr);
    Ok(())
}

#[inline]
fn copy_hmac_result(value: JsValue) -> Result<[u8; HMAC_SHA256_BYTES], CryptoProviderError> {
    let arr: Uint8Array = value
        .dyn_into()
        .map_err(|_| CryptoProviderError::BackendFailed)?;
    if arr.length() as usize != HMAC_SHA256_BYTES {
        return Err(CryptoProviderError::BackendFailed);
    }
    let mut out = [0u8; HMAC_SHA256_BYTES];
    arr.copy_to(&mut out);
    Ok(out)
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

impl SymmetricCryptoProvider for JsCryptoAdapter {
    fn aes_256_cbc_encrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        let ret = unsafe { call3_bytes(&self.aes_cbc_encrypt, key, iv, plaintext)? };
        let padded = plaintext
            .len()
            .checked_add(AES_CBC_BLOCK_BYTES - plaintext.len() % AES_CBC_BLOCK_BYTES)
            .ok_or(CryptoProviderError::BadInput)?;
        append_exact_returned_bytes(out, ret, padded)
    }

    fn aes_256_cbc_decrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        ciphertext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(AES_CBC_BLOCK_BYTES) {
            return Err(CryptoProviderError::BadInput);
        }
        let ret = unsafe { call3_bytes(&self.aes_cbc_decrypt, key, iv, ciphertext)? };
        let plaintext: Uint8Array = ret
            .dyn_into()
            .map_err(|_| CryptoProviderError::BackendFailed)?;
        if !ciphertext
            .len()
            .checked_sub(plaintext.length() as usize)
            .is_some_and(|padding| (1..=AES_CBC_BLOCK_BYTES).contains(&padding))
        {
            return Err(CryptoProviderError::BackendFailed);
        }
        js_bytes::append(out, &plaintext);
        Ok(())
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

        js_bytes::copy_exact(&mut buffer[..plaintext_len], &plaintext);
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
        let expected_len = plaintext
            .len()
            .checked_add(GCM_AUTH_TAG_BYTES)
            .ok_or(CryptoProviderError::BackendFailed)?;
        append_exact_returned_bytes(out, ret, expected_len)
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
        let expected_len = ciphertext_with_tag
            .len()
            .checked_sub(GCM_AUTH_TAG_BYTES)
            .ok_or(CryptoProviderError::BadInput)?;
        append_exact_returned_bytes(out, ret, expected_len)
    }

    fn hmac_sha256(&self, key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], CryptoProviderError> {
        if parts.len() >= 2
            && let Some(callback) = &self.hmac_sha256_two_part
        {
            // Signal authenticates two identity keys followed by the message.
            // Join only the small prefix so the payload stays borrowed.
            let (last, prefix) = parts.split_last().expect("at least two parts");
            let joined_prefix;
            let first = if let [first] = prefix {
                *first
            } else {
                joined_prefix = prefix.concat();
                &joined_prefix
            };
            return copy_hmac_result(unsafe { call3_bytes(callback, key, first, last)? });
        }
        let joined;
        let input = if let [part] = parts {
            *part
        } else {
            joined = parts.concat();
            &joined
        };
        let key_view = unsafe { Uint8Array::view(key) };
        let data_view = unsafe { Uint8Array::view(input) };
        let result = self
            .hmac_sha256
            .call2(&JsValue::NULL, &key_view, &data_view)
            .map_err(|_| CryptoProviderError::BackendFailed)?;
        copy_hmac_result(result)
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
        js_bytes::copy_exact(buffer.as_mut_slice(), &arr);
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
        js_bytes::copy_exact(&mut buffer.as_mut_slice()[..new_len], &arr);
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
fn use_native(payload_len: usize, min_bytes: usize) -> bool {
    payload_len >= min_bytes
}

#[inline]
fn gcm_plaintext_len(ciphertext_len: usize) -> usize {
    ciphertext_len.saturating_sub(GCM_AUTH_TAG_BYTES)
}

impl SymmetricCryptoProvider for HybridCryptoAdapter {
    fn aes_256_cbc_encrypt(
        &self,
        key: &[u8; 32],
        iv: &[u8; 16],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), CryptoProviderError> {
        if use_native(plaintext.len(), NATIVE_CBC_ENCRYPT_MIN_BYTES) {
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
        if use_native(ciphertext.len(), NATIVE_CBC_DECRYPT_MIN_BYTES) {
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
        if use_native(plaintext.len(), NATIVE_GCM_MIN_BYTES) {
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
        if use_native(plaintext_len, NATIVE_GCM_MIN_BYTES) {
            self.js
                .aes_256_gcm_decrypt(key, nonce, aad, ciphertext_with_tag, out)
        } else {
            self.rust
                .aes_256_gcm_decrypt(key, nonce, aad, ciphertext_with_tag, out)
        }
    }

    fn hmac_sha256(&self, key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], CryptoProviderError> {
        let len = parts
            .iter()
            .fold(0usize, |len, part| len.saturating_add(part.len()));
        if use_native(len, NATIVE_HMAC_MIN_BYTES) {
            self.js.hmac_sha256(key, parts)
        } else {
            self.rust.hmac_sha256(key, parts)
        }
    }

    fn aes_256_gcm_encrypt_in_place(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut dyn GcmInPlaceBuffer,
    ) -> Result<(), CryptoProviderError> {
        if use_native(buffer.len(), NATIVE_GCM_MIN_BYTES) {
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
        if use_native(plaintext_len, NATIVE_GCM_MIN_BYTES) {
            self.js
                .aes_256_gcm_decrypt_in_place(key, nonce, aad, buffer)
        } else {
            self.rust
                .aes_256_gcm_decrypt_in_place(key, nonce, aad, buffer)
        }
    }
}

thread_local! {
    static DEFAULT_PROVIDER: RefCell<Option<Arc<dyn SymmetricCryptoProvider>>> = RefCell::new(None);
}

pub(crate) fn default_provider() -> Arc<dyn SymmetricCryptoProvider> {
    DEFAULT_PROVIDER
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| Arc::new(RustCryptoProvider))
}

/// Existing clients retain their runtime's backend when initialization is repeated.
pub fn try_install_from_js(cfg: &JsValue) -> Result<bool, JsValue> {
    if cfg.is_null() || cfg.is_undefined() {
        return Ok(false);
    }
    let adapter = HybridCryptoAdapter {
        js: JsCryptoAdapter::from_js(cfg)?,
        rust: RustCryptoProvider,
    };
    DEFAULT_PROVIDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            log::warn!("crypto provider already set; ignoring JsCryptoAdapter install");
            return Ok(false);
        }
        *slot = Some(Arc::new(adapter));
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// The threshold ORDERING is a const assertion next to the constants; this
    /// covers the runtime half, that the boundary itself is inclusive.
    #[test]
    fn native_aes_threshold_has_an_explicit_boundary() {
        assert!(!use_native(
            NATIVE_CBC_ENCRYPT_MIN_BYTES - 1,
            NATIVE_CBC_ENCRYPT_MIN_BYTES
        ));
        assert!(use_native(
            NATIVE_CBC_ENCRYPT_MIN_BYTES,
            NATIVE_CBC_ENCRYPT_MIN_BYTES
        ));
    }

    #[test]
    fn gcm_routing_uses_plaintext_size_not_tagged_size() {
        assert_eq!(gcm_plaintext_len(GCM_AUTH_TAG_BYTES - 1), 0);
        assert_eq!(gcm_plaintext_len(GCM_AUTH_TAG_BYTES), 0);
        assert_eq!(
            gcm_plaintext_len(NATIVE_GCM_MIN_BYTES + GCM_AUTH_TAG_BYTES),
            NATIVE_GCM_MIN_BYTES
        );
    }
}

#[cfg(test)]
mod generated_host_callback_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    #[test]
    fn registration_keeps_getter_order_and_required_throw_identity() {
        let value=js_sys::eval(r#"(() => { const reads=[], obj={reads};
            for(const name of ["aesCbc256Encrypt", "aesCbc256Decrypt", "aesGcm256Encrypt", "aesGcm256Decrypt", "hmacSha256", "hmacSha256TwoPart"]) Object.defineProperty(obj,name,{get(){reads.push(name);return ()=>{};}});
            return obj; })()"#).unwrap();
        JsCryptoAdapter::from_js(&value).unwrap();
        let reads: Vec<String> =
            serde_wasm_bindgen::from_value(js_sys::Reflect::get(&value, &"reads".into()).unwrap())
                .unwrap();
        assert_eq!(
            reads,
            vec![
                "aesCbc256Encrypt",
                "aesCbc256Decrypt",
                "aesGcm256Encrypt",
                "aesGcm256Decrypt",
                "hmacSha256",
                "hmacSha256TwoPart"
            ]
        );
        let value=js_sys::eval(r#"(() => {const marker={marker:true}; return {marker,get aesCbc256Encrypt(){throw marker;}};})()"#).unwrap();
        let marker = js_sys::Reflect::get(&value, &"marker".into()).unwrap();
        let error = JsCryptoAdapter::from_js(&value).err().unwrap();
        assert_eq!(error, marker);
        let value = js_sys::eval(r#"({aesCbc256Encrypt:42})"#).unwrap();
        let error = JsCryptoAdapter::from_js(&value).err().unwrap();
        assert_eq!(
            error.as_string().unwrap(),
            "crypto.aesCbc256Encrypt must be a function"
        );
    }
    #[test]
    fn optional_crypto_callbacks_reject_invalid_values_and_propagate_getter_throws() {
        let value=js_sys::eval("({aesCbc256Encrypt(){},aesCbc256Decrypt(){},aesGcm256Encrypt(){},aesGcm256Decrypt(){},hmacSha256(){},hmacSha256TwoPart:42})").unwrap();
        assert_eq!(
            JsCryptoAdapter::from_js(&value)
                .err()
                .unwrap()
                .as_string()
                .unwrap(),
            "crypto.hmacSha256TwoPart must be a function"
        );
        js_sys::Reflect::delete_property(
            &value.clone().unchecked_into::<js_sys::Object>(),
            &"hmacSha256TwoPart".into(),
        )
        .unwrap();
        assert!(
            JsCryptoAdapter::from_js(&value)
                .unwrap()
                .hmac_sha256_two_part
                .is_none()
        );
        let value=js_sys::eval("({aesCbc256Encrypt(){},aesCbc256Decrypt(){},aesGcm256Encrypt(){},aesGcm256Decrypt(){},hmacSha256(){},get hmacSha256TwoPart(){throw 17;}})").unwrap();
        assert_eq!(
            JsCryptoAdapter::from_js(&value).err().unwrap().as_f64(),
            Some(17.0)
        );
    }
}

#[cfg(test)]
mod provider_contract_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn adapter(body: &str) -> JsCryptoAdapter {
        let value = js_sys::eval(&format!("({{aesCbc256Encrypt(){{}},aesCbc256Decrypt(){{}},aesGcm256Encrypt(){{}},aesGcm256Decrypt(){{}},hmacSha256(){{{body}}}}})")).unwrap();
        JsCryptoAdapter::from_js(&value).unwrap()
    }

    #[test]
    fn hmac_failures_are_fallible_without_fallback_or_panic() {
        for body in [
            "throw new Error('host failure');",
            "return 42;",
            "return new Uint8Array(31);",
            "return new Uint8Array(33);",
        ] {
            assert!(matches!(
                adapter(body).hmac_sha256(b"key", &[b"part"]),
                Err(CryptoProviderError::BackendFailed)
            ));
        }
    }

    #[test]
    fn hmac_preserves_part_order_and_validates_the_optional_callback() {
        let value = js_sys::eval("({aesCbc256Encrypt(){},aesCbc256Decrypt(){},aesGcm256Encrypt(){},aesGcm256Decrypt(){},hmacSha256(){throw Error('unexpected fallback');},hmacSha256TwoPart(k,a,b){if(a.join(',')!=='1,2'||b.join(',')!=='3')throw Error('order');return new Uint8Array(32).fill(7);}})").unwrap();
        let provider = JsCryptoAdapter::from_js(&value).unwrap();
        assert_eq!(
            provider.hmac_sha256(b"key", &[&[1], &[2], &[3]]).unwrap(),
            [7; 32]
        );
        assert!(matches!(
            provider.hmac_sha256(b"key", &[&[1], &[2]]),
            Err(CryptoProviderError::BackendFailed)
        ));
    }
}
