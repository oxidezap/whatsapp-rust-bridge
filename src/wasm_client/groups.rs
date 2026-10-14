//! Groups, parent groups and invites.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Groups ───────────────────────────────────────────────────────────

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
            .list_participating()
            .await?;

        let obj = js_sys::Object::new();
        for group in &groups {
            let result = group_overview_to_result(group);
            let js_metadata = serde_wasm_bindgen::to_value(&result)?;
            js_sys::Reflect::set(&obj, &JsValue::from_str(&result.id), &js_metadata)?;
        }
        Ok(obj.into())
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
        let mut options = whatsapp_rust::features::CreateCommunityOptions::new(name)
            .map_err(|e| crate::errors::invalid_arg("name", e.to_string()))?;
        options.description = description
            .map(whatsapp_rust::features::GroupDescription::new)
            .transpose()
            .map_err(|e| crate::errors::invalid_arg("description", e.to_string()))?;
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
            .list_participating()
            .await?;
        let result = js_sys::Object::new();
        for group in &communities {
            let value = serde_wasm_bindgen::to_value(&group_overview_to_result(group))?;
            js_sys::Reflect::set(&result, &group.id.to_string().into(), &value)?;
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

include!("generated/groups.rs");
