//! Generic zlib decompression, exposed without protocol-specific naming or
//! state so any consumer can inflate a blob it already holds.

use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore_binary::zlib_pool::decompress_zlib_pooled;

use crate::wasm_utils::{byte_array, error_value};

/// Matches the ceiling the core applies to its own inflate paths, so a caller
/// that omits a limit is bounded rather than unprotected.
const DEFAULT_MAX_OUTPUT_BYTES: f64 = 64.0 * 1024.0 * 1024.0;

/// Inflate a zlib stream.
///
/// The decompressor and its scratch buffer are pooled per thread, so repeated
/// calls do not reallocate. `maxOutputBytes` caps the decompressed size and is
/// rejected before the output can grow past it, which is what keeps a
/// compression bomb from exhausting linear memory; it defaults to 64 MiB.
#[wasm_bindgen(js_name = inflateZlib)]
pub fn inflate_zlib(data: &[u8], max_output_bytes: Option<f64>) -> Result<Uint8Array, JsValue> {
    let limit = max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
    if !(limit.is_finite() && limit >= 1.0) {
        return Err(error_value(
            "maxOutputBytes must be a positive finite number",
        ));
    }
    let output = decompress_zlib_pooled(data, limit as u64).map_err(error_value)?;
    Ok(byte_array(&output))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Alias `#[test]` -> wasm_bindgen_test so these run on wasm32 via
    // `wasm-pack test --node` (the crate has no native test target).
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn deflate(payload: &[u8]) -> Vec<u8> {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn round_trips_a_compressed_payload() {
        let payload = b"history sync blob".repeat(100);
        let inflated = inflate_zlib(&deflate(&payload), None).unwrap();
        assert_eq!(inflated.to_vec(), payload);
    }

    #[test]
    fn round_trips_an_empty_payload() {
        let inflated = inflate_zlib(&deflate(b""), None).unwrap();
        assert_eq!(inflated.length(), 0);
    }

    #[test]
    fn rejects_output_larger_than_the_limit() {
        let compressed = deflate(&vec![0u8; 4096]);
        assert!(inflate_zlib(&compressed, Some(64.0)).is_err());
        assert!(inflate_zlib(&compressed, Some(4096.0)).is_ok());
    }

    #[test]
    fn rejects_data_that_is_not_a_zlib_stream() {
        assert!(inflate_zlib(b"not zlib at all", None).is_err());
    }

    #[test]
    fn rejects_a_non_positive_limit() {
        let compressed = deflate(b"payload");
        assert!(inflate_zlib(&compressed, Some(0.0)).is_err());
        assert!(inflate_zlib(&compressed, Some(f64::NAN)).is_err());
    }
}
