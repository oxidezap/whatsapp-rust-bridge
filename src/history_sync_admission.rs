use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use whatsapp_rust::{HistorySyncAdmission, HistorySyncDecision, HistorySyncMetadata};

use crate::errors::BridgeError;

pub(crate) struct JsHistorySyncAdmission {
    callback: Function,
    receiver: JsValue,
}

crate::wasm_send_sync!(JsHistorySyncAdmission);

impl JsHistorySyncAdmission {
    pub(crate) fn from_policies(value: Option<&JsValue>) -> Result<Option<Self>, BridgeError> {
        let Some(value) = value.filter(|value| !value.is_null() && !value.is_undefined()) else {
            return Ok(None);
        };
        if !value.is_object() || Array::is_array(value) {
            return Err(crate::errors::invalid_arg("policies", "must be an object"));
        }
        let callback = Reflect::get(value, &"historySyncAdmission".into()).map_err(|_| {
            crate::errors::invalid_arg("policies", "could not read policies.historySyncAdmission")
        })?;
        if callback.is_undefined() || callback.is_null() {
            return Ok(None);
        }
        let callback = callback.dyn_into::<Function>().map_err(|_| {
            crate::errors::invalid_arg(
                "policies",
                "policies.historySyncAdmission must be a function",
            )
        })?;
        Ok(Some(Self {
            callback,
            receiver: value.clone(),
        }))
    }

    fn call(&self, value: &JsValue) -> HistorySyncDecision {
        match self.callback.call1(&self.receiver, value) {
            Ok(result) if result.as_bool() == Some(true) => HistorySyncDecision::Accept,
            Ok(result) if result.as_bool() == Some(false) => {
                HistorySyncDecision::RejectAndAcknowledge
            }
            Ok(result) => {
                if crate::wasm_client::is_thenable(&result) {
                    observe_thenable_rejection(&result);
                }
                log::error!(
                    "historySyncAdmission must return a boolean synchronously; rejecting and acknowledging the chunk"
                );
                HistorySyncDecision::RejectAndAcknowledge
            }
            Err(error) => {
                log::error!(
                    "historySyncAdmission callback failed: {}",
                    js_error_message(&error)
                );
                HistorySyncDecision::RejectAndAcknowledge
            }
        }
    }
}

fn observe_thenable_rejection(value: &JsValue) {
    let Ok(then) = Reflect::get(value, &"then".into()) else {
        return;
    };
    let Ok(then) = then.dyn_into::<Function>() else {
        return;
    };
    let noop = Function::new_no_args("");
    let _ = then.call2(value, &noop, &noop);
}

impl HistorySyncAdmission for JsHistorySyncAdmission {
    fn decide(&self, metadata: &HistorySyncMetadata<'_>) -> HistorySyncDecision {
        self.call(&metadata_value(
            metadata.sync_type,
            metadata.chunk_order,
            metadata.progress,
            metadata.file_length,
            metadata.inline_payload_len,
            metadata.peer_data_request_session_id,
        ))
    }
}

fn metadata_value(
    sync_type: Option<i32>,
    chunk_order: Option<u32>,
    progress: Option<u32>,
    file_length: Option<u64>,
    inline_payload_len: Option<usize>,
    peer_data_request_session_id: Option<&str>,
) -> Object {
    let value = Object::new();
    set_optional_number(&value, "syncType", sync_type.map(|value| value as f64));
    set_optional_number(&value, "chunkOrder", chunk_order.map(|value| value as f64));
    set_optional_number(&value, "progress", progress.map(|value| value as f64));
    set_file_length(&value, file_length);
    set_optional_number(
        &value,
        "inlinePayloadLen",
        inline_payload_len.map(|value| value as f64),
    );
    if let Some(session_id) = peer_data_request_session_id {
        let _ = Reflect::set(
            &value,
            &"peerDataRequestSessionId".into(),
            &JsValue::from_str(session_id),
        );
    }
    value
}

fn set_optional_number(object: &Object, name: &str, value: Option<f64>) {
    if let Some(value) = value {
        let _ = Reflect::set(object, &name.into(), &JsValue::from_f64(value));
    }
}

fn set_file_length(object: &Object, value: Option<u64>) {
    if let Some(value) = value {
        let _ = Reflect::set(
            object,
            &"fileLength".into(),
            &JsValue::from_str(&value.to_string()),
        );
    }
}

fn js_error_message(error: &JsValue) -> String {
    if let Some(message) = error.as_string() {
        return message;
    }
    if let Some(message) = Reflect::get(error, &"message".into())
        .ok()
        .and_then(|message| message.as_string())
    {
        return message;
    }
    "JavaScript callback threw a non-string value".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn stateful_policy_keeps_its_receiver() {
        let policies = Function::new_no_args(
            "return new class { enabled = true; historySyncAdmission() { return this.enabled; } }",
        )
        .call0(&JsValue::UNDEFINED)
        .unwrap();
        let admission = JsHistorySyncAdmission::from_policies(Some(&policies))
            .unwrap()
            .unwrap();
        assert_eq!(admission.call(&Object::new()), HistorySyncDecision::Accept);
        Reflect::set(&policies, &"enabled".into(), &JsValue::FALSE).unwrap();
        assert_eq!(
            admission.call(&Object::new()),
            HistorySyncDecision::RejectAndAcknowledge
        );
    }

    #[test]
    fn callback_errors_and_non_booleans_fail_closed() {
        for body in [
            "throw new Error('failure')",
            "return false",
            "return undefined",
            "return 1",
            "return 'true'",
            "return Promise.resolve(true)",
            "return Promise.reject(new Error('failure'))",
        ] {
            let policies = Object::new();
            Reflect::set(
                &policies,
                &"historySyncAdmission".into(),
                &Function::new_no_args(body),
            )
            .unwrap();
            let admission = JsHistorySyncAdmission::from_policies(Some(&policies))
                .unwrap()
                .unwrap();
            assert_eq!(
                admission.call(&Object::new()),
                HistorySyncDecision::RejectAndAcknowledge
            );
        }
    }

    #[test]
    fn file_length_keeps_all_uint64_digits() {
        let object = Object::new();
        set_file_length(&object, Some(u64::MAX));
        assert_eq!(
            Reflect::get(&object, &"fileLength".into())
                .unwrap()
                .as_string(),
            Some(u64::MAX.to_string())
        );
    }

    #[test]
    fn metadata_mapping_preserves_fields_and_omits_absent_values() {
        let value = metadata_value(
            Some(-1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
            Some("session"),
        );
        assert_eq!(
            Reflect::get(&value, &"syncType".into()).unwrap().as_f64(),
            Some(-1.0)
        );
        assert_eq!(
            Reflect::get(&value, &"chunkOrder".into()).unwrap().as_f64(),
            Some(2.0)
        );
        assert_eq!(
            Reflect::get(&value, &"progress".into()).unwrap().as_f64(),
            Some(3.0)
        );
        assert_eq!(
            Reflect::get(&value, &"fileLength".into())
                .unwrap()
                .as_string(),
            Some("4".into())
        );
        assert_eq!(
            Reflect::get(&value, &"inlinePayloadLen".into())
                .unwrap()
                .as_f64(),
            Some(5.0)
        );
        assert_eq!(
            Reflect::get(&value, &"peerDataRequestSessionId".into())
                .unwrap()
                .as_string(),
            Some("session".into())
        );

        let absent = metadata_value(None, None, None, None, None, None);
        for field in [
            "syncType",
            "chunkOrder",
            "progress",
            "fileLength",
            "inlinePayloadLen",
            "peerDataRequestSessionId",
        ] {
            assert!(Reflect::get(&absent, &field.into()).unwrap().is_undefined());
        }
    }

    #[test]
    fn callback_error_messages_use_stable_js_values() {
        assert_eq!(js_error_message(&JsValue::from_str("failure")), "failure");
        let error = Object::new();
        Reflect::set(&error, &"message".into(), &JsValue::from_str("broken")).unwrap();
        assert_eq!(js_error_message(&error.into()), "broken");
        assert_eq!(
            js_error_message(&JsValue::TRUE),
            "JavaScript callback threw a non-string value"
        );
    }
}
