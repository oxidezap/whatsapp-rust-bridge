//! Typed return values and parameter enums for the exported client methods.
//!
//! Port of the old wasm `src/result_types.rs` with the tsify/wasm-bindgen layer
//! removed. The serde derives/attrs are kept verbatim: the napi (or future wasm)
//! binding serializes results into a `BridgeValue` (via `to_bridge_camel`) and
//! deserializes JS inputs via `serde_json`. Byte fields keep
//! `#[serde(with = "serde_bytes")]` so they always go through `serialize_bytes`
//! → `BridgeValue::Bytes` (materialized as `Uint8Array`).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Parameter enums — typed string alternatives for &str dispatch
// ---------------------------------------------------------------------------

/// Media type for upload/download operations.
#[derive(Debug, Clone, Copy, Deserialize)]
pub enum MediaType {
    #[serde(rename = "image")]
    Image,
    #[serde(rename = "video")]
    Video,
    #[serde(rename = "audio")]
    Audio,
    #[serde(rename = "document")]
    Document,
    #[serde(rename = "sticker")]
    Sticker,
    #[serde(rename = "thumbnail-link")]
    ThumbnailLink,
    #[serde(rename = "md-msg-hist")]
    History,
    #[serde(rename = "md-app-state")]
    AppState,
    /// Product catalog image — uses same crypto as Image.
    #[serde(rename = "product-catalog-image")]
    ProductCatalogImage,
}

impl From<MediaType> for wacore::download::MediaType {
    fn from(mt: MediaType) -> Self {
        match mt {
            MediaType::Image => Self::Image,
            MediaType::Video => Self::Video,
            MediaType::Audio => Self::Audio,
            MediaType::Document => Self::Document,
            MediaType::Sticker => Self::Sticker,
            MediaType::ThumbnailLink => Self::LinkThumbnail,
            MediaType::History => Self::History,
            MediaType::AppState => Self::AppState,
            MediaType::ProductCatalogImage => Self::ProductCatalogImage,
        }
    }
}

/// Block/unblock action.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockAction {
    Block,
    Unblock,
}

/// Presence status.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Available,
    Unavailable,
}

/// Chat state (typing indicator).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Composing,
    Recording,
    Paused,
}

/// Group participant action.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupParticipantAction {
    Add,
    Remove,
    Promote,
    Demote,
}

/// Group setting type.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupSetting {
    Locked,
    Announce,
    MembershipApproval,
}

/// Group member add mode.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberAddMode {
    AdminAdd,
    AllMemberAdd,
}

/// Picture type for profile picture URL.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PictureType {
    Preview,
    Image,
}

/// Group join request action.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupRequestAction {
    Approve,
    Reject,
}

// ---------------------------------------------------------------------------
// Result types — serialized return values
// ---------------------------------------------------------------------------

/// Result from `updateProfilePicture` or `removeProfilePicture`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePictureResult {
    pub id: String,
}

/// Result from `profilePictureUrl`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePictureInfo {
    pub id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A single entry from `fetchBlocklist`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlocklistEntryResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

/// A single entry from `fetchUserInfo`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfoResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture_id: Option<String>,
    pub is_business: bool,
}

/// A participant change result from `groupParticipantsUpdate`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantChangeResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A single media host from `getMediaConn`.
#[derive(Serialize)]
pub struct MediaHost {
    pub hostname: String,
}

/// Result from `getMediaConn`.
#[derive(Serialize)]
pub struct MediaConnResult {
    pub auth: String,
    pub ttl: f64,
    pub hosts: Vec<MediaHost>,
}

/// Result from `uploadMedia`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadMediaResult {
    pub url: String,
    pub direct_path: String,
    #[serde(with = "serde_bytes")]
    pub media_key: [u8; 32],
    #[serde(with = "serde_bytes")]
    pub file_sha256: [u8; 32],
    #[serde(with = "serde_bytes")]
    pub file_enc_sha256: [u8; 32],
    pub file_length: f64,
}

/// Result from `encryptMediaStream`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptMediaResult {
    #[serde(with = "serde_bytes")]
    pub media_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub file_sha256: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub file_enc_sha256: Vec<u8>,
    pub file_length: f64,
}

/// A single voter entry for `getAggregateVotesInPollMessage`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollVoterEntry {
    pub voter: String,
    #[serde(with = "serde_bytes")]
    pub enc_payload: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub enc_iv: Vec<u8>,
}

/// A message key for `readMessages`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadMessageKey {
    pub remote_jid: String,
    pub id: String,
    #[serde(default)]
    pub participant: Option<String>,
}

/// Result from `createPoll`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePollResult {
    pub message_id: String,
    #[serde(with = "serde_bytes")]
    pub message_secret: Vec<u8>,
}

/// Result from `isOnWhatsApp`.
///
/// Mirrors the core `IsOnWhatsAppResult` so callers get the LID/PN counterpart
/// and business flag from the same usync round trip — no follow-up
/// `fetchUserInfo` IQ needed for the common "check + enrich" flow.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IsOnWhatsAppResult {
    pub jid: String,
    pub is_registered: bool,
    /// LID counterpart of `jid` when the input was a PN, populated from the
    /// usync `<lid>` attribute (or the local LID/PN cache).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lid: Option<String>,
    /// PN counterpart, set when the server responds with a LID as primary JID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pn_jid: Option<String>,
    pub is_business: bool,
}

/// Result from `fetchStatus`.
#[derive(Serialize)]
pub struct FetchStatusResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Group participant info as returned from `getGroupMetadata` / cached group
/// state. Distinct from `wacore::stanza::groups::GroupParticipantInfo` (the
/// event-time variant that carries `Jid` objects on the wire); naming it
/// separately avoids the TypeScript collision that forced consumers to cast.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMetadataParticipant {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    pub is_admin: bool,
}

/// Result from `getGroupMetadata`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMetadataResult {
    pub id: String,
    pub subject: String,
    pub participants: Vec<GroupMetadataParticipant>,
    pub addressing_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_id: Option<String>,
    pub is_locked: bool,
    pub is_announcement: bool,
    pub ephemeral_expiration: f64,
    pub membership_approval: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_add_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_link_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    pub is_parent_group: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_group_jid: Option<String>,
    pub is_default_sub_group: bool,
    pub is_general_chat: bool,
    pub allow_non_admin_sub_group_creation: bool,
}

/// Result from newsletter methods.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterMetadataResult {
    pub jid: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub subscriber_count: f64,
    pub verification: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_time: Option<f64>,
}

/// Result from `getMemoryDiagnostics`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDiagnosticsResult {
    pub group_cache: f64,
    pub device_registry_cache: f64,
    pub sender_key_device_cache: f64,
    pub lid_pn_lid_entries: f64,
    pub lid_pn_pn_entries: f64,
    pub recent_messages: f64,
    pub message_retry_counts: f64,
    pub pdo_pending_requests: f64,
    pub session_locks: f64,
    pub chat_lanes: f64,
    pub response_waiters: f64,
    pub node_waiters: f64,
    pub pending_retries: f64,
    pub presence_subscriptions: f64,
    pub app_state_key_requests: f64,
    pub app_state_syncing: f64,
    pub signal_cache_sessions: f64,
    pub signal_cache_identities: f64,
    pub signal_cache_sender_keys: f64,
    pub chatstate_handlers: f64,
    pub custom_enc_handlers: f64,
}

/// Result from `getAggregateVotesInPollMessage`.
#[derive(Serialize)]
pub struct PollAggregateResult {
    pub name: String,
    pub voters: Vec<String>,
}

/// Result from `groupRequestParticipantsList`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipRequestResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_time: Option<f64>,
}

/// Result from `getBusinessProfile`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessProfileResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wid: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub website: Vec<String>,
    pub categories: Vec<BusinessCategoryResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub business_hours: BusinessHoursResult,
}

/// Business category info.
#[derive(Serialize)]
pub struct BusinessCategoryResult {
    pub id: String,
    pub name: String,
}

/// Business hours.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessHoursResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_config: Option<Vec<BusinessHoursConfigResult>>,
}

/// Business hours config for a day.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessHoursConfigResult {
    pub day_of_week: String,
    pub mode: String,
    pub open_time: f64,
    pub close_time: f64,
}

// ---------------------------------------------------------------------------
// Conversions from core types
// ---------------------------------------------------------------------------

/// Build a [`GroupMetadataResult`] from the core `GroupMetadata`. Ported from
/// the old `wasm_client::group_metadata_to_result` so the conversion lives
/// alongside its result type. Enum-valued fields (`addressing_mode`,
/// `member_add_mode`, `member_link_mode`) are stringified exactly as before.
pub fn group_metadata_to_result(
    metadata: &whatsapp_rust::features::GroupMetadata,
) -> GroupMetadataResult {
    GroupMetadataResult {
        id: metadata.id.to_string(),
        subject: metadata.subject.to_string(),
        participants: metadata
            .participants
            .iter()
            .map(|p| GroupMetadataParticipant {
                jid: p.jid.to_string(),
                phone_number: p.phone_number.as_ref().map(|pn| pn.to_string()),
                is_admin: p.is_admin(),
            })
            .collect(),
        addressing_mode: serde_json::to_string(&metadata.addressing_mode)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string(),
        creator: metadata.creator.as_ref().map(|j| j.to_string()),
        creation_time: metadata.creation_time.map(|v| v as f64),
        subject_time: metadata.subject_time.map(|v| v as f64),
        subject_owner: metadata.subject_owner.as_ref().map(|j| j.to_string()),
        description: metadata.description.clone(),
        description_id: metadata.description_id.clone(),
        is_locked: metadata.is_locked,
        is_announcement: metadata.is_announcement,
        ephemeral_expiration: metadata.ephemeral_expiration as f64,
        membership_approval: metadata.membership_approval,
        member_add_mode: metadata
            .member_add_mode
            .as_ref()
            .map(|m| format!("{:?}", m)),
        member_link_mode: metadata
            .member_link_mode
            .as_ref()
            .map(|m| format!("{:?}", m)),
        size: metadata.size.map(|v| v as f64),
        is_parent_group: metadata.is_parent_group,
        parent_group_jid: metadata.parent_group_jid.as_ref().map(|j| j.to_string()),
        is_default_sub_group: metadata.is_default_sub_group,
        is_general_chat: metadata.is_general_chat,
        allow_non_admin_sub_group_creation: metadata.allow_non_admin_sub_group_creation,
    }
}
