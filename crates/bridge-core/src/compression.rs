//! Generic zlib decompression, exposed without protocol-specific naming or
//! state so any consumer can inflate a blob it already holds.

use whatsapp_rust::wacore_binary::zlib_pool::decompress_zlib_pooled;

use crate::{CoreError, CoreResult};

/// Matches the ceiling the core applies to its own inflate paths, so a caller
/// that omits a limit is bounded rather than unprotected.
pub const DEFAULT_MAX_OUTPUT_BYTES: f64 = 64.0 * 1024.0 * 1024.0;

/// Inflate a zlib stream.
///
/// `max_output_bytes` caps the decompressed size and is rejected before the
/// output can grow past it, which is what keeps a compression bomb from
/// exhausting linear memory; it defaults to 64 MiB.
pub fn inflate_zlib(data: &[u8], max_output_bytes: Option<f64>) -> CoreResult<Vec<u8>> {
    let limit = max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
    if !(limit.is_finite() && limit >= 1.0) {
        return Err(CoreError::new(
            "maxOutputBytes must be a positive finite number",
        ));
    }
    decompress_zlib_pooled(data, limit as u64).map_err(CoreError::from_display)
}
