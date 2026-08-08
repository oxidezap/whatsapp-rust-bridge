//! Typed return values and parameter enums for wasm-bindgen exported methods.
//!
//! Using `#[derive(Tsify, Serialize)]` auto-generates TypeScript types
//! and eliminates manual `js_sys::Object` construction + `skip_typescript`.

use serde::{Deserialize, Serialize};
use tsify::Tsify;
use whatsapp_rust::wacore;

// ---------------------------------------------------------------------------
// Parameter enums — typed string alternatives for &str dispatch
// ---------------------------------------------------------------------------

/// Media type for upload/download operations.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
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
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum BlockAction {
    Block,
    Unblock,
}

/// Presence status.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Available,
    Unavailable,
}

/// Chat state (typing indicator).
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Composing,
    Recording,
    Paused,
}

/// Group participant action.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum GroupParticipantAction {
    Add,
    Remove,
    Promote,
    Demote,
    Modify,
}

/// Group setting type.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum GroupSetting {
    Locked,
    Announce,
    MembershipApproval,
}

/// Group member add mode.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum MemberAddMode {
    AdminAdd,
    AllMemberAdd,
}

/// Picture type for profile picture URL.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum PictureType {
    Preview,
    Image,
}

/// Group join request action.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "snake_case")]
pub enum GroupRequestAction {
    Approve,
    Reject,
}

/// Neutral controls for retransmitting an existing message to one device.
///
/// The encoded message remains a separate byte slice so this small control
/// object never base64-encodes or copies the protobuf payload.
#[derive(Debug, Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct MessageRetransmissionInput {
    pub requester_jid: String,
    pub message_id: String,
    pub retry_count: u32,
    #[tsify(optional)]
    pub recipient_jid: Option<String>,
    pub refresh_group_metadata: bool,
}

// ---------------------------------------------------------------------------
// Result types — serialized return values
// ---------------------------------------------------------------------------

/// Result from `updateProfilePicture` or `removeProfilePicture`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePictureResult {
    pub id: String,
}

/// Result from `profilePictureUrl`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
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
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct BlocklistEntryResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

/// A single entry from `fetchUserInfo`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
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
    /// Verified business name from the usync `<business><verified_name>` cert, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_name: Option<String>,
    /// Device IDs from the usync `<devices>` sublist the same query returns. Empty when
    /// the server returned no device list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<u16>,
}

/// A participant change result from `groupParticipantsUpdate`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantChangeResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_request: Option<ParticipantAddRequestResult>,
}

/// Invite fallback returned for a participant that could not be added directly.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantAddRequestResult {
    pub code: String,
    pub expiration: f64,
}

/// A single media host from `getMediaConn`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct MediaHost {
    pub hostname: String,
}

/// Result from `getMediaConn`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct MediaConnResult {
    pub auth: String,
    pub ttl: f64,
    pub hosts: Vec<MediaHost>,
}

/// Result from `uploadMedia`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct UploadMediaResult {
    pub url: String,
    pub direct_path: String,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub media_key: [u8; 32],
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub file_sha256: [u8; 32],
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub file_enc_sha256: [u8; 32],
    pub file_length: f64,
}

/// Result from `encryptMediaStream`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct EncryptMediaResult {
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub media_key: Vec<u8>,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub file_sha256: Vec<u8>,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub file_enc_sha256: Vec<u8>,
    pub file_length: f64,
}

/// Public portion of one pre-key in a supplied pairwise session bundle.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SignalPreKeyInput {
    pub key_id: u32,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub public_key: Vec<u8>,
}

/// Signed pre-key in a supplied pairwise session bundle.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SignalSignedPreKeyInput {
    pub key_id: u32,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub public_key: Vec<u8>,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

/// Inputs required to establish one outgoing pairwise session.
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct SignalSessionBundleInput {
    pub registration_id: u32,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub identity_key: Vec<u8>,
    pub signed_pre_key: SignalSignedPreKeyInput,
    #[tsify(optional)]
    pub pre_key: Option<SignalPreKeyInput>,
}

/// Read-only information from a currently open pairwise session.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct SignalSessionInfoResult {
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub base_key: Vec<u8>,
    pub registration_id: u32,
}

/// One linked-identifier to phone-number mapping supplied by the host.
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct LidPnMappingInput {
    pub lid: String,
    pub pn: String,
}

/// Counts produced while moving pairwise sessions between identifier namespaces.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct SignalSessionMigrationResult {
    pub migrated: u32,
    pub skipped: u32,
    pub total: u32,
}

/// A message key for `readMessages`.
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct ReadMessageKey {
    pub remote_jid: String,
    pub id: String,
    #[tsify(optional)]
    pub participant: Option<String>,
}

/// Key of an existing message targeted by `sendReaction` / `sendCommentBytes`.
/// The chat JID comes from the method's `jid` argument; `participant` is the
/// original sender (required for group/status targets).
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct TargetMessageKey {
    pub id: String,
    #[tsify(optional)]
    #[serde(default)]
    pub from_me: bool,
    #[tsify(optional)]
    pub participant: Option<String>,
}

/// Result from `createPoll`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct CreatePollResult {
    pub message_id: String,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub message_secret: Vec<u8>,
}

/// Result from `isOnWhatsApp`.
///
/// Mirrors the core `IsOnWhatsAppResult` so callers get the LID/PN counterpart
/// and business flag from the same usync round trip — no follow-up
/// `fetchUserInfo` IQ needed for the common "check + enrich" flow.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
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
    /// Verified business name from the usync `<business><verified_name>` cert, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_name: Option<String>,
}

/// Result from `fetchStatus`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct FetchStatusResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Group participant info as returned from `getGroupMetadata` / cached group
/// state. Distinct from `wacore::stanza::groups::GroupParticipantInfo` (the
/// event-time variant that carries `Jid` objects on the wire); naming it
/// separately avoids the TypeScript collision that forced consumers to cast.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct GroupMetadataParticipant {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    /// LID counterpart when `jid` is a phone-number JID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lid: Option<String>,
    /// Meta username carried by the participant node, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Protocol role (`member`, `admin`, or `superadmin`).
    pub participant_type: String,
    pub is_admin: bool,
    pub is_super_admin: bool,
}

/// Disappearing-message settings returned by the group `<ephemeral>` node.
///
/// The outer `Option` on `GroupMetadataResult::ephemeral` preserves the
/// distinction between an absent node and a present node whose values are
/// zero or omitted.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct GroupEphemeralSettingsResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<f64>,
}

/// Server-managed group growth lock information.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct GroupGrowthLockInfoResult {
    pub lock_type: String,
    pub expiration: f64,
}

/// Result from `getGroupMetadata`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct GroupMetadataResult {
    pub id: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
    pub participants: Vec<GroupMetadataParticipant>,
    pub addressing_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_pn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_owner_pn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_owner_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_owner_pn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_owner_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_time: Option<f64>,
    pub is_locked: bool,
    pub is_announcement: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<GroupEphemeralSettingsResult>,
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
    pub no_frequently_forwarded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_share_history_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub growth_locked: Option<GroupGrowthLockInfoResult>,
    pub is_suspended: bool,
    pub allow_admin_reports: bool,
    pub is_hidden_group: bool,
    pub is_incognito: bool,
    pub has_group_history: bool,
    pub is_limit_sharing_enabled: bool,
}

/// Result from newsletter methods.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
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
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDiagnosticsResult {
    pub group_cache: f64,
    pub group_cache_bytes: f64,
    pub device_registry_cache: f64,
    pub device_registry_cache_bytes: f64,
    pub sender_key_device_cache: f64,
    pub sender_key_device_cache_bytes: f64,
    pub group_devices_memo: f64,
    pub group_devices_memo_bytes: f64,
    pub lid_pn_lid_entries: f64,
    pub lid_pn_lid_bytes: f64,
    pub lid_pn_pn_entries: f64,
    pub lid_pn_pn_bytes: f64,
    pub recent_messages: f64,
    pub recent_messages_bytes: f64,
    pub message_retry_counts: f64,
    pub undecryptable_dispatched: f64,
    pub pdo_pending_requests: f64,
    pub pdo_requested: f64,
    pub session_locks: f64,
    pub chat_lanes: f64,
    pub group_distribution_locks: f64,
    pub group_distribution_lock_evictions: f64,
    pub group_distribution_lock_eviction_blocks: f64,
    pub resend_rate_limiter_chats: f64,
    pub response_waiters: f64,
    pub node_waiters: f64,
    pub pending_retries: f64,
    pub presence_subscriptions: f64,
    pub app_state_key_requests: f64,
    pub app_state_syncing: f64,
    pub signal_cache_sessions: f64,
    pub signal_cache_sessions_bytes: f64,
    pub signal_cache_identities: f64,
    pub signal_cache_identities_bytes: f64,
    pub signal_cache_sender_keys: f64,
    pub signal_cache_sender_keys_bytes: f64,
    pub history_sync_tasks: f64,
    pub history_sync_payload_bytes: f64,
    pub history_sync_peak_tasks: f64,
    pub history_sync_peak_payload_bytes: f64,
    pub chatstate_handlers: f64,
    pub custom_enc_handlers: f64,
    pub client_estimated_bytes: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_memory_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_pages: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_io_read_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_io_write_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_read_buffer_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_write_buffer_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_tls_state_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_pool_connections: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_pool_buffer_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_inflight_bytes: Option<f64>,
    pub resource_estimated_bytes: f64,
}

/// Allocation churn attributed by whatsapp-rust's own `AllocMeter` to tasks
/// spawned for this client. Available in diagnostics builds only.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct CoreAllocationSnapshotResult {
    pub enabled: bool,
    pub allocated_bytes: f64,
    pub freed_bytes: f64,
    pub allocations: f64,
    pub net_bytes: f64,
}

/// Result from `groupRequestParticipantsList`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct MembershipRequestResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_time: Option<f64>,
}

/// A subgroup returned by a parent-group metadata query.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct CommunitySubgroupResult {
    pub id: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_count: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub is_default_sub_group: bool,
    pub is_general_chat: bool,
}

/// One failed parent/subgroup relationship mutation.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct CommunityLinkFailureResult {
    pub jid: String,
    pub error: f64,
}

/// Result of linking or unlinking subgroups.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct CommunityLinkResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<CommunityLinkFailureResult>,
}

/// Result from `getBusinessProfile`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
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
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct BusinessCategoryResult {
    pub id: String,
    pub name: String,
}

/// Business hours.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct BusinessHoursResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_config: Option<Vec<BusinessHoursConfigResult>>,
}

/// Business hours config for a day.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct BusinessHoursConfigResult {
    pub day_of_week: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_time: Option<f64>,
}

// ---------------------------------------------------------------------------
// Newsletter
// ---------------------------------------------------------------------------

/// One follower of a newsletter, from `newsletterFollowers`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterFollowerResult {
    pub jid: String,
    /// Withheld by the server when the follower's privacy settings hide it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_jid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_profile: Option<NewsletterAdminProfileResult>,
}

/// An admin's published profile on a newsletter.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterAdminProfileResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture_direct_path: Option<String>,
}

/// Result from `newsletterAdminInfo`.
///
/// `adminCount` has no query of its own — it rides along with the admin
/// profile. Absent means the server withheld it (it answers only admins and
/// owners), never zero.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterAdminInfoResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_count: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_profile: Option<NewsletterAdminProfileResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_profiles_enabled: Option<bool>,
}

/// A reaction tally on a newsletter message.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterReactionCountResult {
    pub code: String,
    pub count: f64,
}

/// One message from `newsletterMessages`.
///
/// `serverId` is the pagination cursor and the key `newsletterReactMessage`
/// uses; `messageId` is what edit and revoke key on. They are different ids and
/// both cross as strings, since a `serverId` is a u64.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterMessageResult {
    pub message_id: String,
    pub server_id: String,
    pub timestamp: f64,
    pub message_type: String,
    pub is_sender: bool,
    /// The decoded protobuf, re-encoded. Absent when the stanza carried none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_bytes::ByteBuf>,
    pub reactions: Vec<NewsletterReactionCountResult>,
}
