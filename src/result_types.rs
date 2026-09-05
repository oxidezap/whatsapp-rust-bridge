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
#[serde(rename_all = "snake_case")]
pub enum BlockAction {
    Block,
    Unblock,
}

/// Presence status.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Available,
    Unavailable,
}

/// Chat state (typing indicator).
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Composing,
    Recording,
    Paused,
}

/// Group participant action.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
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
#[serde(rename_all = "snake_case")]
pub enum GroupSetting {
    Locked,
    Announce,
    MembershipApproval,
}

/// Group member add mode.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[serde(rename_all = "snake_case")]
pub enum MemberAddMode {
    AdminAdd,
    AllMemberAdd,
}

/// Picture type for profile picture URL.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
#[serde(rename_all = "snake_case")]
pub enum PictureType {
    Preview,
    Image,
}

/// Group join request action.
#[derive(Debug, Clone, Copy, Deserialize, Tsify)]
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

/// What the client's connection state means for work handed to it now.
///
/// The core's `Reachability`, carried across unflattened. A refused call says
/// what happened to that attempt; this says what the client is, which is the
/// only thing that answers whether asking again is worth it — so it is read
/// when the question comes up rather than stamped onto an error that is a fact
/// about one instant.
///
/// `unknown` is not one of the core's: the enum is `#[non_exhaustive]`, and a
/// state added upstream has no name here yet. Naming the gap beats reporting it
/// as one of its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Tsify)]
#[serde(rename_all = "kebab-case")]
pub enum Reachability {
    /// A request sent now has a socket, an authenticated session and a reader
    /// to decode the answer.
    Reachable,
    /// Between connections, with something driving the client back to one.
    /// Nothing is re-sent by that: recovery restores the ability to ask, not
    /// the request that was refused.
    Reconnecting,
    /// A pause is in effect. Not finished, but what ends it is a resume, and
    /// only the application knows when that comes.
    Paused,
    /// Nothing is reading this client, so no answer would be decoded and no
    /// reconnect will be attempted.
    Unsupervised,
    /// The session is over for good: shut down, logged out, replaced, or
    /// refused in a way no reconnect recovers.
    Finished,
    /// A state this version of the bridge has no name for.
    Unknown,
}

impl From<whatsapp_rust::Reachability> for Reachability {
    fn from(state: whatsapp_rust::Reachability) -> Self {
        use whatsapp_rust::Reachability as R;
        match state {
            R::Reachable => Self::Reachable,
            R::Reconnecting => Self::Reconnecting,
            R::Paused => Self::Paused,
            R::Unsupervised => Self::Unsupervised,
            R::Finished => Self::Finished,
            _ => Self::Unknown,
        }
    }
}

/// Why the supervised run loop started by `run()` ended.
///
/// The core's `RunCompletionReason`, carried across unflattened: the
/// discriminant says which branch ended the run, and only the
/// `auto-reconnect-disabled` branch carries causes. Every cause is typed at
/// the boundary (no `Debug` rendering), and every absence is an absent key.
/// `generation` keys the result to the `run()` call that produced it; a stale
/// task can never overwrite a newer run's result.
///
/// `unknown` is not one of the core's: the enum is `#[non_exhaustive]`, and a
/// reason added upstream has no shape here yet.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "reason")]
pub enum RunCompletionResult {
    /// A terminal shutdown was requested through `disconnect()`, `logout()`
    /// or client teardown.
    #[serde(rename = "shutdown-requested")]
    ShutdownRequested { generation: f64 },
    /// The connection ended while automatic reconnection was disabled.
    /// `connection` is the final reader outcome when a connection was
    /// established; `connectError` is the final connect failure when none
    /// was; `protocolError` is the terminal stream or connect-failure cause
    /// the reader captured, when it captured one.
    #[serde(rename = "auto-reconnect-disabled", rename_all = "camelCase")]
    AutoReconnectDisabled {
        generation: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        connection: Option<DisconnectReasonResult>,
        #[serde(skip_serializing_if = "Option::is_none")]
        connect_error: Option<ConnectErrorResult>,
        #[serde(skip_serializing_if = "Option::is_none")]
        protocol_error: Option<ProtocolTerminalReasonResult>,
    },
    /// The supervision flag was observed cleared without a classified
    /// terminal verdict. Carries no claim about the protocol cause.
    #[serde(rename = "stopped")]
    Stopped { generation: f64 },
    /// Another task already owns the client's read loop.
    #[serde(rename = "already-running")]
    AlreadyRunning { generation: f64 },
    /// A reason this version of the bridge has no shape for. `detail`
    /// carries the core's own rendering so the host still learns what ended
    /// the run instead of receiving a neighbour's meaning.
    #[serde(rename = "unknown", rename_all = "camelCase")]
    Unknown { generation: f64, detail: String },
}

/// Why the transport connection ended, as the final reader observed it.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]
pub enum DisconnectReasonResult {
    /// The peer sent a WebSocket Close frame. `code` is the RFC 6455 close
    /// code; absent when the frame carried none.
    #[serde(rename = "server-close", rename_all = "camelCase")]
    ServerClose {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<f64>,
        reason: String,
    },
    /// The stream ended (EOF) without a Close frame.
    #[serde(rename = "stream-ended")]
    StreamEnded,
    /// A transport-level read/IO error ended the connection.
    #[serde(rename = "read-error", rename_all = "camelCase")]
    ReadError { message: String },
    /// The reason was not reported by the transport.
    #[serde(rename = "unknown")]
    Unknown,
}

/// Why a connection attempt failed, as the final attempt reported it.
///
/// `anyhow` causes cross as their rendered message: the detail is diagnostic
/// text, not a boundary contract.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]
pub enum ConnectErrorResult {
    /// A connection is already up, or another attempt is in flight.
    #[serde(rename = "already-connected")]
    AlreadyConnected,
    /// Construction never completed, so the attempt was rejected before any I/O.
    #[serde(rename = "not-activated")]
    NotActivated,
    /// The client was shut down; build a new client rather than reconnecting.
    #[serde(rename = "shutdown")]
    Shutdown,
    /// A pause is in effect; resume lifts it.
    #[serde(rename = "paused")]
    Paused,
    /// A step of the connect flow ran out of time.
    #[serde(rename = "timeout", rename_all = "camelCase")]
    Timeout { stage: String, timeout_ms: f64 },
    /// The app version could not be resolved.
    #[serde(rename = "version", rename_all = "camelCase")]
    Version { message: String },
    /// The transport factory could not open a connection.
    #[serde(rename = "transport", rename_all = "camelCase")]
    Transport { message: String },
    /// The noise handshake failed after the transport was up.
    #[serde(rename = "handshake", rename_all = "camelCase")]
    Handshake { reason: HandshakeFailureResult },
    /// A failure this version of the bridge has no shape for. `detail`
    /// carries the core's own rendering rather than a guessed kind.
    #[serde(rename = "unknown", rename_all = "camelCase")]
    Unknown { detail: String },
}

/// Which handshake step failed, for a `handshake` connect error.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]
pub enum HandshakeFailureResult {
    /// The transport failed under the handshake.
    #[serde(rename = "transport", rename_all = "camelCase")]
    Transport { message: String },
    /// The core Noise handshake step failed, typed per cause below.
    #[serde(rename = "core", rename_all = "camelCase")]
    Core { reason: NoiseHandshakeFailureResult },
    /// The handshake ran out of time, as opposed to being torn down.
    #[serde(rename = "timeout")]
    Timeout,
    /// The transport event stream closed before the handshake completed.
    #[serde(rename = "stream-closed")]
    StreamClosed,
    /// The client disconnected during the handshake.
    #[serde(rename = "disconnected")]
    Disconnected,
    /// The peer sent something the handshake did not expect.
    #[serde(rename = "unexpected-event", rename_all = "camelCase")]
    UnexpectedEvent { detail: String },
    /// A failure this version of the bridge has no shape for. `detail`
    /// carries the core's own rendering rather than a guessed kind.
    #[serde(rename = "unknown", rename_all = "camelCase")]
    Unknown { detail: String },
}

/// Which core Noise handshake step failed, for a `core` handshake failure.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]
pub enum NoiseHandshakeFailureResult {
    /// The server hello did not decode.
    #[serde(rename = "proto-decode", rename_all = "camelCase")]
    ProtoDecode { message: String },
    /// The handshake response is missing required parts.
    #[serde(rename = "incomplete-response")]
    IncompleteResponse,
    /// A Noise crypto operation failed.
    #[serde(rename = "crypto", rename_all = "camelCase")]
    Crypto { detail: String },
    /// The server certificate chain did not verify.
    #[serde(rename = "cert-verification", rename_all = "camelCase")]
    CertVerification { detail: String },
    /// A handshake field arrived at an unexpected length.
    #[serde(rename = "invalid-length", rename_all = "camelCase")]
    InvalidLength {
        name: String,
        expected: f64,
        got: f64,
    },
    /// A handshake key arrived at an invalid length.
    #[serde(rename = "invalid-key-length")]
    InvalidKeyLength,
    /// The Noise protocol state machine failed.
    #[serde(rename = "noise", rename_all = "camelCase")]
    Noise { message: String },
}

/// Terminal protocol cause the reader captured before its expected-disconnect
/// flag suppressed the transport outcome.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]
pub enum ProtocolTerminalReasonResult {
    /// A `<stream:error>` carrying a numeric code.
    #[serde(rename = "stream-error", rename_all = "camelCase")]
    StreamError { code: f64 },
    /// A `<failure>` stanza; `reason` uses the same spelling the
    /// `connect_failure` event carries.
    #[serde(rename = "connect-failure", rename_all = "camelCase")]
    ConnectFailure { reason: String },
    /// The peer replaced this session (`<conflict>` with no numeric code).
    #[serde(rename = "conflict")]
    Conflict,
    /// A cause this version of the bridge has no shape for. `detail` carries
    /// the core's own rendering rather than a guessed kind.
    #[serde(rename = "unknown", rename_all = "camelCase")]
    Unknown { detail: String },
}

/// Result from `updateProfilePicture` or `removeProfilePicture`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePictureResult {
    pub id: String,
}

/// Result from `profilePictureUrl`.
#[derive(Serialize, Tsify)]
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
#[serde(rename_all = "camelCase")]
pub struct BlocklistEntryResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

/// A single entry from `fetchUserInfo`.
#[derive(Serialize, Tsify)]
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
    /// Meta username, without the display-only `@` prefix. Absent when the
    /// server reported none, which is also how it reports a deleted one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

/// A participant change result from `groupParticipantsUpdate`.
#[derive(Serialize, Tsify)]
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
#[serde(rename_all = "camelCase")]
pub struct ParticipantAddRequestResult {
    pub code: String,
    pub expiration: f64,
}

/// A single media host from `getMediaConn`.
#[derive(Serialize, Tsify)]
pub struct MediaHost {
    pub hostname: String,
}

/// Result from `getMediaConn`.
#[derive(Serialize, Tsify)]
pub struct MediaConnResult {
    pub auth: String,
    pub ttl: f64,
    pub hosts: Vec<MediaHost>,
}

/// Result from `uploadMedia`.
#[derive(Serialize, Tsify)]
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
#[serde(rename_all = "camelCase")]
pub struct SignalSessionInfoResult {
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub base_key: Vec<u8>,
    pub registration_id: u32,
}

/// One linked-identifier to phone-number mapping supplied by the host.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct LidPnMappingInput {
    pub lid: String,
    pub pn: String,
}

/// Counts produced while moving pairwise sessions between identifier namespaces.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SignalSessionMigrationResult {
    pub migrated: u32,
    pub skipped: u32,
    pub total: u32,
}

/// A message key for `readMessages`.
#[derive(Deserialize, Tsify)]
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
    /// Meta username, without the display-only `@` prefix. Absent when the
    /// server reported none, which is also how it reports a deleted one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

/// What `findByUsername` learned about a Meta username.
///
/// Mirrors the core `UsernameLookup`, which is a three-way answer rather than
/// an optional user: a username the server confirms but will not resolve
/// without the account's username key is neither a hit nor a miss.
#[derive(Serialize, Tsify)]
#[serde(tag = "status")]
pub enum UsernameLookupResult {
    /// No account answers to this username, or it is not reachable from here.
    #[serde(rename = "notFound")]
    NotFound,
    /// The username exists and the server withheld the identity behind it.
    /// Repeat the lookup with the account's username key.
    #[serde(rename = "keyRequired", rename_all = "camelCase")]
    KeyRequired {
        /// Username as the server spelled it back, when it did.
        #[serde(skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    /// The username resolved to an account.
    #[serde(rename = "found", rename_all = "camelCase")]
    Found {
        /// Identity the server returned. The query addresses contacts by LID,
        /// so this is normally a LID.
        jid: String,
        /// Phone-number JID, when the server disclosed one on `<business>`.
        #[serde(skip_serializing_if = "Option::is_none")]
        pn_jid: Option<String>,
        /// Username as the server spelled it back.
        #[serde(skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        is_business: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        verified_name: Option<String>,
    },
}

/// Result from `getUsername`: this account's own Meta username.
///
/// Every field is optional because the server omits the ones that do not
/// apply. An account with no username at all comes back as `null` from the
/// method rather than as an all-absent object.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct OwnUsernameResult {
    /// The handle, without the display-only `@` prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// `ACTIVE` or `RESERVED`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The numeric username key that guards lookups of this account by handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Result from `fetchStatus`.
#[derive(Serialize, Tsify)]
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
#[serde(rename_all = "camelCase")]
pub struct GroupEphemeralSettingsResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<f64>,
}

/// Server-managed group growth lock information.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct GroupGrowthLockInfoResult {
    pub lock_type: String,
    pub expiration: f64,
}

/// Result from `getGroupMetadata`.
#[derive(Serialize, Tsify)]
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
#[serde(rename_all = "camelCase")]
pub struct MembershipRequestResult {
    pub jid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_time: Option<f64>,
}

/// A subgroup returned by a parent-group metadata query.
#[derive(Serialize, Tsify)]
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
#[serde(rename_all = "camelCase")]
pub struct CommunityLinkFailureResult {
    pub jid: String,
    pub error: f64,
}

/// Result of linking or unlinking subgroups.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct CommunityLinkResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<CommunityLinkFailureResult>,
}

/// Result from `getBusinessProfile`.
#[derive(Serialize, Tsify)]
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
pub struct BusinessCategoryResult {
    pub id: String,
    pub name: String,
}

/// Business hours.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BusinessHoursResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_config: Option<Vec<BusinessHoursConfigResult>>,
}

/// Business hours config for a day.
#[derive(Serialize, Tsify)]
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
// Business catalog
// ---------------------------------------------------------------------------

/// A price, in thousandths of the currency's main unit.
///
/// WhatsApp scales money by 1000, not 100, and the protobuf field is an
/// `int64`. `amount1000` therefore crosses as a **string**: a `number` is exact
/// only below 2^53, and a large order would be silently wrong rather than
/// rejected. Dividing by 1000 for display is the consumer's decision.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PriceResult {
    /// Thousandths of one currency unit: `"1990"` is 1.99 in `currency`.
    pub amount_1000: String,
    /// ISO 4217 code. Absent when the server sends a price without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// A sale price and the window it applies to.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SalePriceResult {
    pub price: PriceResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ProductImageResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_image_url: Option<String>,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ProductVideoResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_video_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}

/// A postal address, as sent for a product's importer of record.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ImporterAddressResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
}

/// A catalog product. Only `id` is guaranteed; the server omits rather than
/// blanks, so an absent name is absent, not `""`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ProductResult {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retailer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The link-shimmed form of `url`, carried alongside it rather than
    /// instead. Which to open is a consumer's call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shimmed_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<PriceResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sale_price: Option<SalePriceResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_sanctioned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_available: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_appeal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub belongs_to: Option<bool>,
    pub images: Vec<ProductImageResult>,
    pub videos: Vec<ProductVideoResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compliance_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importer_address: Option<ImporterAddressResult>,
}

/// One page of a business catalog.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResult {
    pub products: Vec<ProductResult>,
    /// Feed back as `after` for the next page; absent on the last one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_cursor: Option<String>,
    /// Carried by the response, though the catalog query takes only `after`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_cursor: Option<String>,
}

/// A named group of products within a catalog.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct CollectionResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A prefix, and it says so only by its length: the collections query
    /// carries no per-collection cursor, so `products.length === itemLimit`
    /// means more may exist.
    pub products: Vec<ProductResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_appeal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commerce_url: Option<String>,
}

/// One page of a business's collections.
///
/// Forward cursor only — the collections paging object has no `before`, and
/// the asymmetry with the catalog is the wire's, not an oversight.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct CollectionsResult {
    pub collections: Vec<CollectionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_cursor: Option<String>,
}

/// One dimension of a chosen product variant, e.g. `Size` / `Large`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct VariantPropertyResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// A line item on an order: a snapshot, so the price is what was quoted when
/// the order was placed.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct OrderProductResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<PriceResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<f64>,
    pub images: Vec<ProductImageResult>,
    /// Empty for a product with no variants. Without it a variant order cannot
    /// be fulfilled: id, name and price are shared across one listing's
    /// variants.
    pub variant_properties: Vec<VariantPropertyResult>,
}

#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct OrderPriceDetailsResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtotal: Option<PriceResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<PriceResult>,
}

/// Result from `getOrder`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct OrderResult {
    pub products: Vec<OrderProductResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_details: Option<OrderPriceDetailsResult>,
    /// Unix seconds, as sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<f64>,
}

/// Options for `getCatalog`. Omitted fields take the core's own defaults; no
/// second default is applied here.
#[derive(Default, Deserialize, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogOptionsInput {
    pub limit: Option<u32>,
    /// Cursor from a previous page's `afterCursor`.
    pub after: Option<String>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub allow_shop_source: Option<bool>,
}

/// Options for `getCollections`.
///
/// No `before`: the collections query has no backward cursor, so one accepted
/// here would go nowhere.
#[derive(Default, Deserialize, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct CollectionOptionsInput {
    pub collection_limit: Option<u32>,
    /// Products returned inline per collection.
    pub item_limit: Option<u32>,
    pub after: Option<String>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
}

/// One opening range for a day of the week.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BusinessHoursConfigInput {
    /// `mon`, `tue`, … as the wire spells them.
    pub day_of_week: String,
    /// `open_24h`, `specific_hours` or `appointment_only`.
    pub mode: String,
    /// Minutes past local midnight. Required by `specific_hours` and rejected
    /// for the other modes, by the core.
    pub open_time: Option<u32>,
    pub close_time: Option<u32>,
}

/// Opening hours for `updateBusinessProfile`.
#[derive(Default, Deserialize, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct BusinessHoursUpdateInput {
    /// IANA zone name, e.g. `America/Araguaina`.
    pub timezone: Option<String>,
    pub note: Option<String>,
    pub config: Vec<BusinessHoursConfigInput>,
}

/// A delta on the account's own business profile.
///
/// Absent means "leave alone"; an empty value means "clear" (`""` for text,
/// `[]` for `websites`). The core rejects a delta with nothing set.
#[derive(Default, Deserialize, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct BusinessProfileUpdateInput {
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub description: Option<String>,
    pub email: Option<String>,
    /// At most two URLs. `[]` clears the list.
    pub websites: Option<Vec<String>>,
    /// Category *ids* from the directory, not names.
    pub categories: Option<Vec<String>>,
    pub business_hours: Option<BusinessHoursUpdateInput>,
}

/// The receipt a `biz-cover-photo` upload returns.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct CoverPhotoUploadInput {
    /// `fbid` from the upload response.
    pub id: String,
    /// `meta_hmac` from the upload response.
    pub token: String,
    /// `ts` from the upload response, as a string: it is an i64.
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// Newsletter
// ---------------------------------------------------------------------------

/// One follower of a newsletter, from `newsletterFollowers`.
#[derive(Serialize, Tsify)]
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

// ---------------------------------------------------------------------------
// Bot directory
// ---------------------------------------------------------------------------

/// Result from `getBotList`.
///
/// Sections arrive as the server grouped them. The core keeps every section
/// because a bot can be carried by a `category` or `featured` section and by no
/// other, so flattening is a consumer's decision, not the bridge's.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BotListResult {
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_bot: Option<BotDefaultResult>,
    pub sections: Vec<BotListSectionResult>,
}

/// The bot the server marks as the one to offer by default.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BotDefaultResult {
    pub jid: String,
    pub persona_id: String,
}

/// One section of the bot directory.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BotListSectionResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub section_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_type: Option<String>,
    pub bots: Vec<BotListEntryResult>,
}

/// One bot in the directory.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BotListEntryResult {
    pub jid: String,
    pub persona_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<f64>,
    pub themes: Vec<BotThemeResult>,
}

/// Per-mode colours for a bot's card.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct BotThemeResult {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_text: Option<String>,
}

// ---------------------------------------------------------------------------
// New-chat message capping
// ---------------------------------------------------------------------------

/// Result from `fetchNewChatMessageCappingInfo`.
///
/// Every field is optional because the server omits the ones that do not apply
/// to an account's tier. `remainingQuota` is the core's own derivation, present
/// only when both quota fields are.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct NewChatMessageCappingResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capping_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ote_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mv_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_quota: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_quota: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_start_timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_end_timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_sent_timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_quota: Option<f64>,
}
