// Tracing expands the sync worker's nested future beyond rustc's default query depth.
#![recursion_limit = "256"]

#[cfg(feature = "audio")]
pub mod audio;
#[cfg(feature = "image")]
pub mod image_utils;
#[cfg(feature = "sticker")]
pub mod sticker_metadata;

pub mod addon_crypto;
pub mod camel_serializer;
pub mod client_profile;
pub mod compression;
pub mod crypto;
pub mod device_props;
pub mod errors;
mod generated_types;
pub mod js_backend;
mod js_bytes;
pub mod js_cache_store;
pub mod js_crypto;
pub mod js_http;
mod js_keys;
pub mod js_time;
pub mod js_transport;
#[cfg(feature = "legacy-session")]
pub mod legacy_session;
pub mod logger;
pub mod memory_profile;
pub mod proto;
pub mod result_types;
pub mod runtime;
#[cfg(feature = "client-signal")]
pub mod signal_records;
pub mod wasm_client;
mod wasm_utils;
mod wire_batch;

/// WebAssembly 1.0 linear-memory page size. Keep memory limits and diagnostics
/// derived from the same unit instead of repeating the protocol constant.
pub(crate) const WASM_PAGE_BYTES: usize = 65_536;

/// SAFETY: WASM is single-threaded — Send + Sync are trivially satisfied.
/// This macro reduces boilerplate for types that hold JS values.
macro_rules! wasm_send_sync {
    ($($t:ty),+ $(,)?) => {
        $(
            unsafe impl Send for $t {}
            unsafe impl Sync for $t {}
        )+
    };
}
pub(crate) use wasm_send_sync;

use serde::Serialize;
use tsify::Tsify;
use wasm_bindgen::prelude::*;

/// Enabled features in this build.
/// Use this to check feature availability at runtime before calling feature-gated functions.
#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct EnabledFeatures {
    /// Audio processing support (waveform generation, duration detection)
    pub audio: bool,
    /// Image processing support (thumbnails, profile pictures, format conversion)
    pub image: bool,
    /// Sticker metadata support (WebP EXIF for WhatsApp stickers)
    pub sticker: bool,
}

/// Returns which optional features are enabled in this build.
/// Use this to conditionally call feature-gated functions.
#[wasm_bindgen(js_name = getEnabledFeatures)]
pub fn get_enabled_features() -> EnabledFeatures {
    EnabledFeatures {
        audio: cfg!(feature = "audio"),
        image: cfg!(feature = "image"),
        sticker: cfg!(feature = "sticker"),
    }
}

/// Returns current WASM linear memory usage in bytes.
///
/// This is the total memory reserved by the WASM instance (pages × 64KB).
/// Useful for monitoring memory pressure during media operations.
///
/// Note: this includes free space managed by the allocator — it's the
/// total memory footprint, not the amount currently in use.
#[wasm_bindgen(js_name = getWasmMemoryBytes)]
pub fn get_wasm_memory_bytes() -> f64 {
    let pages = core::arch::wasm32::memory_size::<0>();
    (pages * WASM_PAGE_BYTES) as f64
}

/// Snapshot of requested Rust heap bytes plus scoped history-sync allocation
/// churn. Detailed counters are enabled only in `memory-profiling` builds;
/// linear-memory bytes are always available.
#[wasm_bindgen(typescript_custom_section)]
const _TS_WASM_ALLOCATION_SNAPSHOT_API: &str = r#"
export function getWasmAllocationSnapshot(): WasmAllocationSnapshot;
"#;

#[wasm_bindgen(js_name = getWasmAllocationSnapshot, skip_typescript)]
pub fn get_wasm_allocation_snapshot() -> Result<JsValue, JsValue> {
    let _diagnostics = memory_profile::enter_scope(memory_profile::AllocationScope::Diagnostics);
    serde_wasm_bindgen::to_value(&memory_profile::snapshot())
        .map_err(|error| JsValue::from_str(&format!("serialize WASM allocation snapshot: {error}")))
}

/// Start a new peak-observation window without disturbing live/cumulative
/// counters. The benchmark harness calls this immediately before constructing
/// a client, then computes churn from successive snapshots.
#[wasm_bindgen(js_name = beginWasmAllocationProfile)]
pub fn begin_wasm_allocation_profile() {
    memory_profile::begin_profile();
}
