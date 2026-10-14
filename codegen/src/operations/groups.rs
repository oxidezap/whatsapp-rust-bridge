use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(/// Get metadata for a group.
    #[wasm_bindgen(js_name = getGroupMetadata)])),
            signature: parse_quote!(async fn group_metadata(
        &self,
        jid: &str,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().fetch_metadata(&group_jid)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(metadata),
                expression: parse_quote!(to_ts(group_metadata_to_result(&metadata))),
            },
        },
        Operation {
            attributes: attributes(quote!(/// Update a group's subject (name).
    #[wasm_bindgen(js_name = groupUpdateSubject)])),
            signature: parse_quote!(async fn group_update_subject(
        &self,
        jid: &str,
        subject: &str,
    ) -> Result<(), crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;

                let group_subject = whatsapp_rust::features::GroupSubject::new(subject)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().set_subject(&group_jid, group_subject)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(
                quote!(/// Update a group's description. Pass null/undefined to remove.
    #[wasm_bindgen(js_name = groupUpdateDescription)]),
            ),
            signature: parse_quote!(async fn group_update_description(
        &self,
        jid: &str,
        description: Option<String>,
    ) -> Result<(), crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;

                let desc = description
                    .as_deref()
                    .map(whatsapp_rust::features::GroupDescription::new)
                    .transpose()?;
            }),
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
            attributes: attributes(quote!(/// Leave a group.
    #[wasm_bindgen(js_name = groupLeave)])),
            signature: parse_quote!(async fn group_leave(&self, jid: &str) -> Result<(), crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().leave(&group_jid)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(
                quote!(/// Set or clear the bot's per-group "member label" — the small tag rendered
    /// under the bot's display name inside that group's UI. Empty `label`
    /// clears the label. The core sends this as a `ProtocolMessage` over the
    /// normal message path (not an IQ), matching WA Web's behavior.
    #[wasm_bindgen(js_name = updateMemberLabel)]),
            ),
            signature: parse_quote!(async fn update_member_label(
        &self,
        group_jid: &str,
        label: &str,
    ) -> Result<String, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let parsed = parse_jid(group_jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().update_member_label_with_id(&parsed, label)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(/// Get the invite link for a group.
    #[wasm_bindgen(js_name = groupInviteCode)])),
            signature: parse_quote!(async fn group_invite_code(&self, jid: &str) -> Result<String, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_invite_link(&group_jid, false)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(
                quote!(/// Set disappearing messages timer for a group (0 to disable).
    #[wasm_bindgen(js_name = groupToggleEphemeral)]),
            ),
            signature: parse_quote!(async fn group_toggle_ephemeral(
        &self,
        jid: &str,
        expiration: u32,
    ) -> Result<(), crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().set_ephemeral(&group_jid, expiration)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(/// Revoke a group's invite link (generates new one).
    #[wasm_bindgen(js_name = groupRevokeInvite)])),
            signature: parse_quote!(async fn group_revoke_invite(
        &self,
        jid: &str,
    ) -> Result<String, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_invite_link(&group_jid, true)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(new_code),
                expression: parse_quote!(Ok(new_code)),
            },
        },
        Operation {
            attributes: attributes(
                quote!(/// Deactivate a parent group without deleting its former subgroups.
    #[wasm_bindgen(js_name = deactivateCommunity)]),
            ),
            signature: parse_quote!(async fn deactivate_community(&self, jid: &str) -> Result<(), crate::errors::BridgeError>),
            prepare: parse_quote!({
                let target = parse_jid(jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.community().deactivate(target)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(/// Link existing groups to a parent group.
    #[wasm_bindgen(js_name = linkCommunitySubgroups)])),
            signature: parse_quote!(async fn link_community_subgroups(
        &self,
        parent_jid: &str,
        subgroup_jids: Vec<String>,
    ) -> Result<Ts<crate::result_types::CommunityLinkResult>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let parent = parse_jid(parent_jid)?;
                let subgroups = subgroup_jids
                    .iter()
                    .map(|subgroup| parse_jid(subgroup))
                    .collect::<Result<Vec<_>, _>>()?;
            }),
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
            attributes: attributes(quote!(/// Unlink groups from a parent group.
    #[wasm_bindgen(js_name = unlinkCommunitySubgroups)])),
            signature: parse_quote!(async fn unlink_community_subgroups(
        &self,
        parent_jid: &str,
        subgroup_jids: Vec<String>,
        remove_orphan_members: bool,
    ) -> Result<Ts<crate::result_types::CommunityLinkResult>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let parent = parse_jid(parent_jid)?;
                let subgroups = subgroup_jids
                    .iter()
                    .map(|subgroup| parse_jid(subgroup))
                    .collect::<Result<Vec<_>, _>>()?;
            }),
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
            attributes: attributes(quote!(/// Fetch subgroups of a parent group.
    #[wasm_bindgen(js_name = getCommunitySubgroups)])),
            signature: parse_quote!(async fn get_community_subgroups(
        &self,
        parent_jid: &str,
    ) -> Result<Vec<Ts<crate::result_types::CommunitySubgroupResult>>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let parent = parse_jid(parent_jid)?;
            }),
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
            attributes: attributes(quote!(/// Join a group using an invite code.
    #[wasm_bindgen(js_name = groupAcceptInvite)])),
            signature: parse_quote!(async fn group_accept_invite(
        &self,
        code: &str,
    ) -> Result<String, crate::errors::BridgeError>),
            prepare: parse_quote!({}),
            admission: Admission::Online,
            call: parse_quote!(client.groups().join_with_invite_code(code)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(jid),
                expression: parse_quote!(Ok(jid.group_jid().to_string())),
            },
        },
        Operation {
            attributes: attributes(
                quote!(/// Join a group via a GroupInviteMessage (V4 invite).
    #[wasm_bindgen(js_name = groupAcceptInviteV4)]),
            ),
            signature: parse_quote!(async fn group_accept_invite_v4(
        &self,
        group_jid: &str,
        code: &str,
        expiration: f64,
        admin_jid: &str,
    ) -> Result<String, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group = parse_jid(group_jid)?;
                let admin = parse_jid(admin_jid)?;
            }),
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
            attributes: attributes(
                quote!(/// Revoke invitation codes previously issued to participants.
    #[wasm_bindgen(js_name = groupRevokeInviteV4)]),
            ),
            signature: parse_quote!(async fn group_revoke_invite_v4(
        &self,
        group_jid: &str,
        invited_jid: &str,
    ) -> Result<bool, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group = parse_jid(group_jid)?;
                let invited = parse_jid(invited_jid)?;
            }),
            admission: Admission::Online,
            call: parse_quote!(client.groups().revoke_request_code(&group, &[invited])),
            output: ReturnAdapter::Project {
                binding: parse_quote!(responses),
                expression: parse_quote!(Ok(responses.iter().all(|response| response.is_ok()))),
            },
        },
        Operation {
            attributes: attributes(
                quote!(/// Get group info from an invite code (without joining).
    /// Returns the same shape as groupMetadata.
    #[wasm_bindgen(js_name = groupGetInviteInfo)]),
            ),
            signature: parse_quote!(async fn group_get_invite_info(
        &self,
        code: &str,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError>),
            prepare: parse_quote!({}),
            admission: Admission::Online,
            call: parse_quote!(client.groups().get_invite_info(code)),
            output: ReturnAdapter::Project {
                binding: parse_quote!(metadata),
                expression: parse_quote!(to_ts(group_metadata_to_result(&metadata))),
            },
        },
        Operation {
            attributes: attributes(quote!(/// Get list of pending join requests for a group.
    #[wasm_bindgen(js_name = groupRequestParticipantsList)])),
            signature: parse_quote!(async fn group_request_participants_list(
        &self,
        jid: &str,
    ) -> Result<Vec<Ts<crate::result_types::MembershipRequestResult>>, crate::errors::BridgeError>),
            prepare: parse_quote!({
                let group_jid = parse_jid(jid)?;
            }),
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
    ]
}
