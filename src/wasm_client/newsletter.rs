//! Newsletter (channel) operations.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Newsletter ────────────────────────────────────────────────────────

    /// Create a new newsletter (channel).
    #[wasm_bindgen(js_name = newsletterCreate)]
    pub async fn newsletter_create(
        &self,
        name: &str,
        description: Option<String>,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let result = self
            .client
            .online()
            .await
            .newsletter()
            .create(name, description.as_deref())
            .await?;

        Ok(newsletter_metadata_to_result(&result))
    }

    /// Fetch metadata for a newsletter by JID.
    #[wasm_bindgen(js_name = newsletterMetadata)]
    pub async fn newsletter_metadata(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;

        let result = self
            .client
            .online()
            .await
            .newsletter()
            .get_metadata(&target)
            .await?;

        Ok(newsletter_metadata_to_result(&result))
    }

    /// Subscribe (join) a newsletter.
    #[wasm_bindgen(js_name = newsletterSubscribe)]
    pub async fn newsletter_subscribe(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;

        let result = self
            .client
            .online()
            .await
            .newsletter()
            .join(&target)
            .await?;

        Ok(newsletter_metadata_to_result(&result))
    }

    /// Unsubscribe (leave) a newsletter.
    #[wasm_bindgen(js_name = newsletterUnsubscribe)]
    pub async fn newsletter_unsubscribe(
        &self,
        jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .online()
            .await
            .newsletter()
            .leave(&target)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Send a reaction to a newsletter message.
    ///
    /// `server_id` is the server-assigned message ID (passed as string to avoid
    /// JS number precision issues). `reaction` is the emoji code, or null/empty
    /// to remove a reaction.
    #[wasm_bindgen(js_name = newsletterReactMessage)]
    pub async fn newsletter_react_message(
        &self,
        jid: &str,
        server_id: &str,
        reaction: Option<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let sid: u64 = server_id.parse().map_err(|e: std::num::ParseIntError| {
            crate::errors::invalid_arg("serverId", e.to_string())
        })?;
        self.client
            .online()
            .await
            .newsletter()
            .send_reaction(&target, sid, reaction.as_deref().unwrap_or(""))
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a newsletter's follower-activity notifications — the channel
    /// mute a subscriber toggles (WA Web `MUTE_FOLLOWER_ACTIVITY`). `muted = true`
    /// silences them.
    ///
    /// The name does not say which of the core's two mutes this is;
    /// `newsletterFollowerMute` is the explicit spelling and does the same
    /// thing. Kept so existing callers keep working.
    #[wasm_bindgen(js_name = newsletterMute)]
    pub async fn newsletter_mute(
        &self,
        jid: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        self.newsletter_follower_mute(jid, muted).await
    }

    /// Mute or unmute a newsletter's follower-activity notifications
    /// (`MUTE_FOLLOWER_ACTIVITY`) — what a subscriber toggles.
    #[wasm_bindgen(js_name = newsletterFollowerMute)]
    pub async fn newsletter_follower_mute(
        &self,
        jid: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .online()
            .await
            .newsletter()
            .set_follower_mute(&target, muted)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a newsletter's admin-activity notifications
    /// (`MUTE_ADMIN_ACTIVITY`). Only meaningful for owners and admins.
    #[wasm_bindgen(js_name = newsletterAdminMute)]
    pub async fn newsletter_admin_mute(
        &self,
        jid: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .online()
            .await
            .newsletter()
            .set_admin_mute(&target, muted)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// List every newsletter this account is subscribed to.
    #[wasm_bindgen(js_name = newsletterList)]
    pub async fn newsletter_list(
        &self,
    ) -> Result<Vec<crate::result_types::NewsletterMetadataResult>, crate::errors::BridgeError>
    {
        let list = self
            .client
            .online()
            .await
            .newsletter()
            .list_subscribed()
            .await?;
        Ok(list.iter().map(newsletter_metadata_to_result).collect())
    }

    /// Fetch a newsletter's metadata by its invite code.
    #[wasm_bindgen(js_name = newsletterMetadataByInvite)]
    pub async fn newsletter_metadata_by_invite(
        &self,
        invite_code: &str,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let meta = self
            .client
            .online()
            .await
            .newsletter()
            .get_metadata_by_invite(invite_code)
            .await?;
        Ok(newsletter_metadata_to_result(&meta))
    }

    /// Update a newsletter's name and/or description. Returns the refreshed
    /// metadata.
    #[wasm_bindgen(js_name = newsletterUpdate)]
    pub async fn newsletter_update(
        &self,
        jid: &str,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let meta = self
            .client
            .online()
            .await
            .newsletter()
            .update(&target, name.as_deref(), description.as_deref())
            .await?;
        Ok(newsletter_metadata_to_result(&meta))
    }

    /// Delete a newsletter. Owner-only.
    #[wasm_bindgen(js_name = newsletterDelete)]
    pub async fn newsletter_delete(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .online()
            .await
            .newsletter()
            .delete(&target)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Transfer a newsletter's ownership. Owner-only.
    ///
    /// `user` may be a LID or a phone-number JID; the core resolves the latter
    /// to its LID, the only form the server addresses admins by.
    #[wasm_bindgen(js_name = newsletterChangeOwner)]
    pub async fn newsletter_change_owner(
        &self,
        jid: &str,
        user: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let user_jid = parse_jid(user)?;
        self.client
            .online()
            .await
            .newsletter()
            .change_owner(&target, &user_jid)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Demote a newsletter admin back to subscriber. Owner-only.
    #[wasm_bindgen(js_name = newsletterDemoteAdmin)]
    pub async fn newsletter_demote_admin(
        &self,
        jid: &str,
        user: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let user_jid = parse_jid(user)?;
        self.client
            .online()
            .await
            .newsletter()
            .demote_admin(&target, &user_jid)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Replace a newsletter's picture with the given JPEG bytes. Returns the
    /// refreshed metadata.
    #[wasm_bindgen(js_name = newsletterSetPicture)]
    pub async fn newsletter_set_picture(
        &self,
        jid: &str,
        jpeg: &[u8],
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let meta = self
            .client
            .online()
            .await
            .newsletter()
            .set_picture(&target, jpeg)
            .await?;
        Ok(newsletter_metadata_to_result(&meta))
    }

    /// Remove a newsletter's picture. Returns the refreshed metadata.
    #[wasm_bindgen(js_name = newsletterRemovePicture)]
    pub async fn newsletter_remove_picture(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let meta = self
            .client
            .online()
            .await
            .newsletter()
            .remove_picture(&target)
            .await?;
        Ok(newsletter_metadata_to_result(&meta))
    }

    /// Fetch a newsletter's admin-side information, including its admin count.
    #[wasm_bindgen(js_name = newsletterAdminInfo)]
    pub async fn newsletter_admin_info(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::NewsletterAdminInfoResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let info = self
            .client
            .online()
            .await
            .newsletter()
            .get_admin_info(&target)
            .await?;
        Ok(crate::result_types::NewsletterAdminInfoResult {
            admin_count: info.admin_count.map(f64::from),
            admin_profile: info.admin_profile.as_ref().map(admin_profile_to_result),
            admin_profiles_enabled: info.admin_profiles_enabled,
        })
    }

    /// Fetch up to `count` followers of a newsletter.
    ///
    /// One page, no cursor: the core's query takes a count and the real client
    /// does not paginate this list.
    #[wasm_bindgen(js_name = newsletterFollowers)]
    pub async fn newsletter_followers(
        &self,
        jid: &str,
        count: u32,
    ) -> Result<Vec<crate::result_types::NewsletterFollowerResult>, crate::errors::BridgeError>
    {
        let target = parse_jid(jid)?;
        let followers = self
            .client
            .online()
            .await
            .newsletter()
            .get_followers(&target, count)
            .await?;
        Ok(followers
            .iter()
            .map(|follower| crate::result_types::NewsletterFollowerResult {
                jid: follower.jid.to_string(),
                phone_jid: follower.phone_jid.as_ref().map(|j| j.to_string()),
                display_name: follower.display_name.clone(),
                username: follower.username.clone(),
                role: follower.role.as_ref().map(newsletter_role_str),
                follow_time: follower.follow_time.map(|v| v as f64),
                admin_profile: follower.admin_profile.as_ref().map(admin_profile_to_result),
            })
            .collect())
    }

    /// Fetch up to `count` messages from a newsletter's history.
    ///
    /// `before` is a `serverId` from a previous page, as a string because it is
    /// a u64. Omit it for the newest page.
    #[wasm_bindgen(js_name = newsletterMessages)]
    pub async fn newsletter_messages(
        &self,
        jid: &str,
        count: u32,
        before: Option<String>,
    ) -> Result<Vec<crate::result_types::NewsletterMessageResult>, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let before_id = match before.as_deref() {
            Some(value) => Some(value.parse::<u64>().map_err(|e| {
                crate::errors::BridgeError::InvalidArgument {
                    field: "before".into(),
                    reason: format!("must be a serverId: {e}"),
                }
            })?),
            None => None,
        };

        let messages = self
            .client
            .online()
            .await
            .newsletter()
            .get_messages(target, count, before_id)
            .await?;

        Ok(messages
            .iter()
            .map(|message| crate::result_types::NewsletterMessageResult {
                message_id: message.message_id.clone(),
                server_id: message.server_id.to_string(),
                timestamp: message.timestamp as f64,
                message_type: message.message_type.as_str().to_owned(),
                is_sender: message.is_sender,
                message: message
                    .message
                    .as_ref()
                    .map(|m| serde_bytes::ByteBuf::from(waproto::codec::message_to_vec(m))),
                reactions: message
                    .reactions
                    .iter()
                    .map(|r| crate::result_types::NewsletterReactionCountResult {
                        code: r.code.clone(),
                        count: r.count as f64,
                    })
                    .collect(),
            })
            .collect())
    }

    /// Subscribe to a newsletter's live updates. Returns the subscription
    /// duration in seconds, as the server set it.
    #[wasm_bindgen(js_name = newsletterSubscribeLiveUpdates)]
    pub async fn newsletter_subscribe_live_updates(
        &self,
        jid: &str,
    ) -> Result<f64, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let duration = self
            .client
            .online()
            .await
            .newsletter()
            .subscribe_live_updates(target)
            .await?;
        Ok(duration as f64)
    }

    /// Edit a newsletter message. `messageId` is the message's own id, not its
    /// `serverId` — reactions key on the latter, edit and revoke on the former.
    #[wasm_bindgen(js_name = newsletterEditMessage)]
    pub async fn newsletter_edit_message(
        &self,
        jid: &str,
        message_id: &str,
        message: &[u8],
    ) -> Result<(), crate::errors::BridgeError> {
        let (target, new_content) = parse_jid_and_msg_bytes(jid, message)?;
        self.client
            .online()
            .await
            .newsletter()
            .edit_message(&target, message_id, new_content)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Revoke (delete) a newsletter message. `messageId` is the message's own
    /// id, not its `serverId`.
    #[wasm_bindgen(js_name = newsletterRevokeMessage)]
    pub async fn newsletter_revoke_message(
        &self,
        jid: &str,
        message_id: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .online()
            .await
            .newsletter()
            .revoke_message(&target, message_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }
}
