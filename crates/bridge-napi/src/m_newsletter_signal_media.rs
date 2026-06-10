//! Thin napi forwards for the `newsletter_signal_media` batch: newsletter
//! CRUD/reactions, Signal session/encrypt/decrypt primitives, usync device
//! discovery, participant-node creation, media conn/download/upload, media
//! reupload, plus the two standalone poll-vote helpers (module-level free
//! functions, NOT methods). All logic lives in `bridge-core`.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::WasmWhatsAppClient;
use crate::methods::napi_err;
use crate::value_js::JsBridgeValue;

#[napi]
impl WasmWhatsAppClient {
    // ── Newsletter ────────────────────────────────────────────────────────

    #[napi(js_name = "newsletterCreate")]
    pub async fn newsletter_create(
        &self,
        name: String,
        description: Option<String>,
    ) -> Result<JsBridgeValue> {
        self.core
            .newsletter_create(&name, description)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "newsletterMetadata")]
    pub async fn newsletter_metadata(&self, jid: String) -> Result<JsBridgeValue> {
        self.core
            .newsletter_metadata(&jid)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "newsletterSubscribe")]
    pub async fn newsletter_subscribe(&self, jid: String) -> Result<JsBridgeValue> {
        self.core
            .newsletter_subscribe(&jid)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "newsletterUnsubscribe")]
    pub async fn newsletter_unsubscribe(&self, jid: String) -> Result<()> {
        self.core
            .newsletter_unsubscribe(&jid)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "newsletterReactMessage")]
    pub async fn newsletter_react_message(
        &self,
        jid: String,
        server_id: String,
        reaction: Option<String>,
    ) -> Result<()> {
        self.core
            .newsletter_react_message(&jid, &server_id, reaction)
            .await
            .map_err(napi_err)
    }

    // ── Media reupload ──────────────────────────────────────────────────────

    #[napi(js_name = "requestMediaReupload")]
    pub async fn request_media_reupload(
        &self,
        msg_id: String,
        chat_jid: String,
        media_key: Uint8Array,
        is_from_me: bool,
        participant: Option<String>,
    ) -> Result<String> {
        self.core
            .request_media_reupload(&msg_id, &chat_jid, &media_key, is_from_me, participant)
            .await
            .map_err(napi_err)
    }

    // ── Media ───────────────────────────────────────────────────────────────

    #[napi(js_name = "getMediaConn")]
    pub async fn get_media_conn(&self, force: bool) -> Result<JsBridgeValue> {
        self.core
            .get_media_conn(force)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "downloadMedia")]
    pub async fn download_media(
        &self,
        direct_path: String,
        media_key: Uint8Array,
        file_sha256: Uint8Array,
        file_enc_sha256: Uint8Array,
        file_length: f64,
        media_type: serde_json::Value,
    ) -> Result<Uint8Array> {
        self.core
            .download_media(
                &direct_path,
                &media_key,
                &file_sha256,
                &file_enc_sha256,
                file_length,
                media_type,
            )
            .await
            .map(Uint8Array::from)
            .map_err(napi_err)
    }

    #[napi(js_name = "uploadMedia")]
    pub async fn upload_media(
        &self,
        data: Uint8Array,
        media_type: serde_json::Value,
    ) -> Result<JsBridgeValue> {
        self.core
            .upload_media(&data, media_type)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── Sessions / usync ──────────────────────────────────────────────────────

    #[napi(js_name = "assertSessions")]
    pub async fn assert_sessions(&self, jids: Vec<String>, force: bool) -> Result<bool> {
        self.core
            .assert_sessions(jids, force)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "getUSyncDevices")]
    pub async fn get_usync_devices(
        &self,
        jids: Vec<String>,
        use_cache: bool,
        ignore_zero_devices: bool,
    ) -> Result<JsBridgeValue> {
        self.core
            .get_usync_devices(jids, use_cache, ignore_zero_devices)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    // ── Signal protocol ──────────────────────────────────────────────────────

    #[napi(js_name = "signalEncryptMessage")]
    pub async fn signal_encrypt_message(
        &self,
        jid: String,
        data: Uint8Array,
    ) -> Result<JsBridgeValue> {
        self.core
            .signal_encrypt_message(&jid, &data)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "signalDecryptMessage")]
    pub async fn signal_decrypt_message(
        &self,
        jid: String,
        msg_type: String,
        ciphertext: Uint8Array,
    ) -> Result<Uint8Array> {
        self.core
            .signal_decrypt_message(&jid, &msg_type, &ciphertext)
            .await
            .map(Uint8Array::from)
            .map_err(napi_err)
    }

    #[napi(js_name = "signalEncryptGroupMessage")]
    pub async fn signal_encrypt_group_message(
        &self,
        group_jid: String,
        data: Uint8Array,
        me_id: String,
    ) -> Result<JsBridgeValue> {
        self.core
            .signal_encrypt_group_message(&group_jid, &data, &me_id)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "signalDecryptGroupMessage")]
    pub async fn signal_decrypt_group_message(
        &self,
        group_jid: String,
        author_jid: String,
        msg: Uint8Array,
    ) -> Result<Uint8Array> {
        self.core
            .signal_decrypt_group_message(&group_jid, &author_jid, &msg)
            .await
            .map(Uint8Array::from)
            .map_err(napi_err)
    }

    #[napi(js_name = "signalValidateSession")]
    pub async fn signal_validate_session(&self, jid: String) -> Result<bool> {
        self.core
            .signal_validate_session(&jid)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "signalDeleteSessions")]
    pub async fn signal_delete_sessions(&self, jids: Vec<String>) -> Result<()> {
        self.core
            .signal_delete_sessions(jids)
            .await
            .map_err(napi_err)
    }

    // ── Participant node creation ──────────────────────────────────────────────

    #[napi(js_name = "createParticipantNodesBytes")]
    pub async fn create_participant_nodes_bytes(
        &self,
        jids: Vec<String>,
        bytes: Uint8Array,
        _extra_attrs: serde_json::Value,
    ) -> Result<JsBridgeValue> {
        self.core
            .create_participant_nodes_bytes(jids, &bytes)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }
}

// ── Poll vote helpers — module-level free functions (NOT methods) ─────────────

#[napi(js_name = "decryptPollVote")]
pub fn decrypt_poll_vote(
    enc_payload: Uint8Array,
    enc_iv: Uint8Array,
    message_secret: Uint8Array,
    poll_msg_id: String,
    poll_creator_jid: String,
    voter_jid: String,
    option_names: Vec<String>,
) -> Result<Vec<String>> {
    bridge_core::m_newsletter_signal_media::decrypt_poll_vote(
        &enc_payload,
        &enc_iv,
        &message_secret,
        &poll_msg_id,
        &poll_creator_jid,
        &voter_jid,
        option_names,
    )
    .map_err(napi_err)
}

#[napi(js_name = "getAggregateVotesInPollMessage")]
pub fn get_aggregate_votes_in_poll_message(
    option_names: Vec<String>,
    voters: serde_json::Value,
    message_secret: Uint8Array,
    poll_msg_id: String,
    poll_creator_jid: String,
) -> Result<Vec<JsBridgeValue>> {
    let voters: Vec<bridge_core::result_types::PollVoterEntry> =
        serde_json::from_value(voters).map_err(napi_err)?;
    bridge_core::m_newsletter_signal_media::get_aggregate_votes_in_poll_message(
        option_names,
        voters,
        &message_secret,
        &poll_msg_id,
        &poll_creator_jid,
    )
    .map(|v| v.into_iter().map(JsBridgeValue).collect())
    .map_err(napi_err)
}
