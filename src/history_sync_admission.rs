use js_sys::{Function, Object, Reflect};
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
        if !value.is_object() {
            return Err(crate::errors::invalid_arg("policies", "must be an object"));
        }
        let callback = Reflect::get(value, &"historySyncAdmission".into()).map_err(|_| {
            crate::errors::invalid_arg(
                "policies.historySyncAdmission",
                "could not read the callback",
            )
        })?;
        if callback.is_undefined() || callback.is_null() {
            return Ok(None);
        }
        let callback = callback.dyn_into::<Function>().map_err(|_| {
            crate::errors::invalid_arg("policies.historySyncAdmission", "must be a function")
        })?;
        Ok(Some(Self {
            callback,
            receiver: value.clone(),
        }))
    }

    fn call(&self, value: &JsValue) -> HistorySyncDecision {
        match self.callback.call1(&self.receiver, value) {
            Ok(result) if result.as_bool() == Some(true) => HistorySyncDecision::Accept,
            Ok(_) => HistorySyncDecision::RejectAndAcknowledge,
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

impl HistorySyncAdmission for JsHistorySyncAdmission {
    fn decide(&self, metadata: &HistorySyncMetadata<'_>) -> HistorySyncDecision {
        let value = Object::new();
        set_optional_number(
            &value,
            "syncType",
            metadata.sync_type.map(|value| value as f64),
        );
        set_optional_number(
            &value,
            "chunkOrder",
            metadata.chunk_order.map(|value| value as f64),
        );
        set_optional_number(
            &value,
            "progress",
            metadata.progress.map(|value| value as f64),
        );
        set_file_length(&value, metadata.file_length);
        set_optional_number(
            &value,
            "inlinePayloadLen",
            metadata.inline_payload_len.map(|value| value as f64),
        );
        if let Some(session_id) = metadata.peer_data_request_session_id {
            let _ = Reflect::set(
                &value,
                &"peerDataRequestSessionId".into(),
                &JsValue::from_str(session_id),
            );
        }

        self.call(&value)
    }
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
