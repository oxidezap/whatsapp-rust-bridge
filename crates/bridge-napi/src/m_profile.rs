//! Thin napi forwards for the profile / contacts / chat-actions / privacy /
//! presence batch. Boundary conversion only (`Uint8Array` ↔ bytes,
//! `serde_json::Value` for the enum/struct inputs, `BridgeValue` → JS via
//! `JsBridgeValue`, `BridgeError` → napi exception); all logic is in `bridge-core`.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::WasmWhatsAppClient;
use crate::methods::napi_err;
use crate::value_js::JsBridgeValue;

#[napi]
impl WasmWhatsAppClient {
    // ── Device / client profile ──────────────────────────────────────────────

    #[napi(js_name = "setDeviceProps")]
    pub async fn set_device_props(&self, input: serde_json::Value) -> Result<()> {
        self.core.set_device_props(input).await.map_err(napi_err)
    }

    #[napi(js_name = "setClientProfile")]
    pub async fn set_client_profile(&self, input: serde_json::Value) -> Result<()> {
        self.core.set_client_profile(input).await.map_err(napi_err)
    }

    // ── Profile picture / status ─────────────────────────────────────────────

    #[napi(js_name = "updateProfilePicture")]
    pub async fn update_profile_picture(&self, img_data: Uint8Array) -> Result<JsBridgeValue> {
        self.core
            .update_profile_picture(&img_data)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "removeProfilePicture")]
    pub async fn remove_profile_picture(&self) -> Result<JsBridgeValue> {
        self.core
            .remove_profile_picture()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "updateProfileStatus")]
    pub async fn update_profile_status(&self, status: String) -> Result<()> {
        self.core
            .update_profile_status(&status)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "profilePictureUrl")]
    pub async fn profile_picture_url(
        &self,
        jid: String,
        picture_type: serde_json::Value,
    ) -> Result<JsBridgeValue> {
        self.core
            .profile_picture_url(&jid, picture_type)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── Business profile ──────────────────────────────────────────────────────

    #[napi(js_name = "getBusinessProfile")]
    pub async fn get_business_profile(&self, jid: String) -> Result<JsBridgeValue> {
        self.core
            .get_business_profile(&jid)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── User info / status / on-WhatsApp ──────────────────────────────────────

    // The `fetchStatus` / `fetchUserInfo` / `isOnWhatsApp` core futures await
    // `get_user_info`/`is_on_whatsapp`, which feed a generic `persist_lid_mappings<'b>`
    // closure. Requiring those futures to be `Send` (as the napi macro's
    // generated `Send + 'static` wrapper — and `tokio::spawn` — do) trips a
    // rustc higher-ranked-lifetime inference bug ("FnOnce is not general
    // enough"). The old WASM path avoided it by driving everything on a
    // non-`Send` `spawn_local` executor.
    //
    // To reproduce that here without a `Send` bound on the inner future, drive
    // the core future on the CURRENT multi-threaded runtime via
    // `Handle::block_on` from a `spawn_blocking` thread: `block_on` polls the
    // future on the blocking thread (so it is never required to be `Send`),
    // while the main runtime's IO/timer drivers and spawned tasks keep working.
    // Only the owned `BridgeValue` result (which is `Send`) crosses back.
    #[napi(js_name = "fetchStatus")]
    pub async fn fetch_status(&self, jids: Vec<String>) -> Result<JsBridgeValue> {
        let core = self.core.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || handle.block_on(core.fetch_status(jids)))
            .await
            .map_err(napi_err)?
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "fetchUserInfo")]
    pub async fn fetch_user_info(&self, jids: Vec<String>) -> Result<JsBridgeValue> {
        let core = self.core.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || handle.block_on(core.fetch_user_info(jids)))
            .await
            .map_err(napi_err)?
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "isOnWhatsApp")]
    pub async fn is_on_whatsapp(&self, phones: Vec<String>) -> Result<JsBridgeValue> {
        let core = self.core.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || handle.block_on(core.is_on_whatsapp(phones)))
            .await
            .map_err(napi_err)?
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── Blocking ──────────────────────────────────────────────────────────────

    #[napi(js_name = "updateBlockStatus")]
    pub async fn update_block_status(&self, jid: String, action: serde_json::Value) -> Result<()> {
        self.core
            .update_block_status(&jid, action)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "fetchBlocklist")]
    pub async fn fetch_blocklist(&self) -> Result<JsBridgeValue> {
        self.core
            .fetch_blocklist()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── Privacy / disappearing mode ─────────────────────────────────────────────

    #[napi(js_name = "fetchPrivacySettings")]
    pub async fn fetch_privacy_settings(&self) -> Result<JsBridgeValue> {
        self.core
            .fetch_privacy_settings()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "updatePrivacySetting")]
    pub async fn update_privacy_setting(&self, category: String, value: String) -> Result<()> {
        self.core
            .update_privacy_setting(&category, &value)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "updateDefaultDisappearingMode")]
    pub async fn update_default_disappearing_mode(&self, duration: u32) -> Result<()> {
        self.core
            .update_default_disappearing_mode(duration)
            .await
            .map_err(napi_err)
    }

    // ── Chat actions ────────────────────────────────────────────────────────────

    #[napi(js_name = "pinChat")]
    pub async fn pin_chat(&self, jid: String, pin: bool) -> Result<()> {
        self.core.pin_chat(&jid, pin).await.map_err(napi_err)
    }

    #[napi(js_name = "muteChat")]
    pub async fn mute_chat(&self, jid: String, mute_until: Option<f64>) -> Result<()> {
        self.core
            .mute_chat(&jid, mute_until)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "archiveChat")]
    pub async fn archive_chat(&self, jid: String, archive: bool) -> Result<()> {
        self.core
            .archive_chat(&jid, archive)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "starMessage")]
    pub async fn star_message(&self, jid: String, message_id: String, star: bool) -> Result<()> {
        self.core
            .star_message(&jid, &message_id, star)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "markChatAsRead")]
    pub async fn mark_chat_as_read(&self, jid: String, read: bool) -> Result<()> {
        self.core
            .mark_chat_as_read(&jid, read)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "deleteChat")]
    pub async fn delete_chat(&self, jid: String) -> Result<()> {
        self.core.delete_chat(&jid).await.map_err(napi_err)
    }

    // ── Calls ───────────────────────────────────────────────────────────────────

    #[napi(js_name = "rejectCall")]
    pub async fn reject_call(&self, call_id: String, call_from: String) -> Result<()> {
        self.core
            .reject_call(&call_id, &call_from)
            .await
            .map_err(napi_err)
    }

    // ── Presence / chat state ─────────────────────────────────────────────────────

    #[napi(js_name = "sendPresence")]
    pub async fn send_presence(&self, status: serde_json::Value) -> Result<()> {
        self.core.send_presence(status).await.map_err(napi_err)
    }

    #[napi(js_name = "presenceSubscribe")]
    pub async fn presence_subscribe(&self, jid: String) -> Result<()> {
        self.core.presence_subscribe(&jid).await.map_err(napi_err)
    }

    #[napi(js_name = "sendChatState")]
    pub async fn send_chat_state(&self, jid: String, state: serde_json::Value) -> Result<()> {
        self.core
            .send_chat_state(&jid, state)
            .await
            .map_err(napi_err)
    }

    // ── Reachout timelock ────────────────────────────────────────────────────────

    #[napi(js_name = "fetchReachoutTimelock")]
    pub async fn fetch_reachout_timelock(&self) -> Result<JsBridgeValue> {
        self.core
            .fetch_reachout_timelock()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── Message history ───────────────────────────────────────────────────────────

    #[napi(js_name = "fetchMessageHistory")]
    pub async fn fetch_message_history(
        &self,
        count: i32,
        chat_jid: String,
        oldest_msg_id: String,
        oldest_msg_from_me: bool,
        oldest_msg_timestamp_ms: f64,
    ) -> Result<String> {
        self.core
            .fetch_message_history(
                count,
                &chat_jid,
                &oldest_msg_id,
                oldest_msg_from_me,
                oldest_msg_timestamp_ms,
            )
            .await
            .map_err(napi_err)
    }
}
