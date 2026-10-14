use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Send an E2E encrypted message from protobuf bytes.
                /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
                #[wasm_bindgen(js_name = sendMessageBytes)]
            )),
            signature: parse_quote!(
                async fn send_message_bytes(
                    &self,
                    jid: &str,
                    bytes: &[u8],
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Message {
                binding: parse_quote!((to, msg)),
                jid: parse_quote!(jid),
                bytes: parse_quote!(bytes),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.send_message(to, msg)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::MessageId,
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Low-level message relay from protobuf binary bytes.
                #[wasm_bindgen(js_name = relayMessageBytes)]
            )),
            signature: parse_quote!(
                async fn relay_message_bytes(
                    &self,
                    jid: &str,
                    bytes: &[u8],
                    message_id: Option<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Message {
                    binding: parse_quote!((to, msg)),
                    jid: parse_quote!(jid),
                    bytes: parse_quote!(bytes),
                },
                Input::Codec(InputCodec::SendOptions {
                    binding: parse_quote!(options),
                    status: false,
                    extra_nodes: None,
                    group_freshness: None,
                    device_freshness: None,
                    message_id: parse_quote!(message_id),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(send_message_with_options(client, to, msg, options)),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Send an E2E message with neutral core-owned controls.
                ///
                /// Child nodes are converted only at the boundary. Routing, cache policy,
                /// encryption and reserved-node validation remain owned by the core.
                #[wasm_bindgen(js_name = relayMessageBytesWithOptions)]
            )),
            signature: parse_quote!(
                async fn relay_message_bytes_with_options(
                    &self,
                    jid: &str,
                    bytes: &[u8],
                    message_id: Option<String>,
                    extra_nodes: JsBinaryNodeArray,
                    refresh_group_metadata: bool,
                    refresh_devices: bool,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Message {
                    binding: parse_quote!((to, msg)),
                    jid: parse_quote!(jid),
                    bytes: parse_quote!(bytes),
                },
                Input::Codec(InputCodec::SendOptions {
                    binding: parse_quote!(options),
                    status: false,
                    extra_nodes: Some(parse_quote!(extra_nodes)),
                    group_freshness: Some(parse_quote!(refresh_group_metadata)),
                    device_freshness: Some(parse_quote!(refresh_devices)),
                    message_id: parse_quote!(message_id),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(send_message_with_options(client, to, msg, options)),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Retransmit an existing message to one requesting device.
                #[wasm_bindgen(js_name = retransmitMessageBytes)]
            )),
            signature: parse_quote!(
                async fn retransmit_message_bytes(
                    &self,
                    chat_jid: &str,
                    bytes: &[u8],
                    #[wasm_bindgen(unchecked_param_type = "MessageRetransmissionInput")] input: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(input),
                    ty: parse_quote!(crate::result_types::MessageRetransmissionInput),
                    field: parse_quote!("input"),
                    value: parse_quote!(input),
                },
                Input::Message {
                    binding: parse_quote!((chat, msg)),
                    jid: parse_quote!(chat_jid),
                    bytes: parse_quote!(bytes),
                },
                Input::Jid {
                    binding: parse_quote!(requester),
                    value: parse_quote!(&input.requester_jid),
                },
                Input::Convert(parse_quote!({
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
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.retransmit_message(request)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Edit a previously sent message from protobuf bytes.
                ///
                /// `stanzaId` overrides the edit's outer stanza id; omitted, the engine
                /// mints a fresh one. Pinning an existing message's id is best-effort —
                /// the server may dedupe against the outer id — so the visible outcome is
                /// not guaranteed. Returns the edited message's id, as before.
                #[wasm_bindgen(js_name = editMessageBytes)]
            )),
            signature: parse_quote!(
                async fn edit_message_bytes(
                    &self,
                    jid: &str,
                    message_id: &str,
                    bytes: &[u8],
                    stanza_id: Option<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Message {
                    binding: parse_quote!((to, msg)),
                    jid: parse_quote!(jid),
                    bytes: parse_quote!(bytes),
                },
                Input::Convert(parse_quote!({
                    let options = edit_options(stanza_id)?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.edit_message_raw(to, message_id, msg, options)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::MessageId,
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Revoke (delete) a sent message.
                #[wasm_bindgen(js_name = revokeMessage)]
            )),
            signature: parse_quote!(
                async fn revoke_message(
                    &self,
                    jid: &str,
                    message_id: &str,
                    participant: Option<String>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(to),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let revoke_type = match participant {
                        Some(p) => {
                            let sender = parse_jid(&p)?;
                            whatsapp_rust::RevokeType::Admin {
                                original_sender: sender,
                            }
                        }
                        None => whatsapp_rust::RevokeType::Sender,
                    };
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.revoke_message_raw(to, message_id, revoke_type)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Request on-demand message history from the primary phone.
                /// Returns the message ID of the PDO request.
                /// Results will arrive as history_sync events.
                #[wasm_bindgen(js_name = fetchMessageHistory)]
            )),
            signature: parse_quote!(
                async fn fetch_message_history(
                    &self,
                    count: i32,
                    chat_jid: &str,
                    oldest_msg_id: &str,
                    oldest_msg_from_me: bool,
                    oldest_msg_timestamp_ms: f64,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(chat),
                value: parse_quote!(chat_jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.fetch_message_history(
                &chat,
                oldest_msg_id,
                oldest_msg_from_me,
                oldest_msg_timestamp_ms as i64,
                count,
            )),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::Identity,
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Send a chat state update (typing indicator).
                #[wasm_bindgen(js_name = sendChatState)]
            )),
            signature: parse_quote!(
                async fn send_chat_state(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "ChatState")] state: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(state),
                    ty: parse_quote!(crate::result_types::ChatState),
                    field: parse_quote!("state"),
                    value: parse_quote!(state),
                },
                Input::Jid {
                    binding: parse_quote!(to),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::Enum {
                    binding: parse_quote!(chat_state),
                    value: parse_quote!(state),
                    source: parse_quote!(crate::result_types::ChatState),
                    target: parse_quote!(whatsapp_rust::features::ChatStateType),
                    variants: vec![
                        (parse_quote!(Composing), parse_quote!(Composing)),
                        (parse_quote!(Recording), parse_quote!(Recording)),
                        (parse_quote!(Paused), parse_quote!(Paused)),
                    ],
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.chatstate().send(&to, chat_state)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Create and send a poll. Returns `{ messageId, messageSecret }`.
                ///
                /// The `messageSecret` (32 bytes) is needed to decrypt votes later.
                #[wasm_bindgen(js_name = createPoll)]
            )),
            signature: parse_quote!(
                async fn create_poll(
                    &self,
                    jid: &str,
                    name: &str,
                    options: Vec<String>,
                    selectable_count: u32,
                ) -> Result<Ts<crate::result_types::CreatePollResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(to),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.polls().create(&to, name, &options, selectable_count)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(created),
                body: parse_quote!({
                    to_ts(crate::result_types::CreatePollResult {
                        message_id: created.send_result().message_id.to_string(),
                        message_secret: created.secret().as_bytes().to_vec(),
                    })
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Vote on a poll. Returns message ID.
                #[wasm_bindgen(js_name = votePoll)]
            )),
            signature: parse_quote!(
                async fn vote_poll(
                    &self,
                    chat_jid: &str,
                    poll_msg_id: &str,
                    poll_creator_jid: &str,
                    message_secret: &[u8],
                    option_names: Vec<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(chat),
                    value: parse_quote!(chat_jid),
                },
                Input::Jid {
                    binding: parse_quote!(creator),
                    value: parse_quote!(poll_creator_jid),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.polls().vote_raw(
                &chat,
                poll_msg_id,
                &creator,
                message_secret,
                &option_names
            )),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::MessageId,
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Send a status/story message to specified recipients.
                /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
                #[wasm_bindgen(js_name = sendStatusMessageBytes)]
            )),
            signature: parse_quote!(
                async fn send_status_message_bytes(
                    &self,
                    bytes: &[u8],
                    recipients: Vec<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Convert(parse_quote!({
                let (msg, recipients) = status_message_input(bytes, &recipients)?;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(send_status_message_with_options(
                client,
                msg,
                recipients,
                whatsapp_rust::StatusSendOptions::default(),
            )),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Send a status message with a caller-provided ID, neutral child nodes and
                /// an explicit recipient-device freshness policy.
                #[wasm_bindgen(js_name = sendStatusMessageBytesWithOptions)]
            )),
            signature: parse_quote!(
                async fn send_status_message_bytes_with_options(
                    &self,
                    bytes: &[u8],
                    recipients: Vec<String>,
                    message_id: Option<String>,
                    extra_nodes: JsBinaryNodeArray,
                    refresh_devices: bool,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Codec(InputCodec::SendOptions {
                    binding: parse_quote!(options),
                    status: true,
                    extra_nodes: Some(parse_quote!(extra_nodes)),
                    group_freshness: None,
                    device_freshness: Some(parse_quote!(refresh_devices)),
                    message_id: parse_quote!(message_id),
                }),
                Input::Convert(parse_quote!({
                    let (msg, recipients) = status_message_input(bytes, &recipients)?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(send_status_message_with_options(
                client, msg, recipients, options
            )),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Mark messages as read by sending read receipts.
                ///
                /// Held per chat rather than once for the batch: the core stamps the
                /// receipt's timestamp inside `mark_as_read`, so each one is sampled after
                /// its own wait rather than at the call the host made. Only the first of
                /// them can be withdrawn — after that the call has already sent something.
                #[wasm_bindgen(js_name = readMessages)]
            )),
            signature: parse_quote!(
                async fn read_messages(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "ReadMessageKey[]")] keys: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::CommittedReceipts,
            call: parse_quote!(client.mark_as_read(&chat, participant.as_ref(), &id_refs)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Mark voice/video notes as played by sending played receipts
                /// (`<receipt type="played"|"played-self">`). Groups keys by chat +
                /// participant exactly like [`Self::read_messages`]; the core picks
                /// `played` vs `played-self` (newsletters) and sets `participant` only for
                /// group/broadcast chats, so the JS side just hands over the message keys.
                #[wasm_bindgen(js_name = markPlayed)]
            )),
            signature: parse_quote!(
                async fn mark_played(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "ReadMessageKey[]")] keys: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::CommittedReceipts,
            call: parse_quote!(client.mark_as_played(&chat, participant.as_ref(), &id_refs)),
            output: ReturnAdapter::Unit,
        },
    ]
}
