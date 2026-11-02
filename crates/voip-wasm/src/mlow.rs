//! Isolated MLOW binding; preserve the engine's existing error representation.
use wacore::voip::MlowDecoder;
#[cfg(all(test, target_arch = "wasm32"))]
use wacore::voip::MlowEncoder;
pub use wacore::voip::rtp::RTP_PAYLOAD_TYPE_MLOW_RED;

fn invalid_payload_type(detail: &str) -> wasm_bindgen::JsValue {
    wasm_bindgen::JsValue::from_str(&format!("payloadType: {detail}"))
}

include!("../../../shared/mlow_decoder.rs");
