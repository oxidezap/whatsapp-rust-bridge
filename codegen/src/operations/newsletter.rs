use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Create a new newsletter (channel).
                #[wasm_bindgen(js_name = newsletterCreate)]
            )),
            signature: parse_quote!(
                async fn newsletter_create(
                    &self,
                    name: &str,
                    description: Option<String>,
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().create(name, description.as_deref())),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&result)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch metadata for a newsletter by JID.
                #[wasm_bindgen(js_name = newsletterMetadata)]
            )),
            signature: parse_quote!(
                async fn newsletter_metadata(
                    &self,
                    jid: &str,
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().get_metadata(&target)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&result)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Subscribe (join) a newsletter.
                #[wasm_bindgen(js_name = newsletterSubscribe)]
            )),
            signature: parse_quote!(
                async fn newsletter_subscribe(
                    &self,
                    jid: &str,
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().join(&target)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&result)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Unsubscribe (leave) a newsletter.
                #[wasm_bindgen(js_name = newsletterUnsubscribe)]
            )),
            signature: parse_quote!(
                async fn newsletter_unsubscribe(
                    &self,
                    jid: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().leave(&target)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Send a reaction to a newsletter message.
                ///
                /// `server_id` is the server-assigned message ID (passed as string to avoid
                /// JS number precision issues). `reaction` is the emoji code, or null/empty
                /// to remove a reaction.
                #[wasm_bindgen(js_name = newsletterReactMessage)]
            )),
            signature: parse_quote!(
                async fn newsletter_react_message(
                    &self,
                    jid: &str,
                    server_id: &str,
                    reaction: Option<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let sid: u64 = server_id.parse().map_err(|e: std::num::ParseIntError| {
                        crate::errors::invalid_arg("serverId", e.to_string())
                    })?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().send_reaction_raw(
                &target,
                sid,
                reaction.as_deref().unwrap_or("")
            )),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    boundary_result
                        .map(whatsapp_rust::StanzaId::into_string)
                        .map_err(crate::errors::BridgeError::from)
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Mute or unmute a newsletter's follower-activity notifications
                /// (`MUTE_FOLLOWER_ACTIVITY`) — what a subscriber toggles.
                #[wasm_bindgen(js_name = newsletterFollowerMute)]
            )),
            signature: parse_quote!(
                async fn newsletter_follower_mute(
                    &self,
                    jid: &str,
                    muted: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().set_follower_mute(&target, muted)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Mute or unmute a newsletter's admin-activity notifications
                /// (`MUTE_ADMIN_ACTIVITY`). Only meaningful for owners and admins.
                #[wasm_bindgen(js_name = newsletterAdminMute)]
            )),
            signature: parse_quote!(
                async fn newsletter_admin_mute(
                    &self,
                    jid: &str,
                    muted: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().set_admin_mute(&target, muted)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// List every newsletter this account is subscribed to.
                #[wasm_bindgen(js_name = newsletterList)]
            )),
            signature: parse_quote!(
                async fn newsletter_list(
                    &self,
                ) -> Result<Vec<Ts<crate::result_types::NewsletterMetadataResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().list_subscribed()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(list),
                body: parse_quote!({
                    to_ts_vec(list.iter().map(newsletter_metadata_to_result).collect())
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch a newsletter's metadata by its invite code.
                #[wasm_bindgen(js_name = newsletterMetadataByInvite)]
            )),
            signature: parse_quote!(
                async fn newsletter_metadata_by_invite(
                    &self,
                    invite_code: &str,
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().get_metadata_by_invite(invite_code)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(meta),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&meta)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Update a newsletter's name and/or description. Returns the refreshed
                /// metadata.
                #[wasm_bindgen(js_name = newsletterUpdate)]
            )),
            signature: parse_quote!(
                async fn newsletter_update(
                    &self,
                    jid: &str,
                    name: Option<String>,
                    description: Option<String>,
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().update(
                &target,
                name.as_deref(),
                description.as_deref()
            )),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(meta),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&meta)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete a newsletter. Owner-only.
                #[wasm_bindgen(js_name = newsletterDelete)]
            )),
            signature: parse_quote!(
                async fn newsletter_delete(&self, jid: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().delete(&target)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Transfer a newsletter's ownership. Owner-only.
                ///
                /// `user` may be a LID or a phone-number JID; the core resolves the latter
                /// to its LID, the only form the server addresses admins by.
                #[wasm_bindgen(js_name = newsletterChangeOwner)]
            )),
            signature: parse_quote!(
                async fn newsletter_change_owner(
                    &self,
                    jid: &str,
                    user: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Jid {
                    binding: parse_quote!(user_jid),
                    value: parse_quote!(user),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().change_owner(&target, &user_jid)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Demote a newsletter admin back to subscriber. Owner-only.
                #[wasm_bindgen(js_name = newsletterDemoteAdmin)]
            )),
            signature: parse_quote!(
                async fn newsletter_demote_admin(
                    &self,
                    jid: &str,
                    user: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Jid {
                    binding: parse_quote!(user_jid),
                    value: parse_quote!(user),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().demote_admin(&target, &user_jid)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Replace a newsletter's picture with the given JPEG bytes. Returns the
                /// refreshed metadata.
                #[wasm_bindgen(js_name = newsletterSetPicture)]
            )),
            signature: parse_quote!(
                async fn newsletter_set_picture(
                    &self,
                    jid: &str,
                    jpeg: &[u8],
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().set_picture(&target, jpeg)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(meta),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&meta)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove a newsletter's picture. Returns the refreshed metadata.
                #[wasm_bindgen(js_name = newsletterRemovePicture)]
            )),
            signature: parse_quote!(
                async fn newsletter_remove_picture(
                    &self,
                    jid: &str,
                ) -> Result<Ts<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().remove_picture(&target)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(meta),
                body: parse_quote!({ to_ts(newsletter_metadata_to_result(&meta)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch a newsletter's admin-side information, including its admin count.
                #[wasm_bindgen(js_name = newsletterAdminInfo)]
            )),
            signature: parse_quote!(
                async fn newsletter_admin_info(
                    &self,
                    jid: &str,
                ) -> Result<Ts<crate::result_types::NewsletterAdminInfoResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().get_admin_info(&target)),
            output: ReturnAdapter::Dto {
                projection: Projection::NewsletterAdminInfo,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch up to `count` followers of a newsletter.
                ///
                /// One page, no cursor: the core's query takes a count and the real client
                /// does not paginate this list.
                #[wasm_bindgen(js_name = newsletterFollowers)]
            )),
            signature: parse_quote!(
                async fn newsletter_followers(
                    &self,
                    jid: &str,
                    count: u32,
                ) -> Result<Vec<Ts<crate::result_types::NewsletterFollowerResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().get_followers(&target, count)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(followers),
                body: parse_quote!({
                    to_ts_vec(
                        followers
                            .iter()
                            .map(|follower| crate::result_types::NewsletterFollowerResult {
                                jid: follower.jid.to_string(),
                                phone_jid: follower.phone_jid.as_ref().map(|j| j.to_string()),
                                display_name: follower.display_name.clone(),
                                username: follower.username.clone(),
                                role: follower.role.as_ref().map(newsletter_role_str),
                                follow_time: follower.follow_time.map(|v| v as f64),
                                admin_profile: follower
                                    .admin_profile
                                    .as_ref()
                                    .map(crate::result_types::project_newsletter_admin_profile),
                            })
                            .collect(),
                    )
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch up to `count` messages from a newsletter's history.
                ///
                /// `before` is a `serverId` from a previous page, as a string because it is
                /// a u64. Omit it for the newest page.
                #[wasm_bindgen(js_name = newsletterMessages)]
            )),
            signature: parse_quote!(
                async fn newsletter_messages(
                    &self,
                    jid: &str,
                    count: u32,
                    before: Option<String>,
                ) -> Result<Vec<Ts<crate::result_types::NewsletterMessageResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let before_id = match before.as_deref() {
                        Some(value) => Some(value.parse::<u64>().map_err(|e| {
                            crate::errors::BridgeError::InvalidArgument {
                                field: "before".into(),
                                reason: format!("must be a serverId: {e}"),
                            }
                        })?),
                        None => None,
                    };
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().get_messages(target, count, before_id)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(messages),
                body: parse_quote!({
                    to_ts_vec(
                        messages
                            .iter()
                            .map(|message| crate::result_types::NewsletterMessageResult {
                                message_id: message.message_id.as_ref().map(ToString::to_string),
                                server_id: message.server_id.to_string(),
                                timestamp: message.timestamp as f64,
                                message_type: message.message_type.as_str().to_owned(),
                                message_type_raw: message.message_type_raw.clone(),
                                edit: message.edit.as_str().to_owned(),
                                is_sender: message.is_sender,
                                media_type: message
                                    .media_type
                                    .as_ref()
                                    .map(|v| v.as_str().to_owned()),
                                votes: message
                                    .votes
                                    .iter()
                                    .map(|vote| crate::result_types::NewsletterPollVoteResult {
                                        option_hash: serde_bytes::ByteBuf::from(
                                            vote.option_hash.to_vec(),
                                        ),
                                        count: vote.count.to_string(),
                                    })
                                    .collect(),
                                forwards_count: message.forwards_count.map(|v| v.to_string()),
                                views_count: message.views_count.map(|v| v.to_string()),
                                responses_count: message.responses_count.map(|v| v.to_string()),
                                original_timestamp: message
                                    .original_timestamp
                                    .map(|v| v.to_string()),
                                last_edit_timestamp_ms: message
                                    .last_edit_timestamp_ms
                                    .map(|v| v.to_string()),
                                poll_type: message
                                    .poll_type
                                    .as_ref()
                                    .map(|v| v.as_str().to_owned()),
                                poll_type_raw: message.poll_type_raw.clone(),
                                content_type: message.content_type.clone(),
                                question_type: message
                                    .question_type
                                    .as_ref()
                                    .map(|v| v.as_str().to_owned()),
                                message_association_type: message
                                    .message_association_type
                                    .as_ref()
                                    .map(|v| v.as_str().to_owned()),
                                is_wamo_sub: message.is_wamo_sub,
                                admin_profile: message
                                    .admin_profile
                                    .as_ref()
                                    .map(crate::result_types::project_newsletter_admin_profile),
                                rcat: message.rcat.clone().map(serde_bytes::ByteBuf::from),
                                message: message.message.as_ref().map(|m| {
                                    serde_bytes::ByteBuf::from(waproto::codec::message_to_vec(m))
                                }),
                                reactions: message
                                    .reactions
                                    .iter()
                                    .map(crate::result_types::project_newsletter_reaction_count)
                                    .collect(),
                            })
                            .collect(),
                    )
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Subscribe to a newsletter's live updates. Returns the subscription
                /// duration in seconds, as the server set it.
                #[wasm_bindgen(js_name = newsletterSubscribeLiveUpdates)]
            )),
            signature: parse_quote!(
                async fn newsletter_subscribe_live_updates(
                    &self,
                    jid: &str,
                ) -> Result<f64, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().subscribe_live_updates(target)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(duration),
                body: parse_quote!({ Ok(duration as f64) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Edit a newsletter message. `messageId` is the message's own id, not its
                /// `serverId` — reactions key on the latter, edit and revoke on the former.
                #[wasm_bindgen(js_name = newsletterEditMessage)]
            )),
            signature: parse_quote!(
                async fn newsletter_edit_message(
                    &self,
                    jid: &str,
                    message_id: &str,
                    message: &[u8],
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Message {
                    binding: parse_quote!((target, new_content)),
                    jid: parse_quote!(jid),
                    bytes: parse_quote!(message),
                },
                Input::Convert(parse_quote!({
                    let reference = newsletter_content_reference(&target, message_id)?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().edit_message(&reference, new_content)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Revoke (delete) a newsletter message. `messageId` is the message's own
                /// id, not its `serverId`.
                #[wasm_bindgen(js_name = newsletterRevokeMessage)]
            )),
            signature: parse_quote!(
                async fn newsletter_revoke_message(
                    &self,
                    jid: &str,
                    message_id: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let reference = newsletter_content_reference(&target, message_id)?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.newsletter().revoke_message(&reference)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Mute or unmute a newsletter's follower-activity notifications — the channel
                /// mute a subscriber toggles (WA Web `MUTE_FOLLOWER_ACTIVITY`). `muted = true`
                /// silences them.
                ///
                /// The name does not say which of the core's two mutes this is;
                /// `newsletterFollowerMute` is the explicit spelling and does the same
                /// thing. Kept so existing callers keep working.
                #[wasm_bindgen(js_name = newsletterMute)]
            )),
            signature: parse_quote!(
                async fn newsletter_mute(
                    &self,
                    jid: &str,
                    muted: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Alias),
            call: parse_quote!({ self.newsletter_follower_mute(jid, muted).await }),
            output: ReturnAdapter::Direct,
        },
    ]
}
