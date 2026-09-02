//! Groups, parent groups and invites.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Groups ───────────────────────────────────────────────────────────

    /// Get metadata for a group.
    #[wasm_bindgen(js_name = getGroupMetadata)]
    pub async fn group_metadata(
        &self,
        jid: &str,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        let metadata = self
            .client
            .online()
            .await?
            .groups()
            .get_metadata(&group_jid)
            .await?;

        to_ts(group_metadata_to_result(&metadata))
    }

    /// Create a new group.
    ///
    /// Returns the full `GroupMetadataResult` parsed directly from the
    /// server's create response — no follow-up `getGroupMetadata` IQ
    /// needed. Mirrors the reference client: the create reply
    /// already carries the complete `<group>` node (id, subject,
    /// creation, creator, participants, …).
    #[wasm_bindgen(js_name = createGroup)]
    pub async fn group_create(
        &self,
        subject: &str,
        participants: Vec<String>,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError> {
        use whatsapp_rust::features::GroupParticipantOptions;

        let participant_options: Vec<GroupParticipantOptions> = participants
            .iter()
            .map(|p| parse_jid(p).map(GroupParticipantOptions::new))
            .collect::<Result<_, _>>()?;

        let options = whatsapp_rust::features::GroupCreateOptions::new(subject)
            .with_participants(participant_options);

        let result = self
            .client
            .online()
            .await?
            .groups()
            .create_group(options)
            .await?;

        to_ts(group_metadata_to_result(&result.metadata))
    }

    /// Update a group's subject (name).
    #[wasm_bindgen(js_name = groupUpdateSubject)]
    pub async fn group_update_subject(
        &self,
        jid: &str,
        subject: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        let group_subject = whatsapp_rust::features::GroupSubject::new(subject)?;
        self.client
            .online()
            .await?
            .groups()
            .set_subject(&group_jid, group_subject)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Update a group's description. Pass null/undefined to remove.
    #[wasm_bindgen(js_name = groupUpdateDescription)]
    pub async fn group_update_description(
        &self,
        jid: &str,
        description: Option<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        let desc = description
            .as_deref()
            .map(whatsapp_rust::features::GroupDescription::new)
            .transpose()?;
        self.client
            .online()
            .await?
            .groups()
            // The caller holds no description id, so the core reads the current
            // one before sending: without that token the server rejects every
            // update of a group that already has a description.
            .set_description(
                &group_jid,
                desc,
                whatsapp_rust::features::PreviousDescription::Resolve,
            )
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Leave a group.
    #[wasm_bindgen(js_name = groupLeave)]
    pub async fn group_leave(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        self.client
            .online()
            .await?
            .groups()
            .leave(&group_jid)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Set or clear the bot's per-group "member label" — the small tag rendered
    /// under the bot's display name inside that group's UI. Empty `label`
    /// clears the label. The core sends this as a `ProtocolMessage` over the
    /// normal message path (not an IQ), matching WA Web's behavior.
    #[wasm_bindgen(js_name = updateMemberLabel)]
    pub async fn update_member_label(
        &self,
        group_jid: &str,
        label: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let parsed = parse_jid(group_jid)?;
        self.client
            .online()
            .await?
            .groups()
            .update_member_label_with_id(&parsed, label)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Update group participants.
    #[wasm_bindgen(js_name = groupParticipantsUpdate)]
    pub async fn group_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        #[wasm_bindgen(unchecked_param_type = "GroupParticipantAction")] action: JsValue,
    ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError>
    {
        let action =
            from_js_input::<crate::result_types::GroupParticipantAction>("action", action)?;
        let (group_jid, participant_jids) = participants_update_input(jid, &participants, action)?;
        participants_update(
            self.client.online().await?,
            group_jid,
            participant_jids,
            action,
            false,
        )
        .await
    }

    /// Fetch all groups the user is participating in.
    #[wasm_bindgen(js_name = groupFetchAllParticipating, skip_typescript)]
    pub async fn group_fetch_all_participating(
        &self,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let groups = self
            .client
            .online()
            .await?
            .groups()
            .get_participating()
            .await?;

        let obj = js_sys::Object::new();
        for (key, metadata) in &groups {
            let result = group_metadata_to_result(metadata);
            let js_metadata = serde_wasm_bindgen::to_value(&result)?;
            // #767: get_participating now keys by Jid (was String) — stringify for the JS object key.
            js_sys::Reflect::set(&obj, &JsValue::from_str(&key.to_string()), &js_metadata)?;
        }
        Ok(obj.into())
    }

    /// Get the invite link for a group.
    #[wasm_bindgen(js_name = groupInviteCode)]
    pub async fn group_invite_code(&self, jid: &str) -> Result<String, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        self.client
            .online()
            .await?
            .groups()
            .get_invite_link(&group_jid, false)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Update a group setting (locked, announce, membership_approval).
    #[wasm_bindgen(js_name = groupSettingUpdate)]
    pub async fn group_setting_update(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "GroupSetting")] setting: JsValue,
        value: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let setting = from_js_input::<crate::result_types::GroupSetting>("setting", setting)?;
        use crate::result_types::GroupSetting;
        let group_jid = parse_jid(jid)?;

        match setting {
            GroupSetting::Locked => {
                self.client
                    .online()
                    .await?
                    .groups()
                    .set_locked(&group_jid, value)
                    .await?
            }
            GroupSetting::Announce => {
                self.client
                    .online()
                    .await?
                    .groups()
                    .set_announce(&group_jid, value)
                    .await?
            }
            GroupSetting::MembershipApproval => {
                let mode = if value {
                    whatsapp_rust::MembershipApprovalMode::On
                } else {
                    whatsapp_rust::MembershipApprovalMode::Off
                };
                self.client
                    .online()
                    .await?
                    .groups()
                    .set_membership_approval(&group_jid, mode)
                    .await?;
            }
        }

        Ok(())
    }

    /// Set disappearing messages timer for a group (0 to disable).
    #[wasm_bindgen(js_name = groupToggleEphemeral)]
    pub async fn group_toggle_ephemeral(
        &self,
        jid: &str,
        expiration: u32,
    ) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        self.client
            .online()
            .await?
            .groups()
            .set_ephemeral(&group_jid, expiration)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Revoke a group's invite link (generates new one).
    #[wasm_bindgen(js_name = groupRevokeInvite)]
    pub async fn group_revoke_invite(
        &self,
        jid: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        let new_code = self
            .client
            .online()
            .await?
            .groups()
            .get_invite_link(&group_jid, true)
            .await?;
        Ok(new_code)
    }

    // ── Parent groups ───────────────────────────────────────────────────

    /// Create a parent group with explicit protocol options.
    #[wasm_bindgen(js_name = createCommunity)]
    pub async fn create_community(
        &self,
        name: &str,
        description: Option<String>,
        closed: bool,
        allow_non_admin_sub_group_creation: bool,
        create_general_chat: bool,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError> {
        let mut options = whatsapp_rust::features::CreateCommunityOptions::new(name);
        options.description = description;
        options.closed = closed;
        options.allow_non_admin_sub_group_creation = allow_non_admin_sub_group_creation;
        options.create_general_chat = create_general_chat;
        let result = self
            .client
            .online()
            .await?
            .community()
            .create(options)
            .await?;
        to_ts(group_metadata_to_result(&result.metadata))
    }

    /// Create a subgroup already linked to a parent group.
    #[wasm_bindgen(js_name = createCommunitySubgroup)]
    pub async fn create_community_subgroup(
        &self,
        name: &str,
        participants: Vec<String>,
        parent_jid: &str,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError> {
        let participants = participants
            .iter()
            .map(|participant| parse_jid(participant))
            .collect::<Result<Vec<_>, _>>()?;
        let parent = parse_jid(parent_jid)?;
        let result = self
            .client
            .online()
            .await?
            .community()
            .create_subgroup(name, &participants, parent)
            .await?;
        to_ts(group_metadata_to_result(&result.metadata))
    }

    /// Deactivate a parent group without deleting its former subgroups.
    #[wasm_bindgen(js_name = deactivateCommunity)]
    pub async fn deactivate_community(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .online()
            .await?
            .community()
            .deactivate(target)
            .await?;
        Ok(())
    }

    /// Link existing groups to a parent group.
    #[wasm_bindgen(js_name = linkCommunitySubgroups)]
    pub async fn link_community_subgroups(
        &self,
        parent_jid: &str,
        subgroup_jids: Vec<String>,
    ) -> Result<Ts<crate::result_types::CommunityLinkResult>, crate::errors::BridgeError> {
        let parent = parse_jid(parent_jid)?;
        let subgroups = subgroup_jids
            .iter()
            .map(|subgroup| parse_jid(subgroup))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .client
            .online()
            .await?
            .community()
            .link_subgroups(parent, &subgroups)
            .await?;
        to_ts(community_link_result(
            result.linked_jids,
            result.failed_groups,
        ))
    }

    /// Unlink groups from a parent group.
    #[wasm_bindgen(js_name = unlinkCommunitySubgroups)]
    pub async fn unlink_community_subgroups(
        &self,
        parent_jid: &str,
        subgroup_jids: Vec<String>,
        remove_orphan_members: bool,
    ) -> Result<Ts<crate::result_types::CommunityLinkResult>, crate::errors::BridgeError> {
        let parent = parse_jid(parent_jid)?;
        let subgroups = subgroup_jids
            .iter()
            .map(|subgroup| parse_jid(subgroup))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .client
            .online()
            .await?
            .community()
            .unlink_subgroups(parent, &subgroups, remove_orphan_members)
            .await?;
        to_ts(community_link_result(
            result.unlinked_jids,
            result.failed_groups,
        ))
    }

    /// Fetch subgroups of a parent group.
    #[wasm_bindgen(js_name = getCommunitySubgroups)]
    pub async fn get_community_subgroups(
        &self,
        parent_jid: &str,
    ) -> Result<Vec<Ts<crate::result_types::CommunitySubgroupResult>>, crate::errors::BridgeError>
    {
        let parent = parse_jid(parent_jid)?;
        let groups = self
            .client
            .online()
            .await?
            .community()
            .get_subgroups(&parent)
            .await?;
        to_ts_vec(
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
        )
    }

    /// Fetch all parent groups the account currently participates in.
    #[wasm_bindgen(js_name = communityFetchAllParticipating, skip_typescript)]
    pub async fn community_fetch_all_participating(
        &self,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let communities = self
            .client
            .online()
            .await?
            .community()
            .get_participating()
            .await?;
        let result = js_sys::Object::new();
        for (jid, metadata) in &communities {
            let value = serde_wasm_bindgen::to_value(&group_metadata_to_result(metadata))?;
            js_sys::Reflect::set(&result, &jid.to_string().into(), &value)?;
        }
        Ok(result.into())
    }

    /// Update participants of a parent group. Removing a participant also
    /// removes them from its linked groups.
    #[wasm_bindgen(js_name = communityParticipantsUpdate)]
    pub async fn community_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        #[wasm_bindgen(unchecked_param_type = "GroupParticipantAction")] action: JsValue,
    ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError>
    {
        let action =
            from_js_input::<crate::result_types::GroupParticipantAction>("action", action)?;
        let (group_jid, participant_jids) = participants_update_input(jid, &participants, action)?;
        participants_update(
            self.client.online().await?,
            group_jid,
            participant_jids,
            action,
            true,
        )
        .await
    }

    // ── Group invite ────────────────────────────────────────────────────

    /// Join a group using an invite code.
    #[wasm_bindgen(js_name = groupAcceptInvite)]
    pub async fn group_accept_invite(
        &self,
        code: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let jid = self
            .client
            .online()
            .await?
            .groups()
            .join_with_invite_code(code)
            .await?;
        Ok(jid.group_jid().to_string())
    }

    /// Join a group via a GroupInviteMessage (V4 invite).
    #[wasm_bindgen(js_name = groupAcceptInviteV4)]
    pub async fn group_accept_invite_v4(
        &self,
        group_jid: &str,
        code: &str,
        expiration: f64,
        admin_jid: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let admin = parse_jid(admin_jid)?;
        let result = self
            .client
            .online()
            .await?
            .groups()
            .join_with_invite_v4(&group, code, expiration as i64, &admin)
            .await?;
        Ok(result.group_jid().to_string())
    }

    /// Revoke invitation codes previously issued to participants.
    #[wasm_bindgen(js_name = groupRevokeInviteV4)]
    pub async fn group_revoke_invite_v4(
        &self,
        group_jid: &str,
        invited_jid: &str,
    ) -> Result<bool, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let invited = parse_jid(invited_jid)?;
        let responses = self
            .client
            .online()
            .await?
            .groups()
            .revoke_request_code(&group, &[invited])
            .await?;
        Ok(responses.iter().all(|response| response.is_ok()))
    }

    /// Get group info from an invite code (without joining).
    /// Returns the same shape as groupMetadata.
    #[wasm_bindgen(js_name = groupGetInviteInfo)]
    pub async fn group_get_invite_info(
        &self,
        code: &str,
    ) -> Result<Ts<crate::result_types::GroupMetadataResult>, crate::errors::BridgeError> {
        let metadata = self
            .client
            .online()
            .await?
            .groups()
            .get_invite_info(code)
            .await?;
        to_ts(group_metadata_to_result(&metadata))
    }

    /// Get list of pending join requests for a group.
    #[wasm_bindgen(js_name = groupRequestParticipantsList)]
    pub async fn group_request_participants_list(
        &self,
        jid: &str,
    ) -> Result<Vec<Ts<crate::result_types::MembershipRequestResult>>, crate::errors::BridgeError>
    {
        let group_jid = parse_jid(jid)?;
        let list = self
            .client
            .online()
            .await?
            .groups()
            .get_membership_requests(&group_jid)
            .await?;
        to_ts_vec(
            list.iter()
                .map(|r| crate::result_types::MembershipRequestResult {
                    jid: r.jid.to_string(),
                    request_time: r.request_time.map(|t| t as f64),
                })
                .collect(),
        )
    }

    /// Approve or reject pending join requests.
    #[wasm_bindgen(js_name = groupRequestParticipantsUpdate)]
    pub async fn group_request_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        #[wasm_bindgen(unchecked_param_type = "GroupRequestAction")] action: JsValue,
    ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError>
    {
        let action = from_js_input::<crate::result_types::GroupRequestAction>("action", action)?;
        use crate::result_types::GroupRequestAction;
        let group_jid = parse_jid(jid)?;
        let participant_jids: Vec<Jid> = participants
            .iter()
            .map(|s| parse_jid(s))
            .collect::<Result<Vec<_>, _>>()?;

        let responses = match action {
            GroupRequestAction::Approve => {
                self.client
                    .online()
                    .await?
                    .groups()
                    .approve_membership_requests(&group_jid, &participant_jids)
                    .await?
            }
            GroupRequestAction::Reject => {
                self.client
                    .online()
                    .await?
                    .groups()
                    .reject_membership_requests(&group_jid, &participant_jids)
                    .await?
            }
        };
        to_ts_vec(responses.iter().map(participant_change_to_result).collect())
    }

    // ── Group member add mode ────────────────────────────────────────────

    /// Set who can add members to a group.
    #[wasm_bindgen(js_name = groupMemberAddMode)]
    pub async fn group_member_add_mode(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "MemberAddMode")] mode: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let mode = from_js_input::<crate::result_types::MemberAddMode>("mode", mode)?;
        use crate::result_types::MemberAddMode;
        let group_jid = parse_jid(jid)?;
        let add_mode = match mode {
            MemberAddMode::AdminAdd => whatsapp_rust::features::MemberAddMode::AdminAdd,
            MemberAddMode::AllMemberAdd => whatsapp_rust::features::MemberAddMode::AllMemberAdd,
        };
        self.client
            .online()
            .await?
            .groups()
            .set_member_add_mode(&group_jid, add_mode)
            .await
            .map_err(crate::errors::BridgeError::from)
    }
}
