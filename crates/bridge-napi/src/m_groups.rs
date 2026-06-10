//! Thin napi forwards for the group methods on `WhatsAppClientCore`. The binding
//! only converts at the boundary (`Uint8Array` ↔ bytes, `serde_json::Value`
//! enum inputs, `BridgeValue` → JS via `JsBridgeValue`, `BridgeError` → napi
//! exception); all logic lives in `bridge-core`.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::WasmWhatsAppClient;
use crate::methods::napi_err;
use crate::value_js::JsBridgeValue;

#[napi]
impl WasmWhatsAppClient {
    #[napi(js_name = "createGroup")]
    pub async fn group_create(
        &self,
        subject: String,
        participants: Vec<String>,
    ) -> Result<JsBridgeValue> {
        self.core
            .group_create(&subject, participants)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "groupUpdateSubject")]
    pub async fn group_update_subject(&self, jid: String, subject: String) -> Result<()> {
        self.core
            .group_update_subject(&jid, &subject)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupUpdateDescription")]
    pub async fn group_update_description(
        &self,
        jid: String,
        description: Option<String>,
    ) -> Result<()> {
        self.core
            .group_update_description(&jid, description)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupLeave")]
    pub async fn group_leave(&self, jid: String) -> Result<()> {
        self.core.group_leave(&jid).await.map_err(napi_err)
    }

    #[napi(js_name = "updateMemberLabel")]
    pub async fn update_member_label(&self, group_jid: String, label: String) -> Result<()> {
        self.core
            .update_member_label(&group_jid, &label)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupParticipantsUpdate")]
    pub async fn group_participants_update(
        &self,
        jid: String,
        participants: Vec<String>,
        action: serde_json::Value,
    ) -> Result<JsBridgeValue> {
        self.core
            .group_participants_update(&jid, participants, action)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "groupFetchAllParticipating")]
    pub async fn group_fetch_all_participating(&self) -> Result<JsBridgeValue> {
        self.core
            .group_fetch_all_participating()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "groupInviteCode")]
    pub async fn group_invite_code(&self, jid: String) -> Result<String> {
        self.core.group_invite_code(&jid).await.map_err(napi_err)
    }

    #[napi(js_name = "groupSettingUpdate")]
    pub async fn group_setting_update(
        &self,
        jid: String,
        setting: serde_json::Value,
        value: bool,
    ) -> Result<()> {
        self.core
            .group_setting_update(&jid, setting, value)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupToggleEphemeral")]
    pub async fn group_toggle_ephemeral(&self, jid: String, expiration: u32) -> Result<()> {
        self.core
            .group_toggle_ephemeral(&jid, expiration)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupRevokeInvite")]
    pub async fn group_revoke_invite(&self, jid: String) -> Result<String> {
        self.core.group_revoke_invite(&jid).await.map_err(napi_err)
    }

    #[napi(js_name = "groupAcceptInvite")]
    pub async fn group_accept_invite(&self, code: String) -> Result<String> {
        self.core.group_accept_invite(&code).await.map_err(napi_err)
    }

    #[napi(js_name = "groupAcceptInviteV4")]
    pub async fn group_accept_invite_v4(
        &self,
        group_jid: String,
        code: String,
        expiration: f64,
        admin_jid: String,
    ) -> Result<String> {
        self.core
            .group_accept_invite_v4(&group_jid, &code, expiration, &admin_jid)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupGetInviteInfo")]
    pub async fn group_get_invite_info(&self, code: String) -> Result<JsBridgeValue> {
        self.core
            .group_get_invite_info(&code)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "groupRequestParticipantsList")]
    pub async fn group_request_participants_list(&self, jid: String) -> Result<JsBridgeValue> {
        self.core
            .group_request_participants_list(&jid)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "groupRequestParticipantsUpdate")]
    pub async fn group_request_participants_update(
        &self,
        jid: String,
        participants: Vec<String>,
        action: serde_json::Value,
    ) -> Result<()> {
        self.core
            .group_request_participants_update(&jid, participants, action)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "groupMemberAddMode")]
    pub async fn group_member_add_mode(&self, jid: String, mode: serde_json::Value) -> Result<()> {
        self.core
            .group_member_add_mode(&jid, mode)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "setGroupProfilePicture")]
    pub async fn set_group_profile_picture(
        &self,
        group_jid: String,
        img_data: Uint8Array,
    ) -> Result<JsBridgeValue> {
        self.core
            .set_group_profile_picture(&group_jid, img_data.to_vec())
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "removeGroupProfilePicture")]
    pub async fn remove_group_profile_picture(&self, group_jid: String) -> Result<JsBridgeValue> {
        self.core
            .remove_group_profile_picture(&group_jid)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }
}
