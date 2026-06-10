//! Thin napi forwards for the messaging methods. The binding only converts at
//! the boundary (`Uint8Array` ↔ bytes, `BridgeValue` → JS via `JsBridgeValue`,
//! `BridgeError` → napi exception); all logic lives in `bridge-core`.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::WasmWhatsAppClient;
use crate::methods::napi_err;
use crate::value_js::JsBridgeValue;

#[napi]
impl WasmWhatsAppClient {
    #[napi(js_name = "relayMessageBytes")]
    pub async fn relay_message_bytes(
        &self,
        jid: String,
        bytes: Uint8Array,
        message_id: Option<String>,
    ) -> Result<String> {
        self.core
            .relay_message_bytes(&jid, &bytes, message_id)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "editMessageBytes")]
    pub async fn edit_message_bytes(
        &self,
        jid: String,
        message_id: String,
        bytes: Uint8Array,
    ) -> Result<String> {
        self.core
            .edit_message_bytes(&jid, &message_id, &bytes)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "revokeMessage")]
    pub async fn revoke_message(
        &self,
        jid: String,
        message_id: String,
        participant: Option<String>,
    ) -> Result<()> {
        self.core
            .revoke_message(&jid, &message_id, participant)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "deleteMessageForMe")]
    pub async fn delete_message_for_me(
        &self,
        jid: String,
        message_id: String,
        from_me: bool,
    ) -> Result<()> {
        self.core
            .delete_message_for_me(&jid, &message_id, from_me)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "createPoll")]
    pub async fn create_poll(
        &self,
        jid: String,
        name: String,
        options: Vec<String>,
        selectable_count: u32,
    ) -> Result<JsBridgeValue> {
        self.core
            .create_poll(&jid, &name, options, selectable_count)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "votePoll")]
    pub async fn vote_poll(
        &self,
        chat_jid: String,
        poll_msg_id: String,
        poll_creator_jid: String,
        message_secret: Uint8Array,
        option_names: Vec<String>,
    ) -> Result<String> {
        self.core
            .vote_poll(
                &chat_jid,
                &poll_msg_id,
                &poll_creator_jid,
                &message_secret,
                option_names,
            )
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "sendStatusMessageBytes")]
    pub async fn send_status_message_bytes(
        &self,
        bytes: Uint8Array,
        recipients: Vec<String>,
    ) -> Result<String> {
        self.core
            .send_status_message_bytes(&bytes, recipients)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "readMessages")]
    pub async fn read_messages(&self, keys: serde_json::Value) -> Result<()> {
        self.core.read_messages(keys).await.map_err(napi_err)
    }
}
