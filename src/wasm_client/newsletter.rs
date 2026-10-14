//! Newsletter (channel) operations.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[cfg(test)]
mod reference_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn newsletter_reference_keeps_target_before_id_error_order() {
        for (jid, field) in [
            ("120363000000000000@newsletter", "messageId"),
            ("5511999999999@s.whatsapp.net", "jid"),
        ] {
            let jid = jid.parse().unwrap();
            let error = newsletter_content_reference(&jid, "").unwrap_err();
            assert!(
                matches!(error, crate::errors::BridgeError::InvalidArgument { field: actual, .. } if actual == field)
            );
        }
    }

    #[test]
    fn newsletter_content_id_keeps_its_wire_spelling_and_is_not_a_server_id() {
        let jid = "120363000000000000@newsletter".parse().unwrap();
        let reference = newsletter_content_reference(&jid, " POST-ID ").unwrap();
        assert_eq!(
            reference.require_message_id().unwrap().as_str(),
            " POST-ID "
        );
        assert_eq!(reference.chat(), &jid);
        assert_eq!(reference.server_id(), None);
    }
}

include!("generated/newsletter.rs");
