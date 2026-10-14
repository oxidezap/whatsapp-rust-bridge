use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Check if one or more phone numbers / JIDs are registered on WhatsApp.
                ///
                /// Accepts either bare phone numbers (treated as PN JIDs) or full JIDs
                /// (`@s.whatsapp.net` for PN, `@lid` for LID). Mixed PN/LID inputs are
                /// transparently split into the two underlying usync queries by the core,
                /// so a single call is at most two IQs regardless of input size.
                ///
                /// Returns one `IsOnWhatsAppResult` per server hit — including the LID
                /// counterpart and business flag — eliminating the follow-up `fetchUserInfo`
                /// round trip the previous single-phone API forced callers into.
                #[wasm_bindgen(js_name = isOnWhatsApp)]
            )),
            signature: parse_quote!(
                async fn is_on_whatsapp(
                    &self,
                    phones: Vec<String>,
                ) -> Result<Vec<Ts<crate::result_types::IsOnWhatsAppResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Codec(InputCodec::FullOrBareJid {
                binding: parse_quote!(jids),
                value: parse_quote!(phones),
                family: JidFamily::Pn,
                list: true,
            })],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.contacts().is_on_whatsapp(&jids)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(results),
                body: parse_quote!({
                    fn jid_to_owned(jid: &Jid) -> String {
                        let mut buf =
                            String::with_capacity(jid.user.len() + jid.server.as_str().len() + 8);
                        jid.push_to(&mut buf);
                        buf
                    }
                    to_ts_vec(
                        results
                            .iter()
                            .map(|r| crate::result_types::IsOnWhatsAppResult {
                                jid: jid_to_owned(&r.jid),
                                is_registered: r.is_registered,
                                lid: r.lid.as_ref().map(jid_to_owned),
                                pn_jid: r.pn_jid.as_ref().map(jid_to_owned),
                                is_business: r.is_business,
                                verified_name: r
                                    .verified_name
                                    .as_ref()
                                    .and_then(|v| v.name.clone()),
                                username: r.username.as_ref().map(|u| u.to_string()),
                            })
                            .collect(),
                    )
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Resolve a Meta username to the account behind it.
                ///
                /// **Experimental.** The core builds the request exactly as WhatsApp Web
                /// does, but no capture of a server answering it backs the implementation,
                /// so a rejection here is not necessarily a bug.
                ///
                /// `username` is the bare handle; a leading `@` is display-only and the
                /// core strips it. `usernameKey` is the account's numeric username key,
                /// which some accounts require before the server discloses an identity at
                /// all — without it the answer is `{ status: "keyRequired" }`.
                #[wasm_bindgen(js_name = findByUsername)]
            )),
            signature: parse_quote!(
                async fn find_by_username(
                    &self,
                    username: &str,
                    username_key: Option<String>,
                ) -> Result<Ts<crate::result_types::UsernameLookupResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Codec(InputCodec::Import(parse_quote!(
                use whatsapp_rust::features::UsernameLookup;
            )))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .contacts()
                    .find_by_username(username, username_key.as_deref())
            ),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(lookup),
                body: parse_quote!({
                    to_ts(match lookup {
                        UsernameLookup::NotFound => {
                            crate::result_types::UsernameLookupResult::NotFound
                        }
                        UsernameLookup::KeyRequired { username } => {
                            crate::result_types::UsernameLookupResult::KeyRequired {
                                username: username.map(|u| u.to_string()),
                            }
                        }
                        UsernameLookup::Found(user) => {
                            crate::result_types::UsernameLookupResult::Found {
                                jid: user.jid.to_string(),
                                pn_jid: user.pn_jid.as_ref().map(|j| j.to_string()),
                                username: user.username.map(|u| u.to_string()),
                                is_business: user.is_business,
                                verified_name: user.verified_name.and_then(|v| v.name),
                            }
                        }
                        other => {
                            return Err(crate::errors::internal(format!(
                                "unhandled username lookup answer: {other:?}"
                            )));
                        }
                    })
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the profile picture URL for a user or group.
                ///
                /// `picture_type` should be "preview" or "image".
                #[wasm_bindgen(js_name = profilePictureUrl)]
            )),
            signature: parse_quote!(
                async fn profile_picture_url(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "PictureType")] picture_type: JsValue,
                    timeout_ms: Option<f64>,
                ) -> Result<Option<Ts<crate::result_types::ProfilePictureInfo>>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(picture_type),
                    ty: parse_quote!(crate::result_types::PictureType),
                    field: parse_quote!("picture_type"),
                    value: parse_quote!(picture_type),
                },
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
                Input::Codec(InputCodec::Enum {
                    binding: parse_quote!(size),
                    value: parse_quote!(picture_type),
                    source: parse_quote!(crate::result_types::PictureType),
                    target: parse_quote!(whatsapp_rust::ProfilePictureType),
                    variants: vec![
                        (parse_quote!(Preview), parse_quote!(Preview)),
                        (parse_quote!(Image), parse_quote!(Full)),
                    ],
                }),
                Input::Timeout {
                    binding: parse_quote!(timeout),
                    field: parse_quote!("timeoutMs"),
                    value: parse_quote!(timeout_ms),
                    optional: true,
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client.pictures().lookup(
                    whatsapp_rust::ProfilePictureRequest::new(
                        profile_picture_target(&target),
                        size
                    )
                    .timeout(timeout),
                )
            ),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let result = boundary_result?.into_found();
                    to_ts_opt(result.map(|pic| crate::result_types::ProfilePictureInfo {
                        id: pic.id,
                        url: pic.url,
                        direct_path: pic.direct_path,
                        hash: pic.hash,
                    }))
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch user info for one or more JIDs.
                #[wasm_bindgen(js_name = fetchUserInfo , skip_typescript)]
            )),
            signature: parse_quote!(
                async fn fetch_user_info(
                    &self,
                    jids: Vec<String>,
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jids {
                binding: parse_quote!(parsed_jids),
                value: parse_quote!(jids),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.contacts().get_user_info(&parsed_jids)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({
                    let obj = js_sys::Object::new();
                    for (jid, info) in &result {
                        let entry = crate::result_types::UserInfoResult {
                            jid: info.jid.to_string(),
                            lid: info.lid.as_ref().map(|l| l.to_string()),
                            status: info.status.clone(),
                            picture_id: info.picture_id.clone(),
                            is_business: info.is_business,
                            verified_name: info.verified_name.as_ref().and_then(|v| v.name.clone()),
                            devices: info.devices.clone(),
                            username: info.username.as_ref().map(|u| u.to_string()),
                        };
                        let js_entry = serde_wasm_bindgen::to_value(&entry)?;
                        js_sys::Reflect::set(
                            &obj,
                            &JsValue::from_str(&jid.to_string()),
                            &js_entry,
                        )?;
                    }
                    Ok(obj.into())
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Set the user's push name (display name).
                #[wasm_bindgen(js_name = setPushName)]
            )),
            signature: parse_quote!(
                async fn set_push_name(&self, name: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.profile().set_push_name(name)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Read this account's own Meta username, its state and its username key.
                ///
                /// `null` means no username is set: the server answers 404 and the core
                /// reads it that way. Only the read is exposed — setting a username or its
                /// key changes the account's identity in a way the server does not undo,
                /// so the core leaves those two MEX operations unwrapped.
                #[wasm_bindgen(js_name = getUsername)]
            )),
            signature: parse_quote!(
                async fn get_username(
                    &self,
                ) -> Result<Option<Ts<crate::result_types::OwnUsernameResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.mex().get_username()),
            output: ReturnAdapter::Dto {
                projection: Projection::OwnUsername,
                envelope: Envelope::Optional,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Set the profile picture for the logged-in user.
                #[wasm_bindgen(js_name = updateProfilePicture)]
            )),
            signature: parse_quote!(
                async fn update_profile_picture(
                    &self,
                    img_data: Vec<u8>,
                ) -> Result<Ts<crate::result_types::ProfilePictureResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.profile().set_profile_picture(img_data)),
            output: ReturnAdapter::Dto {
                projection: Projection::ProfilePicture,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove the profile picture for the logged-in user.
                #[wasm_bindgen(js_name = removeProfilePicture)]
            )),
            signature: parse_quote!(
                async fn remove_profile_picture(
                    &self,
                ) -> Result<Ts<crate::result_types::ProfilePictureResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.profile().remove_profile_picture()),
            output: ReturnAdapter::Dto {
                projection: Projection::ProfilePicture,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Set the profile picture for a group the user administers.
                ///
                /// Mirrors the core `SetProfilePictureSpec::set_group` path — same IQ as
                /// the self update, just routed at the JID level so admins can change a
                /// group's avatar from JS without an extra capability check.
                #[wasm_bindgen(js_name = setGroupProfilePicture)]
            )),
            signature: parse_quote!(
                async fn set_group_profile_picture(
                    &self,
                    group_jid: &str,
                    img_data: Vec<u8>,
                ) -> Result<Ts<crate::result_types::ProfilePictureResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Codec(InputCodec::Import(parse_quote!(
                    use wacore_binary::JidExt;
                ))),
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(group_jid),
                },
                Input::Codec(InputCodec::JidGuard {
                    value: parse_quote!(target),
                    field: parse_quote!("groupJid"),
                    reason: parse_quote!("must be a group jid"),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.execute(
                wacore::iq::contacts::SetProfilePictureSpec::set_group(&target, img_data,)
            )),
            output: ReturnAdapter::Dto {
                projection: Projection::ProfilePicture,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Remove a group's profile picture.
                #[wasm_bindgen(js_name = removeGroupProfilePicture)]
            )),
            signature: parse_quote!(
                async fn remove_group_profile_picture(
                    &self,
                    group_jid: &str,
                ) -> Result<Ts<crate::result_types::ProfilePictureResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Codec(InputCodec::Import(parse_quote!(
                    use wacore_binary::JidExt;
                ))),
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(group_jid),
                },
                Input::Codec(InputCodec::JidGuard {
                    value: parse_quote!(target),
                    field: parse_quote!("groupJid"),
                    reason: parse_quote!("must be a group jid"),
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.execute(
                wacore::iq::contacts::SetProfilePictureSpec::remove_group(&target,)
            )),
            output: ReturnAdapter::Dto {
                projection: Projection::ProfilePicture,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Update the user's status text (about).
                #[wasm_bindgen(js_name = updateProfileStatus)]
            )),
            signature: parse_quote!(
                async fn update_profile_status(
                    &self,
                    status: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.profile().set_status_text(status)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch the full blocklist.
                #[wasm_bindgen(js_name = fetchBlocklist)]
            )),
            signature: parse_quote!(
                async fn fetch_blocklist(
                    &self,
                ) -> Result<Vec<Ts<crate::result_types::BlocklistEntryResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.blocking().get_blocklist()),
            output: ReturnAdapter::Dto {
                projection: Projection::BlocklistEntry,
                envelope: Envelope::List,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch all privacy settings.
                #[wasm_bindgen(js_name = fetchPrivacySettings)]
            )),
            signature: parse_quote!(
                async fn fetch_privacy_settings(&self) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.fetch_privacy_settings()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(response),
                body: parse_quote!({
                    let map: std::collections::HashMap<&str, &str> = response
                        .settings
                        .iter()
                        .map(|s| (s.category.as_str(), s.value.as_str()))
                        .collect();
                    serde_wasm_bindgen::to_value(&map).map_err(crate::errors::BridgeError::from)
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Update a single privacy setting.
                #[wasm_bindgen(js_name = updatePrivacySetting)]
            )),
            signature: parse_quote!(
                async fn update_privacy_setting(
                    &self,
                    category: &str,
                    value: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.set_privacy_setting(category.into(), value.into())),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Set default disappearing messages duration (seconds). 0 to disable.
                #[wasm_bindgen(js_name = updateDefaultDisappearingMode)]
            )),
            signature: parse_quote!(
                async fn update_default_disappearing_mode(
                    &self,
                    duration: u32,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.set_default_disappearing_mode(duration)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Send presence status ("available" or "unavailable").
                #[wasm_bindgen(js_name = sendPresence)]
            )),
            signature: parse_quote!(
                async fn send_presence(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "PresenceStatus")] status: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(status),
                    ty: parse_quote!(crate::result_types::PresenceStatus),
                    field: parse_quote!("status"),
                    value: parse_quote!(status),
                },
                Input::Codec(InputCodec::Enum {
                    binding: parse_quote!(presence_status),
                    value: parse_quote!(status),
                    source: parse_quote!(crate::result_types::PresenceStatus),
                    target: parse_quote!(whatsapp_rust::features::PresenceStatus),
                    variants: vec![
                        (parse_quote!(Available), parse_quote!(Available)),
                        (parse_quote!(Unavailable), parse_quote!(Unavailable)),
                    ],
                }),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.presence().set(presence_status)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Subscribe to a contact's presence updates.
                #[wasm_bindgen(js_name = presenceSubscribe)]
            )),
            signature: parse_quote!(
                async fn presence_subscribe(&self, jid: &str) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Redriven),
            call: parse_quote!(client.presence().subscribe(&target)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch user status/about text for one or more JIDs.
                #[wasm_bindgen(js_name = fetchStatus)]
            )),
            signature: parse_quote!(
                async fn fetch_status(
                    &self,
                    jids: Vec<String>,
                ) -> Result<Vec<Ts<crate::result_types::FetchStatusResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jids {
                binding: parse_quote!(parsed_jids),
                value: parse_quote!(jids),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.contacts().get_user_info(&parsed_jids)),
            output: ReturnAdapter::Dto {
                projection: Projection::FetchStatus,
                envelope: Envelope::Values,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Block or unblock a contact.
                #[wasm_bindgen(js_name = updateBlockStatus)]
            )),
            signature: parse_quote!(
                async fn update_block_status(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "BlockAction")] action: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(action),
                    ty: parse_quote!(crate::result_types::BlockAction),
                    field: parse_quote!("action"),
                    value: parse_quote!(action),
                },
                Input::Codec(InputCodec::Import(parse_quote!(
                    use crate::result_types::BlockAction;
                ))),
                Input::Jid {
                    binding: parse_quote!(target),
                    value: parse_quote!(jid),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                match action {
                    BlockAction::Block => client.blocking().block(&target).await?,
                    BlockAction::Unblock => client.blocking().unblock(&target).await?,
                }
                Ok(())
            }),
            output: ReturnAdapter::Direct,
        },
    ]
}
