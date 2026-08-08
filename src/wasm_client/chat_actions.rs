//! Chat actions, labels and quick replies (app state).
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Chat actions ──────────────────────────────────────────────────────

    /// Pin or unpin a chat.
    #[wasm_bindgen(js_name = pinChat)]
    pub async fn pin_chat(&self, jid: &str, pin: bool) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        if pin {
            self.client.chat_actions().pin_chat(&chat_jid).await
        } else {
            self.client.chat_actions().unpin_chat(&chat_jid).await
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a chat.
    ///
    /// Pass a positive timestamp (ms) to mute until that time, or null/undefined to unmute.
    #[wasm_bindgen(js_name = muteChat)]
    pub async fn mute_chat(
        &self,
        jid: &str,
        mute_until: Option<f64>,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        match mute_until {
            Some(ts) => {
                self.client
                    .chat_actions()
                    .mute_chat_until(&chat_jid, ts as i64)
                    .await
            }
            None => self.client.chat_actions().unmute_chat(&chat_jid).await,
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// Archive or unarchive a chat.
    #[wasm_bindgen(js_name = archiveChat)]
    pub async fn archive_chat(
        &self,
        jid: &str,
        archive: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        if archive {
            self.client
                .chat_actions()
                .archive_chat(&chat_jid, None)
                .await
        } else {
            self.client
                .chat_actions()
                .unarchive_chat(&chat_jid, None)
                .await
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// Save or rename a contact, syncing the name to the user's linked devices
    /// (a `contact` app-state mutation). `jid` must be a bare phone-number JID
    /// (the core rejects LID/group/device-specific JIDs).
    #[wasm_bindgen(js_name = saveContact)]
    pub async fn save_contact(
        &self,
        jid: &str,
        full_name: Option<String>,
        first_name: Option<String>,
        save_on_primary_addressbook: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let contact_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .save_contact(
                &contact_jid,
                full_name,
                first_name,
                save_on_primary_addressbook,
            )
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Star or unstar a message.
    #[wasm_bindgen(js_name = starMessage)]
    pub async fn star_message(
        &self,
        jid: &str,
        message_id: &str,
        star: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        if star {
            self.client
                .chat_actions()
                .star_message(&chat_jid, None, message_id, true)
                .await
        } else {
            self.client
                .chat_actions()
                .unstar_message(&chat_jid, None, message_id, true)
                .await
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// React to a DM, group, or status@broadcast message. Empty/null `emoji`
    /// removes a previous reaction. For group/status targets `key.participant`
    /// must carry the original sender (DMs don't need it; for your own message
    /// `fromMe: true` suffices). For a Community Announcement Group the core
    /// encrypts the reaction with the target's `messageSecret` and sends
    /// `enc_reaction_message` (WA Web `WAWebReactionEncryptMsgData`) — plaintext
    /// reactions are rejected there, so this path must be used instead of a
    /// JS-built `reactionMessage` proto. Returns the reaction's message id.
    #[wasm_bindgen(js_name = sendReaction)]
    pub async fn send_reaction(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "TargetMessageKey")] key: JsValue,
        emoji: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let key = from_js_input::<crate::result_types::TargetMessageKey>("key", key)?;
        let chat = parse_jid(jid)?;
        let target_key = waproto::whatsapp::MessageKey {
            remote_jid: Some(chat.to_string()),
            from_me: Some(key.from_me),
            id: Some(key.id),
            participant: key.participant,
        };
        let result = self
            .client
            .send_reaction(chat, target_key, emoji.as_deref().unwrap_or(""))
            .await
            .map_err(crate::errors::BridgeError::from)?;
        Ok(result.message_id)
    }

    /// Comment on a channel (CAG) post. `bytes` is the encoded body `Message`
    /// proto (encoding belongs to JS, like `sendMessageBytes`); `parent_key`
    /// references the post: `participant` is the post author, or `fromMe: true`
    /// for your own post (the core then resolves your LID/PN as the author).
    /// Requires the parent's `messageSecret`, captured when the post was
    /// received — the core derives the addon key and sends the encrypted
    /// comment envelope. Returns the comment's message id.
    #[wasm_bindgen(js_name = sendCommentBytes)]
    pub async fn send_comment_bytes(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "TargetMessageKey")] parent_key: JsValue,
        bytes: &[u8],
    ) -> Result<String, crate::errors::BridgeError> {
        let parent_key =
            from_js_input::<crate::result_types::TargetMessageKey>("parent_key", parent_key)?;
        // Without either, the core would fall back to the chat JID as the
        // author and fail the secret lookup the slow way — reject up front.
        if parent_key.participant.is_none() && !parent_key.from_me {
            return Err(crate::errors::internal(
                "parent_key needs participant (the post author) or fromMe: true",
            ));
        }
        let (chat, body) = parse_jid_and_msg_bytes(jid, bytes)?;
        let key = waproto::whatsapp::MessageKey {
            remote_jid: Some(chat.to_string()),
            from_me: Some(parent_key.from_me),
            id: Some(parent_key.id),
            participant: parent_key.participant,
        };
        let result = self
            .client
            .comments()
            .send_message(chat, key, body)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        Ok(result.message_id)
    }

    /// Mark a chat as read or unread via app state mutation.
    /// Different from readMessages (which sends read receipts).
    #[wasm_bindgen(js_name = markChatAsRead)]
    pub async fn mark_chat_as_read(
        &self,
        jid: &str,
        read: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .mark_chat_as_read(&chat_jid, read, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a chat via app state mutation.
    #[wasm_bindgen(js_name = deleteChat)]
    pub async fn delete_chat(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .delete_chat(&chat_jid, true, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Clear a chat's messages while keeping the chat (WA Web's clearChat), via an
    /// app-state mutation. `delete_starred` also removes starred messages and
    /// `delete_media` also removes downloaded media (both flags live in the mutation
    /// index, not the proto). Mirrors `deleteChat` in passing `None` for the message
    /// range, i.e. clears the whole chat.
    #[wasm_bindgen(js_name = clearChat)]
    pub async fn clear_chat(
        &self,
        jid: &str,
        delete_starred: bool,
        delete_media: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .clear_chat(&chat_jid, delete_starred, delete_media, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a message for self (not for everyone).
    #[wasm_bindgen(js_name = deleteMessageForMe)]
    pub async fn delete_message_for_me(
        &self,
        jid: &str,
        message_id: &str,
        from_me: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .delete_message_for_me(&chat_jid, None, message_id, from_me, true, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a contact's status updates.
    #[wasm_bindgen(js_name = setUserStatusMute)]
    pub async fn set_user_status_mute(
        &self,
        jid: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let user_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .set_user_status_mute(&user_jid, muted)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove a contact from the address book on every linked device.
    ///
    /// Separate from `saveContact` rather than `saveContact(null)`: this is the
    /// one contact mutation the core sends as a syncd `Remove`, and a `Set`
    /// carrying an empty action would be applied as a rename to the empty
    /// string. `jid` must be a bare phone-number JID.
    #[wasm_bindgen(js_name = removeContact)]
    pub async fn remove_contact(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let contact_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .remove_contact(&contact_jid)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Labels ────────────────────────────────────────────────────────────

    /// Create or rename a label. App state is an upsert keyed by `labelId`, so
    /// this both creates a label and edits an existing one. `color` is a
    /// WhatsApp color index.
    #[wasm_bindgen(js_name = createLabel)]
    pub async fn create_label(
        &self,
        label_id: &str,
        name: &str,
        color: i32,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .labels()
            .create_label(label_id, name, color)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a label.
    #[wasm_bindgen(js_name = deleteLabel)]
    pub async fn delete_label(&self, label_id: &str) -> Result<(), crate::errors::BridgeError> {
        self.client
            .labels()
            .delete_label(label_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Associate a label with a chat.
    #[wasm_bindgen(js_name = addChatLabel)]
    pub async fn add_chat_label(
        &self,
        label_id: &str,
        chat_jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .add_chat_label(label_id, &chat)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove a label association from a chat.
    #[wasm_bindgen(js_name = removeChatLabel)]
    pub async fn remove_chat_label(
        &self,
        label_id: &str,
        chat_jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .remove_chat_label(label_id, &chat)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Associate a label with a single message.
    ///
    /// Keyed by the message as well as the chat, under a different action than
    /// the chat association. One message per call, mirroring the wire.
    #[wasm_bindgen(js_name = addMessageLabel)]
    pub async fn add_message_label(
        &self,
        label_id: &str,
        chat_jid: &str,
        message_id: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .add_message_label(label_id, &chat, message_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove a label association from a single message.
    #[wasm_bindgen(js_name = removeMessageLabel)]
    pub async fn remove_message_label(
        &self,
        label_id: &str,
        chat_jid: &str,
        message_id: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .remove_message_label(label_id, &chat, message_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Quick replies ─────────────────────────────────────────────────────

    /// Create or edit a quick reply. App state is an upsert keyed by `id`.
    ///
    /// `shortcut` is the `/`-typed trigger and `message` the expanded text;
    /// `keywords` are extra search terms and `count` the usage tally (`0` for a
    /// new one).
    #[wasm_bindgen(js_name = setQuickReply)]
    pub async fn set_quick_reply(
        &self,
        id: &str,
        shortcut: &str,
        message: &str,
        keywords: Vec<String>,
        count: i32,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .quick_replies()
            .set_quick_reply(id, shortcut, message, keywords, count)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a quick reply.
    #[wasm_bindgen(js_name = deleteQuickReply)]
    pub async fn delete_quick_reply(&self, id: &str) -> Result<(), crate::errors::BridgeError> {
        self.client
            .quick_replies()
            .delete_quick_reply(id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── App state settings ────────────────────────────────────────────────

    /// Turn outgoing link previews off or on for the whole account.
    ///
    /// The account's stored preference, replicated to the linked devices. It
    /// does not stop this client from attaching a preview it was explicitly
    /// asked to send.
    #[wasm_bindgen(js_name = setLinkPreviewsDisabled)]
    pub async fn set_link_previews_disabled(
        &self,
        disabled: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .app_state_settings()
            .set_link_previews_disabled(disabled)
            .await
            .map_err(crate::errors::BridgeError::from)
    }
}
