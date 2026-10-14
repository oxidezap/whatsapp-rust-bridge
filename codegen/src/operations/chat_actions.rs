use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Save or rename a contact, syncing the name to the user's linked devices
                /// (a `contact` app-state mutation). `jid` must be a bare phone-number JID
                /// (the core rejects LID/group/device-specific JIDs).
                #[wasm_bindgen(js_name = saveContact)]
            )),
            signature: parse_quote!(
                async fn save_contact(
                    &self,
                    jid: &str,
                    full_name: Option<String>,
                    first_name: Option<String>,
                    save_on_primary_addressbook: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(contact_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.chat_actions().save_contact(
                &contact_jid,
                full_name,
                first_name,
                save_on_primary_addressbook,
            )),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// React to a DM, group, or status@broadcast message. Empty/null `emoji`
                /// removes a previous reaction. For group/status targets `key.participant`
                /// must carry the original sender (DMs don't need it; for your own message
                /// `fromMe: true` suffices). For a Community Announcement Group the core
                /// encrypts the reaction with the target's `messageSecret` and sends
                /// `enc_reaction_message` (WA Web `WAWebReactionEncryptMsgData`) — plaintext
                /// reactions are rejected there, so this path must be used instead of a
                /// JS-built `reactionMessage` proto. Returns the reaction's message id.
                #[wasm_bindgen(js_name = sendReaction)]
            )),
            signature: parse_quote!(
                async fn send_reaction(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "TargetMessageKey")] key: JsValue,
                    emoji: Option<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(key),
                    ty: parse_quote!(crate::result_types::TargetMessageKey),
                    field: parse_quote!("key"),
                    value: parse_quote!(key),
                },
                Input::Jid {
                    binding: parse_quote!(chat),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::MessageKey {
                    binding: parse_quote!(target_key),
                    chat: parse_quote!(chat),
                    key: parse_quote!(key),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.send_reaction_raw(
                chat,
                target_key,
                emoji.as_deref().unwrap_or("")
            )),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::MessageId,
                error: Some(ResultError::SendKey(parse_quote!("target_key"))),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Comment on a channel (CAG) post. `bytes` is the encoded body `Message`
                /// proto (encoding belongs to JS, like `sendMessageBytes`); `parent_key`
                /// references the post: `participant` is the post author, or `fromMe: true`
                /// for your own post (the core then resolves your LID/PN as the author).
                /// Requires the parent's `messageSecret`, captured when the post was
                /// received — the core derives the addon key and sends the encrypted
                /// comment envelope. Returns the comment's message id.
                #[wasm_bindgen(js_name = sendCommentBytes)]
            )),
            signature: parse_quote!(
                async fn send_comment_bytes(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "TargetMessageKey")] parent_key: JsValue,
                    bytes: &[u8],
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(parent_key),
                    ty: parse_quote!(crate::result_types::TargetMessageKey),
                    field: parse_quote!("parent_key"),
                    value: parse_quote!(parent_key),
                },
                Input::Codec(InputCodec::ParentKeyGuard {
                    key: parse_quote!(parent_key),
                }),
                Input::Message {
                    binding: parse_quote!((chat, body)),
                    jid: parse_quote!(jid),
                    bytes: parse_quote!(bytes),
                },
                Input::Codec(InputCodec::MessageKey {
                    binding: parse_quote!(key),
                    chat: parse_quote!(chat),
                    key: parse_quote!(parent_key),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.comments().send_message_raw(chat, key, body)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::MessageId,
                error: Some(ResultError::SendKey(parse_quote!("parent_key"))),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Mark a chat as read or unread via app state mutation.
                /// Different from readMessages (which sends read receipts).
                #[wasm_bindgen(js_name = markChatAsRead)]
            )),
            signature: parse_quote!(
                async fn mark_chat_as_read(
                    &self,
                    jid: &str,
                    read: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .chat_actions()
                    .mark_chat_as_read(&chat_jid, read, None)
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete a chat via app state mutation.
                #[wasm_bindgen(js_name = deleteChat)]
            )),
            signature: parse_quote!(
                async fn delete_chat(&self, jid: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.chat_actions().delete_chat(&chat_jid, true, None)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Clear a chat's messages while keeping the chat (WA Web's clearChat), via an
                /// app-state mutation. `delete_starred` also removes starred messages and
                /// `delete_media` also removes downloaded media (both flags live in the mutation
                /// index, not the proto). Mirrors `deleteChat` in passing `None` for the message
                /// range, i.e. clears the whole chat.
                #[wasm_bindgen(js_name = clearChat)]
            )),
            signature: parse_quote!(
                async fn clear_chat(
                    &self,
                    jid: &str,
                    delete_starred: bool,
                    delete_media: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.chat_actions().clear_chat(
                &chat_jid,
                delete_starred,
                delete_media,
                None
            )),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete a message for self (not for everyone).
                #[wasm_bindgen(js_name = deleteMessageForMe)]
            )),
            signature: parse_quote!(
                async fn delete_message_for_me(
                    &self,
                    jid: &str,
                    message_id: &str,
                    from_me: bool,
                    participant: Option<String>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat_jid),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::Scalar {
                    binding: parse_quote!(participant_jid),
                    value: parse_quote!(participant),
                    ty: parse_quote!(Jid),
                    field: parse_quote!("participant"),
                    prefix: None,
                    optional: true,
                }),
                Input::Codec(InputCodec::MessageReference {
                    chat: parse_quote!(chat_jid),
                    from_me: parse_quote!(from_me),
                    sender: parse_quote!(participant_jid.as_ref()),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .chat_actions()
                    .delete_message_for_me(&target, true, None)
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Mute or unmute a contact's status updates.
                #[wasm_bindgen(js_name = setUserStatusMute)]
            )),
            signature: parse_quote!(
                async fn set_user_status_mute(
                    &self,
                    jid: &str,
                    muted: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(user_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.chat_actions().set_user_status_mute(&user_jid, muted)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove a contact from the address book on every linked device.
                ///
                /// Separate from `saveContact` rather than `saveContact(null)`: this is the
                /// one contact mutation the core sends as a syncd `Remove`, and a `Set`
                /// carrying an empty action would be applied as a rename to the empty
                /// string. `jid` must be a bare phone-number JID.
                #[wasm_bindgen(js_name = removeContact)]
            )),
            signature: parse_quote!(
                async fn remove_contact(&self, jid: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(contact_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.chat_actions().remove_contact(&contact_jid)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Create or rename a label. App state is an upsert keyed by `labelId`, so
                /// this both creates a label and edits an existing one. `color` is a
                /// WhatsApp color index.
                #[wasm_bindgen(js_name = createLabel)]
            )),
            signature: parse_quote!(
                async fn create_label(
                    &self,
                    label_id: &str,
                    name: &str,
                    color: i32,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.labels().create_label(label_id, name, color)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete a label.
                #[wasm_bindgen(js_name = deleteLabel)]
            )),
            signature: parse_quote!(
                async fn delete_label(&self, label_id: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.labels().delete_label(label_id)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Associate a label with a chat.
                #[wasm_bindgen(js_name = addChatLabel)]
            )),
            signature: parse_quote!(
                async fn add_chat_label(
                    &self,
                    label_id: &str,
                    chat_jid: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat),
                value: parse_quote!(chat_jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.labels().add_chat_label(label_id, &chat)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove a label association from a chat.
                #[wasm_bindgen(js_name = removeChatLabel)]
            )),
            signature: parse_quote!(
                async fn remove_chat_label(
                    &self,
                    label_id: &str,
                    chat_jid: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat),
                value: parse_quote!(chat_jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.labels().remove_chat_label(label_id, &chat)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Associate a label with a single message.
                ///
                /// Keyed by the message as well as the chat, under a different action than
                /// the chat association. One message per call, mirroring the wire.
                #[wasm_bindgen(js_name = addMessageLabel)]
            )),
            signature: parse_quote!(
                async fn add_message_label(
                    &self,
                    label_id: &str,
                    chat_jid: &str,
                    message_id: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat),
                    value: parse_quote!(chat_jid),
                },
                Input::Codec(InputCodec::Scalar {
                    binding: parse_quote!(message_id),
                    value: parse_quote!(message_id),
                    ty: parse_quote!(whatsapp_rust::MessageId),
                    field: parse_quote!("messageId"),
                    prefix: None,
                    optional: false,
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .labels()
                    .add_message_label(label_id, &chat, &message_id)
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove a label association from a single message.
                #[wasm_bindgen(js_name = removeMessageLabel)]
            )),
            signature: parse_quote!(
                async fn remove_message_label(
                    &self,
                    label_id: &str,
                    chat_jid: &str,
                    message_id: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat),
                    value: parse_quote!(chat_jid),
                },
                Input::Codec(InputCodec::Scalar {
                    binding: parse_quote!(message_id),
                    value: parse_quote!(message_id),
                    ty: parse_quote!(whatsapp_rust::MessageId),
                    field: parse_quote!("messageId"),
                    prefix: None,
                    optional: false,
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .labels()
                    .remove_message_label(label_id, &chat, &message_id)
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Create or edit a quick reply. App state is an upsert keyed by `id`.
                ///
                /// `shortcut` is the `/`-typed trigger and `message` the expanded text;
                /// `keywords` are extra search terms and `count` the usage tally (`0` for a
                /// new one).
                #[wasm_bindgen(js_name = setQuickReply)]
            )),
            signature: parse_quote!(
                async fn set_quick_reply(
                    &self,
                    id: &str,
                    shortcut: &str,
                    message: &str,
                    keywords: Vec<String>,
                    count: i32,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .quick_replies()
                    .set_quick_reply(id, shortcut, message, keywords, count)
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete a quick reply.
                #[wasm_bindgen(js_name = deleteQuickReply)]
            )),
            signature: parse_quote!(
                async fn delete_quick_reply(&self, id: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.quick_replies().delete_quick_reply(id)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Turn outgoing link previews off or on for the whole account.
                ///
                /// The account's stored preference, replicated to the linked devices. It
                /// does not stop this client from attaching a preview it was explicitly
                /// asked to send.
                #[wasm_bindgen(js_name = setLinkPreviewsDisabled)]
            )),
            signature: parse_quote!(
                async fn set_link_previews_disabled(
                    &self,
                    disabled: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .app_state_settings()
                    .set_link_previews_disabled(disabled)
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Pin or unpin a chat.
                #[wasm_bindgen(js_name = pinChat)]
            )),
            signature: parse_quote!(
                async fn pin_chat(&self, jid: &str, pin: bool) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                if pin {
                    client.chat_actions().pin_chat(&chat_jid).await
                } else {
                    client.chat_actions().unpin_chat(&chat_jid).await
                }
                .map_err(crate::errors::BridgeError::from)
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Mute or unmute a chat.
                ///
                /// Pass a positive timestamp (ms) to mute until that time, or null/undefined to unmute.
                #[wasm_bindgen(js_name = muteChat)]
            )),
            signature: parse_quote!(
                async fn mute_chat(
                    &self,
                    jid: &str,
                    mute_until: Option<f64>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat_jid),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::OptionalMillis {
                    binding: parse_quote!(mute_until),
                    value: parse_quote!(mute_until),
                    field: parse_quote!("muteUntil"),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                match mute_until {
                    Some(until) => {
                        client
                            .chat_actions()
                            .mute_chat_until(&chat_jid, until)
                            .await
                    }
                    None => client.chat_actions().unmute_chat(&chat_jid).await,
                }
                .map_err(crate::errors::BridgeError::from)
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Archive or unarchive a chat.
                #[wasm_bindgen(js_name = archiveChat)]
            )),
            signature: parse_quote!(
                async fn archive_chat(
                    &self,
                    jid: &str,
                    archive: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat_jid),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                if archive {
                    client.chat_actions().archive_chat(&chat_jid, None).await
                } else {
                    client.chat_actions().unarchive_chat(&chat_jid, None).await
                }
                .map_err(crate::errors::BridgeError::from)
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Star or unstar a message.
                #[wasm_bindgen(js_name = starMessage)]
            )),
            signature: parse_quote!(
                async fn star_message(
                    &self,
                    jid: &str,
                    message_id: &str,
                    star: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat_jid),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::MessageReference {
                    chat: parse_quote!(chat_jid),
                    from_me: parse_quote!(true),
                    sender: parse_quote!(None),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                if star {
                    client.chat_actions().star_message(&target).await
                } else {
                    client.chat_actions().unstar_message(&target).await
                }
                .map_err(crate::errors::BridgeError::from)
            }),
            output: ReturnAdapter::Direct,
        },
    ]
}
