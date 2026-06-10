//! Messaging methods — engine-agnostic ports of the old `wasm_client.rs`
//! `WasmWhatsAppClient` message methods (relay/edit/revoke/delete/read/status,
//! polls). Logic is reproduced 1:1; only the JS-interop boundary differs (the
//! napi binding forwards primitives and materializes `BridgeValue`).

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::methods::snake;
use crate::value::BridgeValue;
use crate::{helpers, result_types};

use wacore_binary::jid::Jid;

impl WhatsAppClientCore {
    pub async fn relay_message_bytes(
        &self,
        jid: &str,
        bytes: &[u8],
        message_id: Option<String>,
    ) -> Result<String, BridgeError> {
        let (to, msg) = helpers::parse_jid_and_msg_bytes(jid, bytes)?;
        let options = whatsapp_rust::SendOptions {
            message_id,
            ..Default::default()
        };
        let result = self
            .client
            .send_message_with_options(to, msg, options)
            .await?;
        Ok(result.message_id)
    }

    pub async fn edit_message_bytes(
        &self,
        jid: &str,
        message_id: &str,
        bytes: &[u8],
    ) -> Result<String, BridgeError> {
        let (to, msg) = helpers::parse_jid_and_msg_bytes(jid, bytes)?;
        self.client
            .edit_message(to, message_id, msg)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn revoke_message(
        &self,
        jid: &str,
        message_id: &str,
        participant: Option<String>,
    ) -> Result<(), BridgeError> {
        let to = helpers::parse_jid(jid)?;

        let revoke_type = match participant {
            Some(p) => {
                let sender = helpers::parse_jid(&p)?;
                whatsapp_rust::RevokeType::Admin {
                    original_sender: sender,
                }
            }
            None => whatsapp_rust::RevokeType::Sender,
        };

        self.client
            .revoke_message(to, message_id, revoke_type)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn delete_message_for_me(
        &self,
        jid: &str,
        message_id: &str,
        from_me: bool,
    ) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        self.client
            .chat_actions()
            .delete_message_for_me(&chat_jid, None, message_id, from_me, true, None)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn create_poll(
        &self,
        jid: &str,
        name: &str,
        options: Vec<String>,
        selectable_count: u32,
    ) -> Result<BridgeValue, BridgeError> {
        let to = helpers::parse_jid(jid)?;
        let (result, message_secret) = self
            .client
            .polls()
            .create(&to, name, &options, selectable_count)
            .await?;
        snake(&result_types::CreatePollResult {
            message_id: result.message_id,
            message_secret: message_secret.to_vec(),
        })
    }

    pub async fn vote_poll(
        &self,
        chat_jid: &str,
        poll_msg_id: &str,
        poll_creator_jid: &str,
        message_secret: &[u8],
        option_names: Vec<String>,
    ) -> Result<String, BridgeError> {
        let chat = helpers::parse_jid(chat_jid)?;
        let creator = helpers::parse_jid(poll_creator_jid)?;
        let result = self
            .client
            .polls()
            .vote(&chat, poll_msg_id, &creator, message_secret, &option_names)
            .await?;
        Ok(result.message_id)
    }

    pub async fn send_status_message_bytes(
        &self,
        bytes: &[u8],
        recipients: Vec<String>,
    ) -> Result<String, BridgeError> {
        use prost::Message;
        let msg = waproto::whatsapp::Message::decode(bytes)
            .map_err(|e| errors::internal(format!("invalid message bytes: {e}")))?;
        let jids: Vec<Jid> = recipients
            .iter()
            .map(|s| helpers::parse_jid(s))
            .collect::<Result<_, _>>()?;
        let result = self
            .client
            .status()
            .send_raw(msg, &jids, Default::default())
            .await?;
        Ok(result.message_id)
    }

    pub async fn read_messages(&self, keys: serde_json::Value) -> Result<(), BridgeError> {
        use std::collections::HashMap;
        let keys: Vec<result_types::ReadMessageKey> =
            serde_json::from_value(keys).map_err(|e| errors::invalid_arg("keys", e.to_string()))?;

        let mut grouped: HashMap<(String, Option<String>), Vec<String>> = HashMap::new();

        for key in keys {
            grouped
                .entry((key.remote_jid, key.participant))
                .or_default()
                .push(key.id);
        }

        for ((chat_jid_str, participant_str), ids) in grouped {
            let chat_jid = helpers::parse_jid(&chat_jid_str)?;
            let participant_jid = participant_str
                .as_deref()
                .map(helpers::parse_jid)
                .transpose()?;

            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_read(&chat_jid, participant_jid.as_ref(), &id_refs)
                .await?;
        }

        Ok(())
    }
}
