//! Contacts, profile, blocking, privacy and presence.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Contacts ─────────────────────────────────────────────────────────

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
    pub async fn is_on_whatsapp(
        &self,
        phones: Vec<String>,
    ) -> Result<Vec<crate::result_types::IsOnWhatsAppResult>, crate::errors::BridgeError> {
        let jids: Vec<Jid> = phones
            .iter()
            .map(|p| {
                // Bare digits → PN JID; anything containing '@' → parse as full JID.
                if p.contains('@') {
                    parse_jid(p)
                } else {
                    Ok(Jid::pn(p))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let results = self
            .client
            .online()
            .await?
            .contacts()
            .is_on_whatsapp(&jids)
            .await?;

        // Use `Jid::push_to` instead of `to_string()` — bypasses the
        // `fmt::Display` / `dyn Write` dispatch path the core ships a specialized
        // fast writer for. Each output `String` is still owned (required to cross
        // the WASM ABI), but we skip the formatter machinery and size the buffer
        // up front to avoid mid-push reallocations.
        fn jid_to_owned(jid: &Jid) -> String {
            let mut buf = String::with_capacity(jid.user.len() + jid.server.as_str().len() + 8);
            jid.push_to(&mut buf);
            buf
        }

        Ok(results
            .iter()
            .map(|r| crate::result_types::IsOnWhatsAppResult {
                jid: jid_to_owned(&r.jid),
                is_registered: r.is_registered,
                lid: r.lid.as_ref().map(jid_to_owned),
                pn_jid: r.pn_jid.as_ref().map(jid_to_owned),
                is_business: r.is_business,
                verified_name: r.verified_name.as_ref().and_then(|v| v.name.clone()),
                username: r.username.as_ref().map(|u| u.to_string()),
            })
            .collect())
    }

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
    pub async fn find_by_username(
        &self,
        username: &str,
        username_key: Option<String>,
    ) -> Result<crate::result_types::UsernameLookupResult, crate::errors::BridgeError> {
        use whatsapp_rust::features::UsernameLookup;

        let lookup = self
            .client
            .online()
            .await?
            .contacts()
            .find_by_username(username, username_key.as_deref())
            .await?;

        Ok(match lookup {
            UsernameLookup::NotFound => crate::result_types::UsernameLookupResult::NotFound,
            UsernameLookup::KeyRequired { username } => {
                crate::result_types::UsernameLookupResult::KeyRequired {
                    username: username.map(|u| u.to_string()),
                }
            }
            UsernameLookup::Found(user) => crate::result_types::UsernameLookupResult::Found {
                jid: user.jid.to_string(),
                pn_jid: user.pn_jid.as_ref().map(|j| j.to_string()),
                username: user.username.map(|u| u.to_string()),
                is_business: user.is_business,
                verified_name: user.verified_name.and_then(|v| v.name),
            },
            // The core marks the answer non-exhaustive. One it learns to name
            // and this bridge does not is not a "not found" to flatten.
            other => {
                return Err(crate::errors::internal(format!(
                    "unhandled username lookup answer: {other:?}"
                )));
            }
        })
    }

    /// Get the profile picture URL for a user or group.
    ///
    /// `picture_type` should be "preview" or "image".
    #[wasm_bindgen(js_name = profilePictureUrl)]
    pub async fn profile_picture_url(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "PictureType")] picture_type: JsValue,
        timeout_ms: Option<f64>,
    ) -> Result<Option<crate::result_types::ProfilePictureInfo>, crate::errors::BridgeError> {
        let picture_type =
            from_js_input::<crate::result_types::PictureType>("picture_type", picture_type)?;
        use crate::result_types::PictureType;
        let target = parse_jid(jid)?;
        let preview = match picture_type {
            PictureType::Preview => true,
            PictureType::Image => false,
        };

        let timeout = parse_optional_timeout_ms("timeoutMs", timeout_ms)?;

        let result = self
            .client
            .online()
            .await?
            .contacts()
            .get_profile_picture_with_timeout(&target, preview, timeout)
            .await?;

        Ok(result.map(|pic| crate::result_types::ProfilePictureInfo {
            id: pic.id,
            url: pic.url,
            direct_path: pic.direct_path,
            hash: pic.hash,
        }))
    }

    /// Fetch user info for one or more JIDs.
    #[wasm_bindgen(js_name = fetchUserInfo, skip_typescript)]
    pub async fn fetch_user_info(
        &self,
        jids: Vec<String>,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed_jids: Vec<Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<Vec<_>, _>>()?;

        let result = self
            .client
            .online()
            .await?
            .contacts()
            .get_user_info(&parsed_jids)
            .await?;

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
            js_sys::Reflect::set(&obj, &JsValue::from_str(&jid.to_string()), &js_entry)?;
        }
        Ok(obj.into())
    }

    // ── Profile ──────────────────────────────────────────────────────────

    /// Set the user's push name (display name).
    #[wasm_bindgen(js_name = setPushName)]
    pub async fn set_push_name(&self, name: &str) -> Result<(), crate::errors::BridgeError> {
        self.client
            .online()
            .await?
            .profile()
            .set_push_name(name)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Read this account's own Meta username, its state and its username key.
    ///
    /// `null` means no username is set: the server answers 404 and the core
    /// reads it that way. Only the read is exposed — setting a username or its
    /// key changes the account's identity in a way the server does not undo,
    /// so the core leaves those two MEX operations unwrapped.
    #[wasm_bindgen(js_name = getUsername)]
    pub async fn get_username(
        &self,
    ) -> Result<Option<crate::result_types::OwnUsernameResult>, crate::errors::BridgeError> {
        let own = self.client.online().await?.mex().get_username().await?;
        Ok(own.map(|own| crate::result_types::OwnUsernameResult {
            username: own.username,
            state: own.state,
            key: own.key,
        }))
    }

    /// Set the profile picture for the logged-in user.
    #[wasm_bindgen(js_name = updateProfilePicture)]
    pub async fn update_profile_picture(
        &self,
        img_data: Vec<u8>,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        let result = self
            .client
            .online()
            .await?
            .profile()
            .set_profile_picture(img_data)
            .await?;

        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Remove the profile picture for the logged-in user.
    #[wasm_bindgen(js_name = removeProfilePicture)]
    pub async fn remove_profile_picture(
        &self,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        let result = self
            .client
            .online()
            .await?
            .profile()
            .remove_profile_picture()
            .await?;

        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Set the profile picture for a group the user administers.
    ///
    /// Mirrors the core `SetProfilePictureSpec::set_group` path — same IQ as
    /// the self update, just routed at the JID level so admins can change a
    /// group's avatar from JS without an extra capability check.
    #[wasm_bindgen(js_name = setGroupProfilePicture)]
    pub async fn set_group_profile_picture(
        &self,
        group_jid: &str,
        img_data: Vec<u8>,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        use wacore_binary::JidExt;
        let target = parse_jid(group_jid)?;
        if !target.is_group() {
            return Err(crate::errors::invalid_arg(
                "groupJid",
                "must be a group jid",
            ));
        }
        let result = self
            .client
            .online()
            .await?
            .execute(wacore::iq::contacts::SetProfilePictureSpec::set_group(
                &target, img_data,
            ))
            .await?;
        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Remove a group's profile picture.
    #[wasm_bindgen(js_name = removeGroupProfilePicture)]
    pub async fn remove_group_profile_picture(
        &self,
        group_jid: &str,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        use wacore_binary::JidExt;
        let target = parse_jid(group_jid)?;
        if !target.is_group() {
            return Err(crate::errors::invalid_arg(
                "groupJid",
                "must be a group jid",
            ));
        }
        let result = self
            .client
            .online()
            .await?
            .execute(wacore::iq::contacts::SetProfilePictureSpec::remove_group(
                &target,
            ))
            .await?;
        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Update the user's status text (about).
    #[wasm_bindgen(js_name = updateProfileStatus)]
    pub async fn update_profile_status(
        &self,
        status: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .online()
            .await?
            .profile()
            .set_status_text(status)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Blocking ──────────────────────────────────────────────────────────

    /// Block or unblock a contact.
    #[wasm_bindgen(js_name = updateBlockStatus)]
    pub async fn update_block_status(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "BlockAction")] action: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let action = from_js_input::<crate::result_types::BlockAction>("action", action)?;
        use crate::result_types::BlockAction;
        let target = parse_jid(jid)?;

        match action {
            BlockAction::Block => {
                self.client
                    .online()
                    .await?
                    .blocking()
                    .block(&target)
                    .await?
            }
            BlockAction::Unblock => {
                self.client
                    .online()
                    .await?
                    .blocking()
                    .unblock(&target)
                    .await?
            }
        }
        Ok(())
    }

    /// Fetch the full blocklist.
    #[wasm_bindgen(js_name = fetchBlocklist)]
    pub async fn fetch_blocklist(
        &self,
    ) -> Result<Vec<crate::result_types::BlocklistEntryResult>, crate::errors::BridgeError> {
        let entries = self
            .client
            .online()
            .await?
            .blocking()
            .get_blocklist()
            .await?;

        Ok(entries
            .iter()
            .map(|e| crate::result_types::BlocklistEntryResult {
                jid: e.jid.to_string(),
                timestamp: e.timestamp.map(|v| v as f64),
            })
            .collect())
    }

    // ── Privacy settings ──────────────────────────────────────────────

    /// Fetch all privacy settings.
    #[wasm_bindgen(js_name = fetchPrivacySettings)]
    pub async fn fetch_privacy_settings(&self) -> Result<JsValue, crate::errors::BridgeError> {
        let response = self.client.online().await?.fetch_privacy_settings().await?;
        let map: std::collections::HashMap<&str, &str> = response
            .settings
            .iter()
            .map(|s| (s.category.as_str(), s.value.as_str()))
            .collect();
        serde_wasm_bindgen::to_value(&map).map_err(crate::errors::BridgeError::from)
    }

    /// Update a single privacy setting.
    #[wasm_bindgen(js_name = updatePrivacySetting)]
    pub async fn update_privacy_setting(
        &self,
        category: &str,
        value: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .online()
            .await?
            .set_privacy_setting(category.into(), value.into())
            .await?;
        Ok(())
    }

    /// Set default disappearing messages duration (seconds). 0 to disable.
    #[wasm_bindgen(js_name = updateDefaultDisappearingMode)]
    pub async fn update_default_disappearing_mode(
        &self,
        duration: u32,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .online()
            .await?
            .set_default_disappearing_mode(duration)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Presence ─────────────────────────────────────────────────────────

    /// Send presence status ("available" or "unavailable").
    #[wasm_bindgen(js_name = sendPresence)]
    pub async fn send_presence(
        &self,
        #[wasm_bindgen(unchecked_param_type = "PresenceStatus")] status: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let status = from_js_input::<crate::result_types::PresenceStatus>("status", status)?;
        use crate::result_types::PresenceStatus;
        let presence_status = match status {
            PresenceStatus::Available => whatsapp_rust::features::PresenceStatus::Available,
            PresenceStatus::Unavailable => whatsapp_rust::features::PresenceStatus::Unavailable,
        };
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .presence()
            .set(presence_status)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Subscribe to a contact's presence updates.
    #[wasm_bindgen(js_name = presenceSubscribe)]
    pub async fn presence_subscribe(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .unwaited(Unwaited::Redriven)
            .presence()
            .subscribe(&target)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── User status ──────────────────────────────────────────────────────

    /// Fetch user status/about text for one or more JIDs.
    #[wasm_bindgen(js_name = fetchStatus)]
    pub async fn fetch_status(
        &self,
        jids: Vec<String>,
    ) -> Result<Vec<crate::result_types::FetchStatusResult>, crate::errors::BridgeError> {
        let parsed_jids: Vec<Jid> = jids
            .iter()
            .map(|s| parse_jid(s))
            .collect::<Result<_, _>>()?;
        let infos = self
            .client
            .online()
            .await?
            .contacts()
            .get_user_info(&parsed_jids)
            .await?;
        Ok(infos
            .values()
            .map(|info| crate::result_types::FetchStatusResult {
                jid: info.jid.to_string(),
                status: info.status.clone(),
            })
            .collect())
    }
}
