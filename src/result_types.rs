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

/// One linked-identifier to phone-number mapping supplied by the host.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct LidPnMappingInput {
    pub lid: String,
    pub pn: String,
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

include!("wasm_client/generated/projections.rs");

#[cfg(all(test, feature = "client-business"))]
mod generated_projection_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn generated_money_and_nested_absence_preserve_values() {
        let price = whatsapp_rust::features::Price {
            amount_1000: i64::MAX,
            currency: None,
        };
        let serialized = serde_json::to_value(project_price(&price)).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({"amount1000": "9223372036854775807"})
        );
        let mut hours = whatsapp_rust::wacore::iq::business::BusinessHours::default();
        assert_eq!(
            serde_json::to_value(project_business_hours(&hours)).unwrap(),
            serde_json::json!({})
        );
        hours.business_config = Some(vec![]);
        assert_eq!(
            serde_json::to_value(project_business_hours(&hours)).unwrap(),
            serde_json::json!({"businessConfig": []})
        );
    }

    #[cfg(feature = "client-signal")]
    #[test]
    fn generated_binary_dto_serializes_as_uint8array() {
        let dto = SignalSessionInfoResult {
            base_key: vec![0, 128, 255],
            registration_id: u32::MAX,
        };
        let value = serde_wasm_bindgen::to_value(&dto).unwrap();
        let bytes = js_sys::Reflect::get(&value, &"baseKey".into()).unwrap();
        use wasm_bindgen::JsCast as _;
        assert!(bytes.is_instance_of::<js_sys::Uint8Array>());
        assert_eq!(js_sys::Uint8Array::new(&bytes).to_vec(), [0, 128, 255]);
    }
}

include!("wasm_client/generated/variant_projections.rs");

include!("wasm_client/generated/diagnostics.rs");
