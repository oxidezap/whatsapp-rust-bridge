//! Sending, editing and acknowledging messages.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

/// One receipt's worth of keys: the chat it addresses, the participant it
/// names when the chat has one, and the message ids it covers.
type ReceiptBatch = (Jid, Option<Jid>, Vec<String>);

/// Message keys grouped into the batches a receipt is sent per: one per
/// chat-and-participant pair, since that pair is what addresses the stanza.
///
/// Shared by the read and played receipts, which differ only in the call they
/// end with — grouping the same keys two different ways is a bug waiting for
/// one copy to be edited.
///
/// Every JID is parsed before any receipt is sent, so one unparseable key
/// costs the whole call. That is a change: parsing used to happen inside the
/// send loop, so a bad key left whatever batches `HashMap` iteration had
/// already reached delivered. Which ones those were was not something a caller
/// could predict, let alone repeat.
fn group_receipt_keys(
    field: &'static str,
    keys: JsValue,
) -> Result<Vec<ReceiptBatch>, crate::errors::BridgeError> {
    let keys = from_js_input::<Vec<crate::result_types::ReadMessageKey>>(field, keys)?;

    let mut grouped: HashMap<(String, Option<String>), Vec<String>> = HashMap::new();
    for key in keys {
        grouped
            .entry((key.remote_jid, key.participant))
            .or_default()
            .push(key.id);
    }

    grouped
        .into_iter()
        .map(|((chat, participant), ids)| {
            Ok((
                parse_jid(&chat)?,
                participant.as_deref().map(parse_jid).transpose()?,
                ids,
            ))
        })
        .collect()
}

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Sending messages ─────────────────────────────────────────────────

    /// Send an E2E encrypted message from protobuf bytes.
    /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
    #[wasm_bindgen(js_name = sendMessageBytes)]
    pub async fn send_message_bytes(
        &self,
        jid: &str,
        bytes: &[u8],
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        let result = self.client.send_message(to, msg).await?;
        Ok(result.message_id)
    }

    /// Low-level message relay from protobuf binary bytes.
    #[wasm_bindgen(js_name = relayMessageBytes)]
    pub async fn relay_message_bytes(
        &self,
        jid: &str,
        bytes: &[u8],
        message_id: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        let mut options = whatsapp_rust::SendOptions::default();
        if let Some(message_id) = message_id {
            options = options.with_message_id(message_id);
        }
        send_message_with_options(&self.client, to, msg, options).await
    }

    /// Send an E2E message with neutral core-owned controls.
    ///
    /// Child nodes are converted only at the boundary. Routing, cache policy,
    /// encryption and reserved-node validation remain owned by the core.
    #[wasm_bindgen(js_name = relayMessageBytesWithOptions)]
    pub async fn relay_message_bytes_with_options(
        &self,
        jid: &str,
        bytes: &[u8],
        message_id: Option<String>,
        extra_nodes: JsBinaryNodeArray,
        refresh_group_metadata: bool,
        refresh_devices: bool,
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        let mut options = whatsapp_rust::SendOptions::default()
            .with_extra_stanza_nodes(js_node_array_to_vec(extra_nodes)?)
            .with_group_metadata_freshness(freshness(refresh_group_metadata))
            .with_device_freshness(freshness(refresh_devices));
        if let Some(message_id) = message_id {
            options = options.with_message_id(message_id);
        }
        send_message_with_options(&self.client, to, msg, options).await
    }

    /// Retransmit an existing message to one requesting device.
    #[wasm_bindgen(js_name = retransmitMessageBytes)]
    pub async fn retransmit_message_bytes(
        &self,
        chat_jid: &str,
        bytes: &[u8],
        #[wasm_bindgen(unchecked_param_type = "MessageRetransmissionInput")] input: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let input =
            from_js_input::<crate::result_types::MessageRetransmissionInput>("input", input)?;
        let (chat, msg) = parse_jid_and_msg_bytes(chat_jid, bytes)?;
        let requester = parse_jid(&input.requester_jid)?;
        let retry_count = u8::try_from(input.retry_count)
            .map_err(|_| crate::errors::internal("retry count exceeds the u8 range"))?;
        let mut request = whatsapp_rust::MessageRetransmission::new(
            chat,
            requester,
            msg,
            input.message_id,
            retry_count,
        )
        .with_group_metadata_freshness(freshness(input.refresh_group_metadata));
        if let Some(recipient_jid) = input.recipient_jid {
            request = request.with_recipient(parse_jid(&recipient_jid)?);
        }
        self.client.retransmit_message(request).await?;
        Ok(())
    }

    // ── Message management ──────────────────────────────────────────────

    /// Edit a previously sent message from protobuf bytes.
    ///
    /// `stanza_id` is optional and maps to
    /// [`whatsapp_rust::EditOptions::with_stanza_id`]: it overrides the outer
    /// stanza id so callers can collide the edit with an existing message and
    /// have clients re-render that slot. Without it, JS callers cannot reach
    /// that capability at all — `sendMessage`'s `messageId` option is dropped
    /// on the edit path, so the edit always goes out under the anchor's own id.
    ///
    /// Same contract as the Rust API: best-effort, no id-keyed local state is
    /// bound to the borrowed id, and honoring the collision is server/client
    /// dependent. Omitting the argument keeps the previous behavior exactly.
    #[wasm_bindgen(js_name = editMessageBytes)]
    pub async fn edit_message_bytes(
        &self,
        jid: &str,
        message_id: &str,
        bytes: &[u8],
        stanza_id: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        match stanza_id {
            Some(id) => self
                .client
                .edit_message_with_options(
                    to,
                    message_id,
                    msg,
                    whatsapp_rust::EditOptions::default().with_stanza_id(id),
                )
                .await
                .map_err(crate::errors::BridgeError::from),
            None => self
                .client
                .edit_message(to, message_id, msg)
                .await
                .map_err(crate::errors::BridgeError::from),
        }
    }

    /// Revoke (delete) a sent message.
    #[wasm_bindgen(js_name = revokeMessage)]
    pub async fn revoke_message(
        &self,
        jid: &str,
        message_id: &str,
        participant: Option<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let to = parse_jid(jid)?;

        let revoke_type = match participant {
            Some(p) => {
                let sender = parse_jid(&p)?;
                whatsapp_rust::RevokeType::Admin {
                    original_sender: sender,
                }
            }
            None => whatsapp_rust::RevokeType::Sender,
        };

        self.client
            .revoke_message(to, message_id, revoke_type)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Message history ──────────────────────────────────────────────────

    /// Request on-demand message history from the primary phone.
    /// Returns the message ID of the PDO request.
    /// Results will arrive as history_sync events.
    #[wasm_bindgen(js_name = fetchMessageHistory)]
    pub async fn fetch_message_history(
        &self,
        count: i32,
        chat_jid: &str,
        oldest_msg_id: &str,
        oldest_msg_from_me: bool,
        oldest_msg_timestamp_ms: f64,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        let msg_id = self
            .client
            .fetch_message_history(
                &chat,
                oldest_msg_id,
                oldest_msg_from_me,
                oldest_msg_timestamp_ms as i64,
                count,
            )
            .await?;
        Ok(msg_id)
    }

    // ── Read receipts ─────────────────────────────────────────────────

    /// Mark messages as read by sending read receipts.
    #[wasm_bindgen(js_name = readMessages)]
    pub async fn read_messages(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ReadMessageKey[]")] keys: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        for (chat, participant, ids) in group_receipt_keys("keys", keys)? {
            // #775: mark_as_read now takes &[&str] (alloc-aware); borrow the owned ids.
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_read(&chat, participant.as_ref(), &id_refs)
                .await?;
        }

        Ok(())
    }

    /// Mark voice/video notes as played by sending played receipts
    /// (`<receipt type="played"|"played-self">`). Groups keys by chat +
    /// participant exactly like [`Self::read_messages`]; the core picks
    /// `played` vs `played-self` (newsletters) and sets `participant` only for
    /// group/broadcast chats, so the JS side just hands over the message keys.
    #[wasm_bindgen(js_name = markPlayed)]
    pub async fn mark_played(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ReadMessageKey[]")] keys: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        for (chat, participant, ids) in group_receipt_keys("keys", keys)? {
            // #775: mark_as_played now takes &[&str] (alloc-aware); borrow the owned ids.
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_played(&chat, participant.as_ref(), &id_refs)
                .await?;
        }

        Ok(())
    }

    // ── Chat state ───────────────────────────────────────────────────────

    /// Send a chat state update (typing indicator).
    #[wasm_bindgen(js_name = sendChatState)]
    pub async fn send_chat_state(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "ChatState")] state: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let state = from_js_input::<crate::result_types::ChatState>("state", state)?;
        use crate::result_types::ChatState;
        let to = parse_jid(jid)?;

        let chat_state = match state {
            ChatState::Composing => whatsapp_rust::features::ChatStateType::Composing,
            ChatState::Recording => whatsapp_rust::features::ChatStateType::Recording,
            ChatState::Paused => whatsapp_rust::features::ChatStateType::Paused,
        };

        self.client
            .chatstate()
            .send(&to, chat_state)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Polls ─────────────────────────────────────────────────────────

    /// Create and send a poll. Returns `{ messageId, messageSecret }`.
    ///
    /// The `messageSecret` (32 bytes) is needed to decrypt votes later.
    #[wasm_bindgen(js_name = createPoll)]
    pub async fn create_poll(
        &self,
        jid: &str,
        name: &str,
        options: Vec<String>,
        selectable_count: u32,
    ) -> Result<crate::result_types::CreatePollResult, crate::errors::BridgeError> {
        let to = parse_jid(jid)?;
        let (result, message_secret) = self
            .client
            .polls()
            .create(&to, name, &options, selectable_count)
            .await?;
        Ok(crate::result_types::CreatePollResult {
            message_id: result.message_id,
            message_secret: message_secret.to_vec(),
        })
    }

    /// Vote on a poll. Returns message ID.
    #[wasm_bindgen(js_name = votePoll)]
    pub async fn vote_poll(
        &self,
        chat_jid: &str,
        poll_msg_id: &str,
        poll_creator_jid: &str,
        message_secret: &[u8],
        option_names: Vec<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        let creator = parse_jid(poll_creator_jid)?;
        let result = self
            .client
            .polls()
            .vote(&chat, poll_msg_id, &creator, message_secret, &option_names)
            .await?;
        Ok(result.message_id)
    }

    /// Send a status/story message to specified recipients.
    /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
    #[wasm_bindgen(js_name = sendStatusMessageBytes)]
    pub async fn send_status_message_bytes(
        &self,
        bytes: &[u8],
        recipients: Vec<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        send_status_message_with_options(
            &self.client,
            bytes,
            recipients,
            whatsapp_rust::StatusSendOptions::default(),
        )
        .await
    }

    /// Send a status message with a caller-provided ID, neutral child nodes and
    /// an explicit recipient-device freshness policy.
    #[wasm_bindgen(js_name = sendStatusMessageBytesWithOptions)]
    pub async fn send_status_message_bytes_with_options(
        &self,
        bytes: &[u8],
        recipients: Vec<String>,
        message_id: Option<String>,
        extra_nodes: JsBinaryNodeArray,
        refresh_devices: bool,
    ) -> Result<String, crate::errors::BridgeError> {
        let options = whatsapp_rust::StatusSendOptions {
            message_id,
            extra_stanza_nodes: js_node_array_to_vec(extra_nodes)?,
            device_freshness: freshness(refresh_devices),
            ..Default::default()
        };
        send_status_message_with_options(&self.client, bytes, recipients, options).await
    }
}
