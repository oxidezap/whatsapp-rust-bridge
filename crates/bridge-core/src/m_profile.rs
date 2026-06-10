//! Profile / contacts / chat-actions / privacy / presence methods on
//! `WhatsAppClientCore`. Engine-agnostic ports of the old `wasm_client.rs`
//! `WasmWhatsAppClient` methods (same `js_name`s): device/client profile,
//! profile picture + status, business profile, user info / status / on-WA
//! checks, block list, privacy + disappearing mode, chat actions (pin/mute/
//! archive/star/read/delete), call rejection, presence, chat state, reachout
//! timelock, and on-demand history fetch.
//!
//! Conversion contract matches `methods.rs`: structured results → [`BridgeValue`]
//! (via `snake`, which preserves the result types' `rename_all = "camelCase"`);
//! scalars/strings/bytes pass through. Errors are [`BridgeError`].

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::methods::snake;
use crate::value::BridgeValue;
use crate::{helpers, result_types};

use wacore_binary::jid::Jid;

impl WhatsAppClientCore {
    // ── Device / client profile ──────────────────────────────────────────────

    /// Override `DeviceProps` before initial pairing (drives the phone's
    /// "Linked Devices" display). No-op on the wire for already-paired sessions.
    pub async fn set_device_props(&self, input: serde_json::Value) -> Result<(), BridgeError> {
        let props: crate::device_props::DevicePropsInput = serde_json::from_value(input)
            .map_err(|e| errors::invalid_arg("deviceProps", e.to_string()))?;
        self.client.set_device_props(props.into()).await;
        Ok(())
    }

    /// Override the noise-handshake `ClientPayload` profile (UserAgent +
    /// `web_info` presence). Runtime-only — re-apply each fresh process.
    pub async fn set_client_profile(&self, input: serde_json::Value) -> Result<(), BridgeError> {
        let profile: crate::client_profile::ClientProfileInput = serde_json::from_value(input)
            .map_err(|e| errors::invalid_arg("clientProfile", e.to_string()))?;
        self.client.set_client_profile(profile.into()).await;
        Ok(())
    }

    // ── Profile picture / status ─────────────────────────────────────────────

    /// Set the profile picture for the logged-in user.
    pub async fn update_profile_picture(
        &self,
        img_data: &[u8],
    ) -> Result<BridgeValue, BridgeError> {
        let result = self
            .client
            .profile()
            .set_profile_picture(img_data.to_vec())
            .await?;
        snake(&result_types::ProfilePictureResult { id: result.id })
    }

    /// Remove the profile picture for the logged-in user.
    pub async fn remove_profile_picture(&self) -> Result<BridgeValue, BridgeError> {
        let result = self.client.profile().remove_profile_picture().await?;
        snake(&result_types::ProfilePictureResult { id: result.id })
    }

    /// Update the user's status text (about).
    pub async fn update_profile_status(&self, status: &str) -> Result<(), BridgeError> {
        self.client
            .profile()
            .set_status_text(status)
            .await
            .map_err(BridgeError::from)
    }

    /// Get the profile picture URL for a user or group. `picture_type` is
    /// `"preview"` or `"image"`.
    pub async fn profile_picture_url(
        &self,
        jid: &str,
        picture_type: serde_json::Value,
    ) -> Result<BridgeValue, BridgeError> {
        use result_types::PictureType;
        let picture_type: PictureType = serde_json::from_value(picture_type)
            .map_err(|e| errors::invalid_arg("pictureType", e.to_string()))?;
        let target = helpers::parse_jid(jid)?;
        let preview = match picture_type {
            PictureType::Preview => true,
            PictureType::Image => false,
        };

        let result = self
            .client
            .contacts()
            .get_profile_picture(&target, preview)
            .await?;

        let info = result.map(|pic| result_types::ProfilePictureInfo {
            id: pic.id,
            url: pic.url,
            direct_path: pic.direct_path,
            hash: pic.hash,
        });
        snake(&info)
    }

    // ── Business profile ──────────────────────────────────────────────────────

    /// Get business profile information for a JID. Resolves to `null` when the
    /// JID has no business profile.
    pub async fn get_business_profile(&self, jid: &str) -> Result<BridgeValue, BridgeError> {
        let target_jid = helpers::parse_jid(jid)?;
        let profile = self
            .client
            .execute(wacore::iq::business::BusinessProfileSpec::new(&target_jid))
            .await?;
        let result = profile.as_ref().map(business_profile_to_result);
        snake(&result)
    }

    // ── User info / status / on-WhatsApp ──────────────────────────────────────

    /// Fetch user status/about text for one or more JIDs.
    pub async fn fetch_status(&self, jids: Vec<String>) -> Result<BridgeValue, BridgeError> {
        let parsed_jids: Vec<Jid> = jids
            .iter()
            .map(|s| helpers::parse_jid(s))
            .collect::<Result<_, _>>()?;
        let infos = self.client.contacts().get_user_info(&parsed_jids).await?;
        let results: Vec<result_types::FetchStatusResult> = infos
            .values()
            .map(|info| result_types::FetchStatusResult {
                jid: info.jid.to_string(),
                status: info.status.clone(),
            })
            .collect();
        snake(&results)
    }

    /// Fetch user info for one or more JIDs. Returns a `Record<jid,
    /// UserInfoResult>` keyed by the parsed JID string.
    pub async fn fetch_user_info(&self, jids: Vec<String>) -> Result<BridgeValue, BridgeError> {
        let parsed_jids: Vec<Jid> = jids
            .iter()
            .map(|j| helpers::parse_jid(j))
            .collect::<Result<Vec<_>, _>>()?;

        let result = self.client.contacts().get_user_info(&parsed_jids).await?;

        let map: std::collections::HashMap<String, result_types::UserInfoResult> = result
            .iter()
            .map(|(jid, info)| {
                (
                    jid.to_string(),
                    result_types::UserInfoResult {
                        jid: info.jid.to_string(),
                        lid: info.lid.as_ref().map(|l| l.to_string()),
                        status: info.status.clone(),
                        picture_id: info.picture_id.clone(),
                        is_business: info.is_business,
                    },
                )
            })
            .collect();
        snake(&map)
    }

    /// Check whether one or more phone numbers / JIDs are on WhatsApp. Bare
    /// digits become PN JIDs; anything with `@` is parsed as a full JID.
    pub async fn is_on_whatsapp(&self, phones: Vec<String>) -> Result<BridgeValue, BridgeError> {
        let jids: Vec<Jid> = phones
            .iter()
            .map(|p| {
                if p.contains('@') {
                    helpers::parse_jid(p)
                } else {
                    Ok(Jid::pn(p.as_str()))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let results = self.client.contacts().is_on_whatsapp(&jids).await?;

        // `Jid::push_to` skips the `fmt::Display` / `dyn Write` dispatch path.
        fn jid_to_owned(jid: &Jid) -> String {
            let mut buf = String::with_capacity(jid.user.len() + jid.server.as_str().len() + 8);
            jid.push_to(&mut buf);
            buf
        }

        let out: Vec<result_types::IsOnWhatsAppResult> = results
            .iter()
            .map(|r| result_types::IsOnWhatsAppResult {
                jid: jid_to_owned(&r.jid),
                is_registered: r.is_registered,
                lid: r.lid.as_ref().map(jid_to_owned),
                pn_jid: r.pn_jid.as_ref().map(jid_to_owned),
                is_business: r.is_business,
            })
            .collect();
        snake(&out)
    }

    // ── Blocking ──────────────────────────────────────────────────────────────

    /// Block or unblock a contact.
    pub async fn update_block_status(
        &self,
        jid: &str,
        action: serde_json::Value,
    ) -> Result<(), BridgeError> {
        use result_types::BlockAction;
        let action: BlockAction = serde_json::from_value(action)
            .map_err(|e| errors::invalid_arg("blockAction", e.to_string()))?;
        let target = helpers::parse_jid(jid)?;

        match action {
            BlockAction::Block => self.client.blocking().block(&target).await?,
            BlockAction::Unblock => self.client.blocking().unblock(&target).await?,
        }
        Ok(())
    }

    /// Fetch the full blocklist.
    pub async fn fetch_blocklist(&self) -> Result<BridgeValue, BridgeError> {
        let entries = self.client.blocking().get_blocklist().await?;
        let out: Vec<result_types::BlocklistEntryResult> = entries
            .iter()
            .map(|e| result_types::BlocklistEntryResult {
                jid: e.jid.to_string(),
                timestamp: e.timestamp.map(|v| v as f64),
            })
            .collect();
        snake(&out)
    }

    // ── Privacy / disappearing mode ─────────────────────────────────────────────

    /// Fetch all privacy settings as a `Record<category, value>`.
    pub async fn fetch_privacy_settings(&self) -> Result<BridgeValue, BridgeError> {
        let response = self.client.fetch_privacy_settings().await?;
        let map: std::collections::HashMap<&str, &str> = response
            .settings
            .iter()
            .map(|s| (s.category.as_str(), s.value.as_str()))
            .collect();
        snake(&map)
    }

    /// Update a single privacy setting.
    pub async fn update_privacy_setting(
        &self,
        category: &str,
        value: &str,
    ) -> Result<(), BridgeError> {
        self.client
            .set_privacy_setting(category.into(), value.into())
            .await?;
        Ok(())
    }

    /// Set default disappearing messages duration (seconds). 0 to disable.
    pub async fn update_default_disappearing_mode(&self, duration: u32) -> Result<(), BridgeError> {
        self.client
            .set_default_disappearing_mode(duration)
            .await
            .map_err(BridgeError::from)
    }

    // ── Chat actions ────────────────────────────────────────────────────────────

    /// Pin or unpin a chat.
    pub async fn pin_chat(&self, jid: &str, pin: bool) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        if pin {
            self.client.chat_actions().pin_chat(&chat_jid).await
        } else {
            self.client.chat_actions().unpin_chat(&chat_jid).await
        }
        .map_err(BridgeError::from)
    }

    /// Mute or unmute a chat. `mute_until` is a positive timestamp (ms) to mute
    /// until that time, or `None` to unmute.
    pub async fn mute_chat(&self, jid: &str, mute_until: Option<f64>) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        match mute_until {
            Some(ts) => {
                self.client
                    .chat_actions()
                    .mute_chat_until(&chat_jid, ts as i64)
                    .await
            }
            None => self.client.chat_actions().unmute_chat(&chat_jid).await,
        }
        .map_err(BridgeError::from)
    }

    /// Archive or unarchive a chat.
    pub async fn archive_chat(&self, jid: &str, archive: bool) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        if archive {
            self.client
                .chat_actions()
                .archive_chat(&chat_jid, None)
                .await
        } else {
            self.client
                .chat_actions()
                .unarchive_chat(&chat_jid, None)
                .await
        }
        .map_err(BridgeError::from)
    }

    /// Star or unstar a message.
    pub async fn star_message(
        &self,
        jid: &str,
        message_id: &str,
        star: bool,
    ) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        if star {
            self.client
                .chat_actions()
                .star_message(&chat_jid, None, message_id, true)
                .await
        } else {
            self.client
                .chat_actions()
                .unstar_message(&chat_jid, None, message_id, true)
                .await
        }
        .map_err(BridgeError::from)
    }

    /// Mark a chat as read or unread via app state mutation (distinct from
    /// `read_messages`, which sends read receipts).
    pub async fn mark_chat_as_read(&self, jid: &str, read: bool) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        self.client
            .chat_actions()
            .mark_chat_as_read(&chat_jid, read, None)
            .await
            .map_err(BridgeError::from)
    }

    /// Delete a chat via app state mutation.
    pub async fn delete_chat(&self, jid: &str) -> Result<(), BridgeError> {
        let chat_jid = helpers::parse_jid(jid)?;
        self.client
            .chat_actions()
            .delete_chat(&chat_jid, true, None)
            .await
            .map_err(BridgeError::from)
    }

    // ── Calls ───────────────────────────────────────────────────────────────────

    /// Reject an incoming call.
    pub async fn reject_call(&self, call_id: &str, call_from: &str) -> Result<(), BridgeError> {
        let from_jid = helpers::parse_jid(call_from)?;
        self.client
            .reject_call(call_id, &from_jid)
            .await
            .map_err(BridgeError::from)
    }

    // ── Presence / chat state ─────────────────────────────────────────────────────

    /// Send presence status (`"available"` or `"unavailable"`).
    pub async fn send_presence(&self, status: serde_json::Value) -> Result<(), BridgeError> {
        use result_types::PresenceStatus;
        let status: PresenceStatus = serde_json::from_value(status)
            .map_err(|e| errors::invalid_arg("presenceStatus", e.to_string()))?;
        let presence_status = match status {
            PresenceStatus::Available => whatsapp_rust::features::PresenceStatus::Available,
            PresenceStatus::Unavailable => whatsapp_rust::features::PresenceStatus::Unavailable,
        };

        self.client
            .presence()
            .set(presence_status)
            .await
            .map_err(BridgeError::from)
    }

    /// Subscribe to a contact's presence updates.
    pub async fn presence_subscribe(&self, jid: &str) -> Result<(), BridgeError> {
        let target = helpers::parse_jid(jid)?;
        self.client
            .presence()
            .subscribe(&target)
            .await
            .map_err(BridgeError::from)
    }

    /// Send a chat state update (typing indicator). `state` is `"composing"`,
    /// `"recording"`, or `"paused"`.
    pub async fn send_chat_state(
        &self,
        jid: &str,
        state: serde_json::Value,
    ) -> Result<(), BridgeError> {
        use result_types::ChatState;
        let state: ChatState = serde_json::from_value(state)
            .map_err(|e| errors::invalid_arg("chatState", e.to_string()))?;
        let to = helpers::parse_jid(jid)?;

        let chat_state = match state {
            ChatState::Composing => whatsapp_rust::features::ChatStateType::Composing,
            ChatState::Recording => whatsapp_rust::features::ChatStateType::Recording,
            ChatState::Paused => whatsapp_rust::features::ChatStateType::Paused,
        };

        self.client
            .chatstate()
            .send(&to, chat_state)
            .await
            .map_err(BridgeError::from)
    }

    // ── Reachout timelock ────────────────────────────────────────────────────────

    /// Fetch the account reachout timelock via MEX. Returns the
    /// `xwa2_fetch_account_reachout_timelock` payload (or `null`).
    pub async fn fetch_reachout_timelock(&self) -> Result<BridgeValue, BridgeError> {
        use wacore::iq::mex_operations::fetch_reachout_timelock;
        use whatsapp_rust::features::MexRequest;

        let response = self
            .client
            .mex()
            .query(MexRequest::new(
                fetch_reachout_timelock::NAME,
                fetch_reachout_timelock::DOC_ID,
                serde_json::json!({}),
            ))
            .await?;

        let payload = response
            .data
            .as_ref()
            .and_then(|d| d.get("xwa2_fetch_account_reachout_timelock"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        snake(&payload)
    }

    // ── Message history ───────────────────────────────────────────────────────────

    /// Request on-demand message history from the primary phone. Returns the
    /// message ID of the PDO request; results arrive as history_sync events.
    pub async fn fetch_message_history(
        &self,
        count: i32,
        chat_jid: &str,
        oldest_msg_id: &str,
        oldest_msg_from_me: bool,
        oldest_msg_timestamp_ms: f64,
    ) -> Result<String, BridgeError> {
        let chat = helpers::parse_jid(chat_jid)?;
        let msg_id = self
            .client
            .fetch_message_history(
                &chat,
                oldest_msg_id,
                oldest_msg_from_me,
                oldest_msg_timestamp_ms as i64,
                count,
            )
            .await?;
        Ok(msg_id)
    }
}

/// Build a [`result_types::BusinessProfileResult`] from the core
/// `BusinessProfile`. Ported from the old `wasm_client::business_profile_to_result`.
fn business_profile_to_result(
    p: &wacore::iq::business::BusinessProfile,
) -> result_types::BusinessProfileResult {
    result_types::BusinessProfileResult {
        wid: p.wid.as_ref().map(|j| j.to_string()),
        description: p.description.clone(),
        email: p.email.clone(),
        website: p.website.clone(),
        categories: p
            .categories
            .iter()
            .map(|c| result_types::BusinessCategoryResult {
                id: c.id.clone(),
                name: c.name.clone(),
            })
            .collect(),
        address: p.address.clone(),
        business_hours: result_types::BusinessHoursResult {
            timezone: p.business_hours.timezone.clone(),
            business_config: p.business_hours.business_config.as_ref().map(|configs| {
                configs
                    .iter()
                    .map(|c| result_types::BusinessHoursConfigResult {
                        day_of_week: c.day_of_week.to_string(),
                        mode: c.mode.to_string(),
                        open_time: c.open_time as f64,
                        close_time: c.close_time as f64,
                    })
                    .collect()
            }),
        },
    }
}
