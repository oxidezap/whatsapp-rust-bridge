//! Contacts, profile, blocking, privacy and presence.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;
use whatsapp_rust::wacore_binary::JidExt;

fn profile_picture_target(jid: &Jid) -> whatsapp_rust::ProfilePictureTarget<'_> {
    if jid.is_group() {
        whatsapp_rust::ProfilePictureTarget::Group(jid)
    } else {
        whatsapp_rust::ProfilePictureTarget::Contact(jid)
    }
}

#[cfg(test)]
mod picture_target_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn picture_requests_use_the_core_jid_class_without_community_fallback() {
        let group = "120363000000000000@g.us".parse().unwrap();
        let contact = "5511999999999@s.whatsapp.net".parse().unwrap();
        assert!(
            matches!(profile_picture_target(&group), whatsapp_rust::ProfilePictureTarget::Group(jid) if jid == &group)
        );
        assert!(
            matches!(profile_picture_target(&contact), whatsapp_rust::ProfilePictureTarget::Contact(jid) if jid == &contact)
        );
    }
}

include!("generated/contacts.rs");
