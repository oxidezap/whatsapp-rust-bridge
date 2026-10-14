//! Behavioral fixtures for generated projections, using public core constructors.
use crate::result_types::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::wasm_bindgen_test as test;

fn js<T: serde::Serialize>(value: &T) -> JsValue {
    crate::proto::to_js_value(value).unwrap()
}
fn field(value: &JsValue, name: &str) -> JsValue {
    js_sys::Reflect::get(value, &name.into()).unwrap()
}

#[test]
fn capping_keeps_unknown_distinct_from_zero_and_uses_saturating_remaining_quota() {
    let mut source = whatsapp_rust::features::NewChatMessageCapping::default();
    let value = js(&project_new_chat_message_capping(&source));
    assert!(field(&value, "remainingQuota").is_undefined());
    source.total_quota = Some(3);
    assert!(
        field(
            &js(&project_new_chat_message_capping(&source)),
            "remainingQuota"
        )
        .is_undefined()
    );
    source.used_quota = Some(5);
    let value = js(&project_new_chat_message_capping(&source));
    assert_eq!(field(&value, "remainingQuota").as_f64(), Some(0.0));
    assert_eq!(field(&value, "usedQuota").as_f64(), Some(5.0));
    source.used_quota = Some(1);
    assert_eq!(
        field(
            &js(&project_new_chat_message_capping(&source)),
            "remainingQuota"
        )
        .as_f64(),
        Some(2.0)
    );
}

#[cfg(feature = "client-newsletter")]
#[test]
fn newsletter_projection_preserves_large_counters_binary_and_protobuf_presence() {
    use whatsapp_rust::features::NewsletterMessageType;
    use whatsapp_rust::wacore::types::message::EditAttribute;
    use whatsapp_rust::{NewsletterMessage, NewsletterPollVote};
    let mut source = NewsletterMessage::builder()
        .server_id(u64::MAX)
        .timestamp(17)
        .message_type(NewsletterMessageType::Text)
        .edit(EditAttribute::Empty)
        .is_sender(false)
        .is_wamo_sub(false)
        .votes(vec![
            NewsletterPollVote::builder()
                .option_hash([255; 32])
                .count(u64::MAX)
                .build(),
        ])
        .build();
    let absent = js(&project_newsletter_message(&source));
    assert_eq!(
        field(&absent, "serverId").as_string().as_deref(),
        Some("18446744073709551615")
    );
    for key in ["message", "rcat", "viewsCount", "adminProfile"] {
        assert!(
            field(&absent, key).is_undefined(),
            "{key} must remain absent"
        );
    }
    source.views_count = Some(u64::MAX);
    source.forwards_count = Some(0);
    source.rcat = Some(vec![0, 128, 255]);
    let message = {
        let mut __fixture = whatsapp_rust::waproto::whatsapp::Message::default();
        __fixture.conversation = Some("projection fixture".into());
        __fixture
    };
    let expected = whatsapp_rust::waproto::codec::message_to_vec(&message);
    source.message = Some(message);
    let value = js(&project_newsletter_message(&source));
    assert_eq!(
        field(&value, "viewsCount").as_string().as_deref(),
        Some("18446744073709551615")
    );
    assert_eq!(
        field(&value, "forwardsCount").as_string().as_deref(),
        Some("0")
    );
    for (name, expected) in [("rcat", vec![0, 128, 255]), ("message", expected)] {
        let bytes = field(&value, name)
            .dyn_into::<js_sys::Uint8Array>()
            .expect("binary stays Uint8Array");
        assert_eq!(bytes.to_vec(), expected);
    }
    let votes = js_sys::Array::from(&field(&value, "votes"));
    assert_eq!(votes.length(), 1);
    assert_eq!(
        field(&votes.get(0), "count").as_string().as_deref(),
        Some("18446744073709551615")
    );
    assert_eq!(
        field(&votes.get(0), "optionHash")
            .dyn_into::<js_sys::Uint8Array>()
            .unwrap()
            .to_vec(),
        vec![255; 32]
    );
}

#[cfg(feature = "client-contacts")]
#[test]
fn profile_picture_keeps_blank_values_and_omits_unreported_hash() {
    let mut source = whatsapp_rust::wacore::iq::contacts::ProfilePicture {
        id: "id".into(),
        url: "url".into(),
        direct_path: Some(String::new()),
        hash: None,
    };
    let value = js(&project_profile_picture_info(&source));
    assert_eq!(field(&value, "directPath").as_string().as_deref(), Some(""));
    assert!(field(&value, "hash").is_undefined());
    source.hash = Some("hash".into());
    assert_eq!(
        field(&js(&project_profile_picture_info(&source)), "hash")
            .as_string()
            .as_deref(),
        Some("hash")
    );
}

#[cfg(feature = "client-media")]
#[test]
fn media_connection_preserves_zero_ttl_and_empty_host_list() {
    let source = whatsapp_rust::mediaconn::MediaConn {
        auth: String::new(),
        ttl: 0,
        auth_ttl: Some(99),
        hosts: vec![],
        fetched_at: whatsapp_rust::wacore::time::Instant::now(),
    };
    let value = js(&project_media_conn(&source));
    assert_eq!(field(&value, "auth").as_string().as_deref(), Some(""));
    assert_eq!(field(&value, "ttl").as_f64(), Some(0.0));
    assert_eq!(js_sys::Array::from(&field(&value, "hosts")).length(), 0);
    assert!(field(&value, "authTtl").is_undefined());
}

#[test]
fn core_allocation_projection_distinguishes_disabled_and_saturates_net_bytes() {
    use whatsapp_rust::wacore::stats::{AllocMeter, TaskInstrument};
    let disabled = project_core_allocation_snapshot(None);
    assert!(!disabled.enabled);
    assert_eq!(disabled.allocated_bytes, 0.0);
    assert_eq!(disabled.freed_bytes, 0.0);
    assert_eq!(disabled.allocations, 0.0);
    assert_eq!(disabled.net_bytes, 0.0);

    let meter = AllocMeter::new();
    meter.on_poll_start();
    AllocMeter::on_alloc(17);
    AllocMeter::on_alloc(23);
    AllocMeter::on_dealloc(51);
    meter.on_poll_end();
    let value = project_core_allocation_snapshot(Some(&meter));
    assert!(value.enabled);
    assert_eq!(value.allocated_bytes, 40.0);
    assert_eq!(value.freed_bytes, 51.0);
    assert_eq!(value.allocations, 2.0);
    assert_eq!(value.net_bytes, 0.0);

    meter.on_poll_start();
    AllocMeter::on_alloc(32);
    meter.on_poll_end();
    assert_eq!(
        project_core_allocation_snapshot(Some(&meter)).net_bytes,
        21.0
    );
}
