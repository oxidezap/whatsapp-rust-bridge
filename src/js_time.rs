//! WASM time providers backed by JS globals.

use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore;
use whatsapp_rust::wacore::time::{MonotonicProvider, TimeProvider};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;

    /// Only used to decide, once, whether the clock is callable at all. The
    /// `catch` form wraps every call in a try/catch and returns a JS result
    /// object that has to be unwrapped and dropped — measurable at four calls
    /// per message, and pointless once availability is known.
    #[wasm_bindgen(js_namespace = performance, js_name = now, catch)]
    fn performance_now_checked() -> Result<f64, JsValue>;
}

pub struct JsTimeProvider;

crate::wasm_send_sync!(JsTimeProvider);

impl TimeProvider for JsTimeProvider {
    fn now_millis(&self) -> i64 {
        js_sys::Date::now() as i64
    }
}

/// Sub-millisecond monotonic clock backed by `performance.now()`.
/// Available in browsers, Node 16+, and Bun. The spec guarantees the value
/// is monotonic non-decreasing within an agent.
pub struct JsMonotonicProvider;

crate::wasm_send_sync!(JsMonotonicProvider);

thread_local! {
    /// Probed once: hosts either have a usable `performance.now` or they never
    /// will, so the per-call guard buys nothing after the first answer.
    static MONOTONIC_USABLE: bool =
        matches!(performance_now_checked(), Ok(ms) if ms.is_finite() && ms >= 0.0);
}

impl MonotonicProvider for JsMonotonicProvider {
    fn now_nanos(&self) -> u64 {
        if !MONOTONIC_USABLE.with(|usable| *usable) {
            return 0;
        }
        let ms = performance_now();
        if ms.is_finite() && ms >= 0.0 {
            (ms * 1_000_000.0) as u64
        } else {
            0
        }
    }
}

/// Initialize the WASM time providers. Call once at startup.
pub fn init_time_provider() {
    let _ = wacore::time::set_time_provider(JsTimeProvider);
    let _ = wacore::time::set_monotonic_provider(JsMonotonicProvider);
}
