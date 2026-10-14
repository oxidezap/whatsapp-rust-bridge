use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Get metadata for a group.
                #[wasm_bindgen(js_name = getGroupMetadata)]
            )),
            signature: parse_quote!(
                async fn group_metadata(
                    &self,
                    jid: &str,
                ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(group_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().fetch_metadata(&group_jid)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(metadata),
                expression: parse_quote!(to_ts(group_metadata_to_result(&metadata))),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Update a group's subject (name).
                #[wasm_bindgen(js_name = groupUpdateSubject)]
            )),
            signature: parse_quote!(
                async fn group_update_subject(
                    &self,
                    jid: &str,
                    subject: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group_jid),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let group_subject = whatsapp_rust::features::GroupSubject::new(subject)?;
                })),
            ],
            admission: Admission::Online,
            call: parse_quote!(client.groups().set_subject(&group_jid, group_subject)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Update a group's description. Pass null/undefined to remove.
                #[wasm_bindgen(js_name = groupUpdateDescription)]
            )),
            signature: parse_quote!(
                async fn group_update_description(
                    &self,
                    jid: &str,
                    description: Option<String>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group_jid),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let desc = description
                        .as_deref()
                        .map(whatsapp_rust::features::GroupDescription::new)
                        .transpose()?;
                })),
            ],
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .groups()
                    // The caller holds no description id, so the core reads the current
                    // one before sending: without that token the server rejects every
                    // update of a group that already has a description.
                    .set_description(
                        &group_jid,
                        desc,
                        whatsapp_rust::features::PreviousDescription::Resolve,
                    )
            ),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Leave a group.
                #[wasm_bindgen(js_name = groupLeave)]
            )),
            signature: parse_quote!(
                async fn group_leave(&self, jid: &str) -> Result<(), crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(group_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().leave(&group_jid)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Set or clear the bot's per-group "member label" — the small tag rendered
                /// under the bot's display name inside that group's UI. Empty `label`
                /// clears the label. The core sends this as a `ProtocolMessage` over the
                /// normal message path (not an IQ), matching WA Web's behavior.
                #[wasm_bindgen(js_name = updateMemberLabel)]
            )),
            signature: parse_quote!(
                async fn update_member_label(
                    &self,
                    group_jid: &str,
                    label: &str,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(parsed),
                value: parse_quote!(group_jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().update_member_label_with_id(&parsed, label)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the invite link for a group.
                #[wasm_bindgen(js_name = groupInviteCode)]
            )),
            signature: parse_quote!(
                async fn group_invite_code(&self, jid: &str) -> Result<String, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(group_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_invite_link(&group_jid, false)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Set disappearing messages timer for a group (0 to disable).
                #[wasm_bindgen(js_name = groupToggleEphemeral)]
            )),
            signature: parse_quote!(
                async fn group_toggle_ephemeral(
                    &self,
                    jid: &str,
                    expiration: u32,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(group_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().set_ephemeral(&group_jid, expiration)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Revoke a group's invite link (generates new one).
                #[wasm_bindgen(js_name = groupRevokeInvite)]
            )),
            signature: parse_quote!(
                async fn group_revoke_invite(
                    &self,
                    jid: &str,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(group_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_invite_link(&group_jid, true)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(new_code),
                expression: parse_quote!(Ok(new_code)),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Deactivate a parent group without deleting its former subgroups.
                #[wasm_bindgen(js_name = deactivateCommunity)]
            )),
            signature: parse_quote!(
                async fn deactivate_community(&self, jid: &str) -> Result<(), crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(target),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.community().deactivate(target)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Link existing groups to a parent group.
                #[wasm_bindgen(js_name = linkCommunitySubgroups)]
            )),
            signature: parse_quote!(
                async fn link_community_subgroups(
                    &self,
                    parent_jid: &str,
                    subgroup_jids: Vec<String>,
                ) -> Result<Ts<crate::result_types::CommunityLinkResult>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(parent),
                    value: parse_quote!(parent_jid),
                },
                Input::Convert(parse_quote!({
                    let subgroups = subgroup_jids
                        .iter()
                        .map(|subgroup| parse_jid(subgroup))
                        .collect::<Result<Vec<_>, _>>()?;
                })),
            ],
            admission: Admission::Online,
            call: parse_quote!(client.community().link_subgroups(parent, &subgroups)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(result),
                expression: parse_quote!(to_ts(community_link_result(
                    result.linked_jids,
                    result.failed_groups,
                ))),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Unlink groups from a parent group.
                #[wasm_bindgen(js_name = unlinkCommunitySubgroups)]
            )),
            signature: parse_quote!(
                async fn unlink_community_subgroups(
                    &self,
                    parent_jid: &str,
                    subgroup_jids: Vec<String>,
                    remove_orphan_members: bool,
                ) -> Result<Ts<crate::result_types::CommunityLinkResult>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(parent),
                    value: parse_quote!(parent_jid),
                },
                Input::Convert(parse_quote!({
                    let subgroups = subgroup_jids
                        .iter()
                        .map(|subgroup| parse_jid(subgroup))
                        .collect::<Result<Vec<_>, _>>()?;
                })),
            ],
            admission: Admission::Online,
            call: parse_quote!(client.community().unlink_subgroups(
                parent,
                &subgroups,
                remove_orphan_members
            )),
            output: ReturnAdapter::Project {
                binding: parse_quote!(result),
                expression: parse_quote!(to_ts(community_link_result(
                    result.unlinked_jids,
                    result.failed_groups,
                ))),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch subgroups of a parent group.
                #[wasm_bindgen(js_name = getCommunitySubgroups)]
            )),
            signature: parse_quote!(
                async fn get_community_subgroups(
                    &self,
                    parent_jid: &str,
                ) -> Result<Vec<Ts<crate::result_types::CommunitySubgroupResult>>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(parent),
                value: parse_quote!(parent_jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.community().get_subgroups(&parent)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(groups),
                expression: parse_quote!(to_ts_vec(
                    groups
                        .into_iter()
                        .map(|group| crate::result_types::CommunitySubgroupResult {
                            id: group.id.to_string(),
                            subject: group.subject,
                            participant_count: group.participant_count.map(f64::from),
                            creation: group.creation.map(|value| value as f64),
                            owner: group.owner.map(|value| value.to_string()),
                            is_default_sub_group: group.is_default_sub_group,
                            is_general_chat: group.is_general_chat,
                        })
                        .collect(),
                )),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Join a group using an invite code.
                #[wasm_bindgen(js_name = groupAcceptInvite)]
            )),
            signature: parse_quote!(
                async fn group_accept_invite(
                    &self,
                    code: &str,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![],
            admission: Admission::Online,
            call: parse_quote!(client.groups().join_with_invite_code(code)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(jid),
                expression: parse_quote!(Ok(jid.group_jid().to_string())),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Join a group via a GroupInviteMessage (V4 invite).
                #[wasm_bindgen(js_name = groupAcceptInviteV4)]
            )),
            signature: parse_quote!(
                async fn group_accept_invite_v4(
                    &self,
                    group_jid: &str,
                    code: &str,
                    expiration: f64,
                    admin_jid: &str,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(admin),
                    value: parse_quote!(admin_jid),
                },
            ],
            admission: Admission::Online,
            call: parse_quote!(client.groups().join_with_invite_v4(
                &group,
                code,
                expiration as i64,
                &admin
            )),
            output: ReturnAdapter::Project {
                binding: parse_quote!(result),
                expression: parse_quote!(Ok(result.group_jid().to_string())),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Revoke invitation codes previously issued to participants.
                #[wasm_bindgen(js_name = groupRevokeInviteV4)]
            )),
            signature: parse_quote!(
                async fn group_revoke_invite_v4(
                    &self,
                    group_jid: &str,
                    invited_jid: &str,
                ) -> Result<bool, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(invited),
                    value: parse_quote!(invited_jid),
                },
            ],
            admission: Admission::Online,
            call: parse_quote!(client.groups().revoke_request_code(&group, &[invited])),
            output: ReturnAdapter::Project {
                binding: parse_quote!(responses),
                expression: parse_quote!(Ok(responses.iter().all(|response| response.is_ok()))),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Get group info from an invite code (without joining).
                /// Returns the same shape as groupMetadata.
                #[wasm_bindgen(js_name = groupGetInviteInfo)]
            )),
            signature: parse_quote!(
                async fn group_get_invite_info(
                    &self,
                    code: &str,
                ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![],
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_invite_info(code)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(metadata),
                expression: parse_quote!(to_ts(group_metadata_to_result(&metadata))),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Get list of pending join requests for a group.
                #[wasm_bindgen(js_name = groupRequestParticipantsList)]
            )),
            signature: parse_quote!(
                async fn group_request_participants_list(
                    &self,
                    jid: &str,
                ) -> Result<Vec<Ts<crate::result_types::MembershipRequestResult>>, crate::errors::BridgeError>
            ),
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            inputs: vec![Input::Jid {
                binding: parse_quote!(group_jid),
                value: parse_quote!(jid),
            }],
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_membership_requests(&group_jid)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(list),
                expression: parse_quote!(to_ts_vec(
                    list.iter()
                        .map(|r| crate::result_types::MembershipRequestResult {
                            jid: r.jid.to_string(),
                            request_time: r.request_time.map(|t| t as f64),
                        })
                        .collect(),
                )),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Create a new group.
                ///
                /// Returns the full `GroupMetadataResult` parsed directly from the
                /// server's create response — no follow-up `getGroupMetadata` IQ
                /// needed. Mirrors the reference client: the create reply
                /// already carries the complete `<group>` node (id, subject,
                /// creation, creator, participants, …).
                #[wasm_bindgen(js_name = createGroup)]
            )),
            signature: parse_quote!(
                async fn group_create(
                    &self,
                    subject: &str,
                    participants: Vec<String>,
                ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Convert(parse_quote!({
                use whatsapp_rust::features::GroupParticipantOptions;
                let participant_options: Vec<GroupParticipantOptions> = participants
                    .iter()
                    .map(|p| parse_jid(p).map(GroupParticipantOptions::new))
                    .collect::<Result<_, _>>()?;
                let options = whatsapp_rust::features::GroupCreateOptions::new(subject)
                    .with_participants(participant_options);
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.groups().create_group(options)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({ to_ts(group_metadata_to_result(&result.metadata)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Update group participants.
                #[wasm_bindgen(js_name = groupParticipantsUpdate)]
            )),
            signature: parse_quote!(
                async fn group_participants_update(
                    &self,
                    jid: &str,
                    participants: Vec<String>,
                    #[wasm_bindgen(unchecked_param_type = "GroupParticipantAction")] action: JsValue,
                ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(action),
                    ty: parse_quote!(crate::result_types::GroupParticipantAction),
                    field: parse_quote!("action"),
                    value: parse_quote!(action),
                },
                Input::Convert(parse_quote!({
                    let (group_jid, participant_jids) =
                        participants_update_input(jid, &participants, action)?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(participants_update(
                client,
                group_jid,
                participant_jids,
                action,
                false,
            )),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch all groups the user is participating in.
                #[wasm_bindgen(js_name = groupFetchAllParticipating , skip_typescript)]
            )),
            signature: parse_quote!(
                async fn group_fetch_all_participating(
                    &self,
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.groups().list_participating()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(groups),
                body: parse_quote!({
                    let obj = js_sys::Object::new();
                    for group in &groups {
                        let result = group_overview_to_result(group);
                        let js_metadata = serde_wasm_bindgen::to_value(&result)?;
                        js_sys::Reflect::set(&obj, &JsValue::from_str(&result.id), &js_metadata)?;
                    }
                    Ok(obj.into())
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Create a parent group with explicit protocol options.
                #[wasm_bindgen(js_name = createCommunity)]
            )),
            signature: parse_quote!(
                async fn create_community(
                    &self,
                    name: &str,
                    description: Option<String>,
                    closed: bool,
                    allow_non_admin_sub_group_creation: bool,
                    create_general_chat: bool,
                ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Convert(parse_quote!({
                let mut options = whatsapp_rust::features::CreateCommunityOptions::new(name)
                    .map_err(|e| crate::errors::invalid_arg("name", e.to_string()))?;
                options.description = description
                    .map(whatsapp_rust::features::GroupDescription::new)
                    .transpose()
                    .map_err(|e| crate::errors::invalid_arg("description", e.to_string()))?;
                options.closed = closed;
                options.allow_non_admin_sub_group_creation = allow_non_admin_sub_group_creation;
                options.create_general_chat = create_general_chat;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.community().create(options)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({ to_ts(group_metadata_to_result(&result.metadata)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Create a subgroup already linked to a parent group.
                #[wasm_bindgen(js_name = createCommunitySubgroup)]
            )),
            signature: parse_quote!(
                async fn create_community_subgroup(
                    &self,
                    name: &str,
                    participants: Vec<String>,
                    parent_jid: &str,
                ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Convert(parse_quote!({
                    let participants = participants
                        .iter()
                        .map(|participant| parse_jid(participant))
                        .collect::<Result<Vec<_>, _>>()?;
                })),
                Input::Jid {
                    binding: parse_quote!(parent),
                    value: parse_quote!(parent_jid),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .community()
                    .create_subgroup(name, &participants, parent)
            ),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({ to_ts(group_metadata_to_result(&result.metadata)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch all parent groups the account currently participates in.
                #[wasm_bindgen(js_name = communityFetchAllParticipating , skip_typescript)]
            )),
            signature: parse_quote!(
                async fn community_fetch_all_participating(
                    &self,
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.community().list_participating()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(communities),
                body: parse_quote!({
                    let result = js_sys::Object::new();
                    for group in &communities {
                        let value = serde_wasm_bindgen::to_value(&group_overview_to_result(group))?;
                        js_sys::Reflect::set(&result, &group.id.to_string().into(), &value)?;
                    }
                    Ok(result.into())
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Update participants of a parent group. Removing a participant also
                /// removes them from its linked groups.
                #[wasm_bindgen(js_name = communityParticipantsUpdate)]
            )),
            signature: parse_quote!(
                async fn community_participants_update(
                    &self,
                    jid: &str,
                    participants: Vec<String>,
                    #[wasm_bindgen(unchecked_param_type = "GroupParticipantAction")] action: JsValue,
                ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(action),
                    ty: parse_quote!(crate::result_types::GroupParticipantAction),
                    field: parse_quote!("action"),
                    value: parse_quote!(action),
                },
                Input::Convert(parse_quote!({
                    let (group_jid, participant_jids) =
                        participants_update_input(jid, &participants, action)?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(participants_update(
                client,
                group_jid,
                participant_jids,
                action,
                true,
            )),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Set who can add members to a group.
                #[wasm_bindgen(js_name = groupMemberAddMode)]
            )),
            signature: parse_quote!(
                async fn group_member_add_mode(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "MemberAddMode")] mode: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(mode),
                    ty: parse_quote!(crate::result_types::MemberAddMode),
                    field: parse_quote!("mode"),
                    value: parse_quote!(mode),
                },
                Input::Convert(parse_quote!({
                    use crate::result_types::MemberAddMode;
                })),
                Input::Jid {
                    binding: parse_quote!(group_jid),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let add_mode = match mode {
                        MemberAddMode::AdminAdd => whatsapp_rust::features::MemberAddMode::AdminAdd,
                        MemberAddMode::AllMemberAdd => {
                            whatsapp_rust::features::MemberAddMode::AllMemberAdd
                        }
                    };
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.groups().set_member_add_mode(&group_jid, add_mode)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Update a group setting (locked, announce, membership_approval).
                #[wasm_bindgen(js_name = groupSettingUpdate)]
            )),
            signature: parse_quote!(
                async fn group_setting_update(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "GroupSetting")] setting: JsValue,
                    value: bool,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(setting),
                    ty: parse_quote!(crate::result_types::GroupSetting),
                    field: parse_quote!("setting"),
                    value: parse_quote!(setting),
                },
                Input::Convert(parse_quote!({
                    use crate::result_types::GroupSetting;
                })),
                Input::Jid {
                    binding: parse_quote!(group_jid),
                    value: parse_quote!(jid),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                match setting {
                    GroupSetting::Locked => client.groups().set_locked(&group_jid, value).await?,
                    GroupSetting::Announce => {
                        client.groups().set_announce(&group_jid, value).await?
                    }
                    GroupSetting::MembershipApproval => {
                        let mode = if value {
                            whatsapp_rust::MembershipApprovalMode::On
                        } else {
                            whatsapp_rust::MembershipApprovalMode::Off
                        };
                        client
                            .groups()
                            .set_membership_approval(&group_jid, mode)
                            .await?;
                    }
                }
                Ok(())
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Approve or reject pending join requests.
                #[wasm_bindgen(js_name = groupRequestParticipantsUpdate)]
            )),
            signature: parse_quote!(
                async fn group_request_participants_update(
                    &self,
                    jid: &str,
                    participants: Vec<String>,
                    #[wasm_bindgen(unchecked_param_type = "GroupRequestAction")] action: JsValue,
                ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(action),
                    ty: parse_quote!(crate::result_types::GroupRequestAction),
                    field: parse_quote!("action"),
                    value: parse_quote!(action),
                },
                Input::Convert(parse_quote!({
                    use crate::result_types::GroupRequestAction;
                })),
                Input::Jid {
                    binding: parse_quote!(group_jid),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let participant_jids: Vec<Jid> = participants
                        .iter()
                        .map(|s| parse_jid(s))
                        .collect::<Result<Vec<_>, _>>()?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Online,
            call: parse_quote!({
                let responses = match action {
                    GroupRequestAction::Approve => {
                        client
                            .groups()
                            .approve_membership_requests(&group_jid, &participant_jids)
                            .await?
                    }
                    GroupRequestAction::Reject => {
                        client
                            .groups()
                            .reject_membership_requests(&group_jid, &participant_jids)
                            .await?
                    }
                };
                to_ts_vec(responses.iter().map(participant_change_to_result).collect())
            }),
            output: ReturnAdapter::Direct,
        },
    ]
}
