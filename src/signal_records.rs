//! Typed binary boundary for portable Signal record components.

use js_sys::Uint8Array;
use serde::{Deserialize, Serialize};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::libsignal::protocol::{
    PendingKeyExchangeComponents as CorePendingKeyExchangeComponents,
    PendingPreKeyComponents as CorePendingPreKeyComponents,
    SenderChainKeyComponents as CoreSenderChainKeyComponents, SenderKeyRecord,
    SenderKeyRecordComponents as CoreSenderKeyRecordComponents,
    SenderKeyStateComponents as CoreSenderKeyStateComponents,
    SenderMessageKeyComponents as CoreSenderMessageKeyComponents,
    SenderSigningKeyComponents as CoreSenderSigningKeyComponents,
    SessionChainComponents as CoreSessionChainComponents,
    SessionChainKeyComponents as CoreSessionChainKeyComponents,
    SessionComponents as CoreSessionComponents,
    SessionMessageKeyComponents as CoreSessionMessageKeyComponents,
    SessionMessageKeyMaterial as CoreSessionMessageKeyMaterial, SessionRecord,
    SessionRecordComponents as CoreSessionRecordComponents,
};

use crate::wasm_utils::{byte_array, error_value};

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecordComponents {
    pub current_session: Option<SessionComponents>,
    pub previous_sessions: Vec<SessionComponents>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SessionComponents {
    pub session_version: Option<u32>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_identity_public: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub remote_identity_public: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub root_key: Option<Vec<u8>>,
    pub previous_counter: Option<u32>,
    #[tsify(type = "SenderSessionChainComponents | undefined")]
    pub sender_chain: Option<SessionChainComponents>,
    #[tsify(type = "ReceiverSessionChainComponents[]")]
    pub receiver_chains: Vec<SessionChainComponents>,
    pub pending_key_exchange: Option<PendingKeyExchangeComponents>,
    pub pending_pre_key: Option<PendingPreKeyComponents>,
    pub remote_registration_id: Option<u32>,
    pub local_registration_id: Option<u32>,
    pub needs_refresh: Option<bool>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub alice_base_key: Option<Vec<u8>>,
}

// Declaration-only role: runtime conversion stays on the shared component DTO.
#[derive(Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SenderSessionChainComponents {
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub sender_ratchet_key: Vec<u8>,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub sender_ratchet_key_private: Vec<u8>,
    pub chain_key: RequiredSessionChainKeyComponents,
    pub message_keys: Vec<SessionMessageKeyComponents>,
}

// Declaration-only role: the private-key field is intentionally absent.
#[derive(Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverSessionChainComponents {
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub sender_ratchet_key: Option<Vec<u8>>,
    pub chain_key: Option<SessionChainKeyComponents>,
    pub message_keys: Vec<SessionMessageKeyComponents>,
}

#[derive(Tsify)]
#[serde(rename_all = "camelCase")]
pub struct RequiredSessionChainKeyComponents {
    pub index: u32,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChainComponents {
    #[serde(with = "serde_bytes")]
    pub sender_ratchet_key: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    pub sender_ratchet_key_private: Option<Vec<u8>>,
    pub chain_key: Option<SessionChainKeyComponents>,
    pub message_keys: Vec<SessionMessageKeyComponents>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SessionChainKeyComponents {
    pub index: Option<u32>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub key: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageKeyComponents {
    pub index: u32,
    pub material: SessionMessageKeyMaterial,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SessionMessageKeyMaterial {
    Seed {
        #[tsify(type = "Uint8Array")]
        #[serde(with = "serde_bytes")]
        seed: Vec<u8>,
    },
    Derived {
        #[tsify(type = "Uint8Array")]
        #[serde(with = "serde_bytes")]
        cipher_key: Vec<u8>,
        #[tsify(type = "Uint8Array")]
        #[serde(with = "serde_bytes")]
        mac_key: Vec<u8>,
        #[tsify(type = "Uint8Array")]
        #[serde(with = "serde_bytes")]
        iv: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PendingKeyExchangeComponents {
    pub sequence: Option<u32>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_base_key: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_base_key_private: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_ratchet_key: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_ratchet_key_private: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_identity_key: Option<Vec<u8>>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub local_identity_key_private: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PendingPreKeyComponents {
    pub pre_key_id: Option<u32>,
    pub signed_pre_key_id: Option<i32>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub base_key: Option<Vec<u8>>,
    pub kyber_pre_key_id: Option<u32>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub kyber_ciphertext: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SenderKeyRecordComponents {
    pub states: Vec<SenderKeyStateComponents>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SenderKeyStateComponents {
    pub key_id: u32,
    pub chain_key: SenderChainKeyComponents,
    pub signing_key: SenderSigningKeyComponents,
    pub message_keys: Vec<SenderMessageKeyComponents>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SenderChainKeyComponents {
    pub iteration: u32,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub seed: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SenderSigningKeyComponents {
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub public: Vec<u8>,
    #[tsify(type = "Uint8Array | undefined")]
    #[serde(with = "serde_bytes")]
    pub private: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct SenderMessageKeyComponents {
    pub iteration: u32,
    #[tsify(type = "Uint8Array")]
    #[serde(with = "serde_bytes")]
    pub seed: Vec<u8>,
}

/// The core models skipped-message-key material as fixed-width arrays. These
/// bytes arrive from JS, so the width is checked here and reported rather than
/// truncated or panicked on.
fn key_material<const N: usize>(value: Vec<u8>, field: &'static str) -> Result<[u8; N], JsValue> {
    let len = value.len();
    <[u8; N]>::try_from(value)
        .map_err(|_| error_value(format!("{field} must be {N} bytes, got {len}")))
}

impl TryFrom<SessionMessageKeyMaterial> for CoreSessionMessageKeyMaterial {
    type Error = JsValue;

    fn try_from(value: SessionMessageKeyMaterial) -> Result<Self, Self::Error> {
        Ok(match value {
            SessionMessageKeyMaterial::Seed { seed } => Self::Seed(key_material(seed, "seed")?),
            SessionMessageKeyMaterial::Derived {
                cipher_key,
                mac_key,
                iv,
            } => Self::Derived {
                cipher_key: key_material(cipher_key, "cipherKey")?,
                mac_key: key_material(mac_key, "macKey")?,
                iv: key_material(iv, "iv")?,
            },
        })
    }
}

impl From<CoreSessionMessageKeyMaterial> for SessionMessageKeyMaterial {
    fn from(value: CoreSessionMessageKeyMaterial) -> Self {
        match value {
            CoreSessionMessageKeyMaterial::Seed(seed) => Self::Seed {
                seed: seed.to_vec(),
            },
            CoreSessionMessageKeyMaterial::Derived {
                cipher_key,
                mac_key,
                iv,
            } => Self::Derived {
                cipher_key: cipher_key.to_vec(),
                mac_key: mac_key.to_vec(),
                iv: iv.to_vec(),
            },
        }
    }
}

macro_rules! impl_component_conversion {
    ($local:ty, $core:ty, { $($field:ident),+ $(,)? }) => {
        impl From<$local> for $core {
            fn from(value: $local) -> Self {
                Self { $($field: value.$field.into()),+ }
            }
        }

        impl From<$core> for $local {
            fn from(value: $core) -> Self {
                Self { $($field: value.$field.into()),+ }
            }
        }
    };
}

// Not the macro: the JS-to-core direction validates the material's width.
impl TryFrom<SessionMessageKeyComponents> for CoreSessionMessageKeyComponents {
    type Error = JsValue;

    fn try_from(value: SessionMessageKeyComponents) -> Result<Self, Self::Error> {
        Ok(Self {
            index: value.index,
            material: value.material.try_into()?,
        })
    }
}

impl From<CoreSessionMessageKeyComponents> for SessionMessageKeyComponents {
    fn from(value: CoreSessionMessageKeyComponents) -> Self {
        Self {
            index: value.index,
            material: value.material.into(),
        }
    }
}

impl_component_conversion!(SessionChainKeyComponents, CoreSessionChainKeyComponents, {
    index,
    key
});
impl_component_conversion!(PendingKeyExchangeComponents, CorePendingKeyExchangeComponents, {
    sequence,
    local_base_key,
    local_base_key_private,
    local_ratchet_key,
    local_ratchet_key_private,
    local_identity_key,
    local_identity_key_private
});
impl_component_conversion!(PendingPreKeyComponents, CorePendingPreKeyComponents, {
    pre_key_id,
    signed_pre_key_id,
    base_key,
    kyber_pre_key_id,
    kyber_ciphertext
});
impl_component_conversion!(SenderChainKeyComponents, CoreSenderChainKeyComponents, {
    iteration,
    seed
});
impl_component_conversion!(SenderSigningKeyComponents, CoreSenderSigningKeyComponents, {
    public,
    private
});
impl_component_conversion!(SenderMessageKeyComponents, CoreSenderMessageKeyComponents, {
    iteration,
    seed
});
impl TryFrom<SessionChainComponents> for CoreSessionChainComponents {
    type Error = JsValue;

    fn try_from(value: SessionChainComponents) -> Result<Self, Self::Error> {
        Ok(Self {
            sender_ratchet_key: value.sender_ratchet_key,
            sender_ratchet_key_private: value.sender_ratchet_key_private,
            chain_key: value.chain_key.map(Into::into),
            message_keys: value
                .message_keys
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<CoreSessionChainComponents> for SessionChainComponents {
    fn from(value: CoreSessionChainComponents) -> Self {
        Self {
            sender_ratchet_key: value.sender_ratchet_key,
            sender_ratchet_key_private: value.sender_ratchet_key_private,
            chain_key: value.chain_key.map(Into::into),
            message_keys: value.message_keys.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<SessionComponents> for CoreSessionComponents {
    type Error = JsValue;

    fn try_from(value: SessionComponents) -> Result<Self, Self::Error> {
        Ok(Self {
            session_version: value.session_version,
            local_identity_public: value.local_identity_public,
            remote_identity_public: value.remote_identity_public,
            root_key: value.root_key,
            previous_counter: value.previous_counter,
            sender_chain: value.sender_chain.map(TryInto::try_into).transpose()?,
            receiver_chains: value
                .receiver_chains
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            pending_key_exchange: value.pending_key_exchange.map(Into::into),
            pending_pre_key: value.pending_pre_key.map(Into::into),
            remote_registration_id: value.remote_registration_id,
            local_registration_id: value.local_registration_id,
            needs_refresh: value.needs_refresh,
            alice_base_key: value.alice_base_key,
        })
    }
}

impl From<CoreSessionComponents> for SessionComponents {
    fn from(value: CoreSessionComponents) -> Self {
        Self {
            session_version: value.session_version,
            local_identity_public: value.local_identity_public,
            remote_identity_public: value.remote_identity_public,
            root_key: value.root_key,
            previous_counter: value.previous_counter,
            sender_chain: value.sender_chain.map(Into::into),
            receiver_chains: value.receiver_chains.into_iter().map(Into::into).collect(),
            pending_key_exchange: value.pending_key_exchange.map(Into::into),
            pending_pre_key: value.pending_pre_key.map(Into::into),
            remote_registration_id: value.remote_registration_id,
            local_registration_id: value.local_registration_id,
            needs_refresh: value.needs_refresh,
            alice_base_key: value.alice_base_key,
        }
    }
}

impl TryFrom<SessionRecordComponents> for CoreSessionRecordComponents {
    type Error = JsValue;

    fn try_from(value: SessionRecordComponents) -> Result<Self, Self::Error> {
        Ok(Self {
            current_session: value.current_session.map(TryInto::try_into).transpose()?,
            previous_sessions: value
                .previous_sessions
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<CoreSessionRecordComponents> for SessionRecordComponents {
    fn from(value: CoreSessionRecordComponents) -> Self {
        Self {
            current_session: value.current_session.map(Into::into),
            previous_sessions: value
                .previous_sessions
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<SenderKeyStateComponents> for CoreSenderKeyStateComponents {
    fn from(value: SenderKeyStateComponents) -> Self {
        Self {
            key_id: value.key_id,
            chain_key: value.chain_key.into(),
            signing_key: value.signing_key.into(),
            message_keys: value.message_keys.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<CoreSenderKeyStateComponents> for SenderKeyStateComponents {
    fn from(value: CoreSenderKeyStateComponents) -> Self {
        Self {
            key_id: value.key_id,
            chain_key: value.chain_key.into(),
            signing_key: value.signing_key.into(),
            message_keys: value.message_keys.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<SenderKeyRecordComponents> for CoreSenderKeyRecordComponents {
    fn from(value: SenderKeyRecordComponents) -> Self {
        Self {
            states: value.states.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<CoreSenderKeyRecordComponents> for SenderKeyRecordComponents {
    fn from(value: CoreSenderKeyRecordComponents) -> Self {
        Self {
            states: value.states.into_iter().map(Into::into).collect(),
        }
    }
}

#[wasm_bindgen(js_name = encodeSessionRecordComponents)]
pub fn encode_session_record_components(
    value: Ts<SessionRecordComponents>,
) -> Result<Uint8Array, JsValue> {
    let value = value.to_rust().map_err(error_value)?;
    let bytes = SessionRecord::from_components(value.try_into()?)
        .and_then(|record| record.serialize())
        .map_err(error_value)?;
    Ok(byte_array(&bytes))
}

#[wasm_bindgen(js_name = decodeSessionRecordComponents)]
pub fn decode_session_record_components(
    bytes: &[u8],
) -> Result<Ts<SessionRecordComponents>, JsValue> {
    let components = SessionRecord::deserialize(bytes)
        .and_then(SessionRecord::into_components)
        .map_err(error_value)?;
    let result: SessionRecordComponents = components.into();
    result.into_ts().map_err(error_value)
}

#[wasm_bindgen(js_name = encodeSenderKeyRecordComponents)]
pub fn encode_sender_key_record_components(
    value: Ts<SenderKeyRecordComponents>,
) -> Result<Uint8Array, JsValue> {
    let value = value.to_rust().map_err(error_value)?;
    let bytes = SenderKeyRecord::from_components(value.into())
        .and_then(|record| record.serialize())
        .map_err(error_value)?;
    Ok(byte_array(&bytes))
}

#[wasm_bindgen(js_name = decodeSenderKeyRecordComponents)]
pub fn decode_sender_key_record_components(
    bytes: &[u8],
) -> Result<Ts<SenderKeyRecordComponents>, JsValue> {
    let components = SenderKeyRecord::deserialize(bytes)
        .and_then(SenderKeyRecord::into_components)
        .map_err(error_value)?;
    let result: SenderKeyRecordComponents = components.into();
    result.into_ts().map_err(error_value)
}
