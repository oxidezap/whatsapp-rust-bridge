//! Property keys interned once per thread.
//!
//! Handing a `&str` to `js_sys::Reflect::set` goes through a `&str -> JsValue`
//! cast, and that cast is not free: it copies the bytes out of linear memory
//! through a `TextDecoder` and allocates a JS string. Property keys are written
//! on every object the bridge builds, so paying it per object turns a handful of
//! fixed names into tens of thousands of short-lived JS strings per second.
//!
//! V8 sizes its young generation from the allocation rate, so that churn is not
//! merely wasted CPU: it shows up as committed RSS. Interning replaces it with
//! one allocation per key for the life of the thread.

use wasm_bindgen::JsValue;

/// Declare interned JS property keys, each as a thread-local `JsValue`.
macro_rules! js_keys {
    ($( $(#[$meta:meta])* $name:ident => $literal:literal ),* $(,)?) => {
        thread_local! {
            $( $(#[$meta])* pub(crate) static $name: JsValue = JsValue::from_str($literal); )*
        }
    };
}

js_keys! {
    /// Envelope of a dispatched event.
    EVENT_TYPE_KEY => "type",
    EVENT_DATA_KEY => "data",

    /// `MessageWireBatch` fields, mirrored by `ts/wire-info.ts`.
    MESSAGE_WIRE_DATA_KEY => "messageData",
    MESSAGE_WIRE_OFFSETS_KEY => "messageOffsets",
    MESSAGE_WIRE_INFO_STRING_DATA_KEY => "infoStringData",
    MESSAGE_WIRE_INFO_STRING_OFFSETS_KEY => "infoStringOffsets",
    MESSAGE_WIRE_INFO_RECORDS_KEY => "infoRecords",

    /// Single-buffer batches (`FlatBatchWriter`).
    PACKED_BUFFER_KEY => "buffer",
}

/// Set an interned key on `target`.
#[inline]
pub(crate) fn set(
    target: &js_sys::Object,
    key: &'static std::thread::LocalKey<JsValue>,
    value: &JsValue,
) -> Result<(), JsValue> {
    key.with(|key| js_sys::Reflect::set(target, key, value))?;
    Ok(())
}
