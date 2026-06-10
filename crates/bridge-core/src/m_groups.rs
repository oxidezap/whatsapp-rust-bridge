//! Group methods — engine-agnostic ports of the old `wasm_client.rs`
//! `WasmWhatsAppClient` group APIs (create/update/leave/participants/invite/
//! settings/membership/profile-picture). Logic is reproduced 1:1; only the
//! JS-interop boundary changes (the napi binding forwards primitives and
//! materializes `BridgeValue`).

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::methods::snake;
use crate::value::BridgeValue;
use crate::{helpers, result_types};

use wacore_binary::jid::Jid;

impl WhatsAppClientCore {
    pub async fn group_create(
        &self,
        subject: &str,
        participants: Vec<String>,
    ) -> Result<BridgeValue, BridgeError> {
        use whatsapp_rust::features::GroupParticipantOptions;

        let participant_options: Vec<GroupParticipantOptions> = participants
            .iter()
            .map(|p| helpers::parse_jid(p).map(GroupParticipantOptions::new))
            .collect::<Result<_, _>>()?;

        let options = whatsapp_rust::features::GroupCreateOptions::new(subject)
            .with_participants(participant_options);

        let result = self.client.groups().create_group(options).await?;

        snake(&result_types::group_metadata_to_result(&result.metadata))
    }

    pub async fn group_update_subject(&self, jid: &str, subject: &str) -> Result<(), BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;

        let group_subject = whatsapp_rust::features::GroupSubject::new(subject)?;

        self.client
            .groups()
            .set_subject(&group_jid, group_subject)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn group_update_description(
        &self,
        jid: &str,
        description: Option<String>,
    ) -> Result<(), BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;

        let desc = description
            .as_deref()
            .map(whatsapp_rust::features::GroupDescription::new)
            .transpose()?;

        self.client
            .groups()
            .set_description(&group_jid, desc, None)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn group_leave(&self, jid: &str) -> Result<(), BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;

        self.client
            .groups()
            .leave(&group_jid)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn update_member_label(
        &self,
        group_jid: &str,
        label: &str,
    ) -> Result<(), BridgeError> {
        let parsed = helpers::parse_jid(group_jid)?;
        self.client
            .groups()
            .update_member_label(&parsed, label)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn group_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        action: serde_json::Value,
    ) -> Result<BridgeValue, BridgeError> {
        use result_types::GroupParticipantAction;

        let action: GroupParticipantAction = serde_json::from_value(action)
            .map_err(|e| errors::invalid_arg("action", e.to_string()))?;
        let group_jid = helpers::parse_jid(jid)?;

        let participant_jids: Vec<Jid> = participants
            .iter()
            .map(|p| helpers::parse_jid(p))
            .collect::<Result<_, _>>()?;

        let to_results =
            |responses: Vec<whatsapp_rust::features::ParticipantChangeResponse>| -> Vec<result_types::ParticipantChangeResult> {
                responses
                    .iter()
                    .map(|r| result_types::ParticipantChangeResult {
                        jid: r.jid.to_string(),
                        status: r.status.clone(),
                        error: r.error.clone(),
                    })
                    .collect()
            };

        let results = match action {
            GroupParticipantAction::Add => {
                let result = self
                    .client
                    .groups()
                    .add_participants(&group_jid, &participant_jids)
                    .await?;
                to_results(result)
            }
            GroupParticipantAction::Remove => {
                let result = self
                    .client
                    .groups()
                    .remove_participants(&group_jid, &participant_jids)
                    .await?;
                to_results(result)
            }
            GroupParticipantAction::Promote => {
                self.client
                    .groups()
                    .promote_participants(&group_jid, &participant_jids)
                    .await?;
                Vec::new()
            }
            GroupParticipantAction::Demote => {
                self.client
                    .groups()
                    .demote_participants(&group_jid, &participant_jids)
                    .await?;
                Vec::new()
            }
        };

        snake(&results)
    }

    pub async fn group_fetch_all_participating(&self) -> Result<BridgeValue, BridgeError> {
        let groups = self.client.groups().get_participating().await?;

        let mut fields: Vec<(String, BridgeValue)> = Vec::with_capacity(groups.len());
        for (key, metadata) in &groups {
            let result = result_types::group_metadata_to_result(metadata);
            fields.push((key.to_string(), snake(&result)?));
        }
        Ok(BridgeValue::Object(fields))
    }

    pub async fn group_invite_code(&self, jid: &str) -> Result<String, BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;

        self.client
            .groups()
            .get_invite_link(&group_jid, false)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn group_setting_update(
        &self,
        jid: &str,
        setting: serde_json::Value,
        value: bool,
    ) -> Result<(), BridgeError> {
        use result_types::GroupSetting;

        let setting: GroupSetting = serde_json::from_value(setting)
            .map_err(|e| errors::invalid_arg("setting", e.to_string()))?;
        let group_jid = helpers::parse_jid(jid)?;

        match setting {
            GroupSetting::Locked => self.client.groups().set_locked(&group_jid, value).await?,
            GroupSetting::Announce => self.client.groups().set_announce(&group_jid, value).await?,
            GroupSetting::MembershipApproval => {
                let mode = if value {
                    whatsapp_rust::MembershipApprovalMode::On
                } else {
                    whatsapp_rust::MembershipApprovalMode::Off
                };
                self.client
                    .groups()
                    .set_membership_approval(&group_jid, mode)
                    .await?;
            }
        }

        Ok(())
    }

    pub async fn group_toggle_ephemeral(
        &self,
        jid: &str,
        expiration: u32,
    ) -> Result<(), BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;
        self.client
            .groups()
            .set_ephemeral(&group_jid, expiration)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn group_revoke_invite(&self, jid: &str) -> Result<String, BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;
        let new_code = self
            .client
            .groups()
            .get_invite_link(&group_jid, true)
            .await?;
        Ok(new_code)
    }

    pub async fn group_accept_invite(&self, code: &str) -> Result<String, BridgeError> {
        let jid = self.client.groups().join_with_invite_code(code).await?;
        Ok(jid.group_jid().to_string())
    }

    pub async fn group_accept_invite_v4(
        &self,
        group_jid: &str,
        code: &str,
        expiration: f64,
        admin_jid: &str,
    ) -> Result<String, BridgeError> {
        let group = helpers::parse_jid(group_jid)?;
        let admin = helpers::parse_jid(admin_jid)?;
        let result = self
            .client
            .groups()
            .join_with_invite_v4(&group, code, expiration as i64, &admin)
            .await?;
        Ok(result.group_jid().to_string())
    }

    pub async fn group_get_invite_info(&self, code: &str) -> Result<BridgeValue, BridgeError> {
        let metadata = self.client.groups().get_invite_info(code).await?;
        snake(&result_types::group_metadata_to_result(&metadata))
    }

    pub async fn group_request_participants_list(
        &self,
        jid: &str,
    ) -> Result<BridgeValue, BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;
        let list = self
            .client
            .groups()
            .get_membership_requests(&group_jid)
            .await?;
        let results: Vec<result_types::MembershipRequestResult> = list
            .iter()
            .map(|r| result_types::MembershipRequestResult {
                jid: r.jid.to_string(),
                request_time: r.request_time.map(|t| t as f64),
            })
            .collect();
        snake(&results)
    }

    pub async fn group_request_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        action: serde_json::Value,
    ) -> Result<(), BridgeError> {
        use result_types::GroupRequestAction;

        let action: GroupRequestAction = serde_json::from_value(action)
            .map_err(|e| errors::invalid_arg("action", e.to_string()))?;
        let group_jid = helpers::parse_jid(jid)?;
        let participant_jids: Vec<Jid> = participants
            .iter()
            .map(|s| helpers::parse_jid(s))
            .collect::<Result<Vec<_>, _>>()?;

        match action {
            GroupRequestAction::Approve => {
                self.client
                    .groups()
                    .approve_membership_requests(&group_jid, &participant_jids)
                    .await?;
            }
            GroupRequestAction::Reject => {
                self.client
                    .groups()
                    .reject_membership_requests(&group_jid, &participant_jids)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn group_member_add_mode(
        &self,
        jid: &str,
        mode: serde_json::Value,
    ) -> Result<(), BridgeError> {
        use result_types::MemberAddMode;

        let mode: MemberAddMode =
            serde_json::from_value(mode).map_err(|e| errors::invalid_arg("mode", e.to_string()))?;
        let group_jid = helpers::parse_jid(jid)?;
        let add_mode = match mode {
            MemberAddMode::AdminAdd => whatsapp_rust::features::MemberAddMode::AdminAdd,
            MemberAddMode::AllMemberAdd => whatsapp_rust::features::MemberAddMode::AllMemberAdd,
        };
        self.client
            .groups()
            .set_member_add_mode(&group_jid, add_mode)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn set_group_profile_picture(
        &self,
        group_jid: &str,
        img_data: Vec<u8>,
    ) -> Result<BridgeValue, BridgeError> {
        use wacore_binary::JidExt;
        let target = helpers::parse_jid(group_jid)?;
        if !target.is_group() {
            return Err(errors::internal(
                "setGroupProfilePicture: target jid must be a group jid",
            ));
        }
        let result = self
            .client
            .execute(wacore::iq::contacts::SetProfilePictureSpec::set_group(
                &target, img_data,
            ))
            .await?;
        snake(&result_types::ProfilePictureResult { id: result.id })
    }

    pub async fn remove_group_profile_picture(
        &self,
        group_jid: &str,
    ) -> Result<BridgeValue, BridgeError> {
        use wacore_binary::JidExt;
        let target = helpers::parse_jid(group_jid)?;
        if !target.is_group() {
            return Err(errors::internal(
                "removeGroupProfilePicture: target jid must be a group jid",
            ));
        }
        let result = self
            .client
            .execute(wacore::iq::contacts::SetProfilePictureSpec::remove_group(
                &target,
            ))
            .await?;
        snake(&result_types::ProfilePictureResult { id: result.id })
    }
}
