use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;

pub(crate) fn byte_array(bytes: &[u8]) -> Uint8Array {
    let output = Uint8Array::new_with_length(bytes.len() as u32);
    output.copy_from(bytes);
    output
}

pub(crate) fn error_value(error: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}
