//! Opt-in resident MLOW binding; codec and bindings are shared with the isolated engine.
use whatsapp_rust::wacore::voip::MlowDecoder;
#[cfg(all(test, target_arch = "wasm32"))]
use whatsapp_rust::wacore::voip::MlowEncoder;
pub use whatsapp_rust::wacore::voip::rtp::RTP_PAYLOAD_TYPE_MLOW_RED;

fn invalid_payload_type(detail: &str) -> wasm_bindgen::JsValue {
    crate::errors::to_js_error(&crate::errors::invalid_arg("payloadType", detail))
}

include!("../shared/mlow_decoder.rs");
