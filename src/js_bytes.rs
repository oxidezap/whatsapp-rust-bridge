//! Moving bytes from a JS `Uint8Array` into linear memory.
//!
//! `js_sys::Uint8Array::to_vec` is convenient but does three things we do not
//! want on a hot path: it reads `length` over the FFI, allocates a zero-filled
//! `Vec` — memsetting bytes that the copy is about to overwrite — and then
//! copies through `copy_to`, which reads `length` a *second* time to assert the
//! sizes match. Every inbound WebSocket frame and every store read pays that.
//!
//! The helpers here read `length` once, leave the destination uninitialised, and
//! copy straight into it.

use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// `Uint8Array.prototype.set.call(dst, src)` — the primitive `copy_to` is
    /// built on, without its length assertion.
    #[wasm_bindgen(js_namespace = Uint8Array, js_name = "prototype.set.call")]
    fn uint8array_set_into(dst: &mut [u8], src: &Uint8Array);
}

/// Copy `src` into `dst` when the caller has already read `src.length()` and
/// sized `dst` from it.
///
/// Sizing `dst` from anything else is a bug: a `src` longer than `dst` makes the
/// underlying `set` throw, and a shorter one silently leaves the tail of `dst`
/// untouched — which, where `dst` is uninitialised, would expose uninitialised
/// memory. The debug assertion pins the invariant in tests.
#[inline]
pub(crate) fn copy_exact(dst: &mut [u8], src: &Uint8Array) {
    debug_assert_eq!(src.length() as usize, dst.len());
    uint8array_set_into(dst, src);
}

/// Read a `Uint8Array` into a fresh `Vec`, with one `length` crossing and no
/// zero-fill.
#[inline]
pub(crate) fn to_vec(src: &Uint8Array) -> Vec<u8> {
    let len = src.length() as usize;
    let mut out = Vec::with_capacity(len);
    // SAFETY: with_capacity guarantees room for `len` bytes. `copy_exact` writes
    // exactly `len` of them, fully initialising the region before `set_len`
    // publishes it; nothing reads the uninitialised range in between.
    unsafe {
        copy_exact(std::slice::from_raw_parts_mut(out.as_mut_ptr(), len), src);
        out.set_len(len);
    }
    out
}

/// Append a `Uint8Array` to `out`, with one `length` crossing and no zero-fill
/// of the grown region.
#[inline]
pub(crate) fn append(out: &mut Vec<u8>, src: &Uint8Array) {
    let len = src.length() as usize;
    let start = out.len();
    out.reserve(len);
    // SAFETY: as in `to_vec`, over the reserved tail.
    unsafe {
        copy_exact(
            std::slice::from_raw_parts_mut(out.as_mut_ptr().add(start), len),
            src,
        );
        out.set_len(start + len);
    }
}
