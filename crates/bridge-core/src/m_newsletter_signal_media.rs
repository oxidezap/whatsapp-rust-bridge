//! Engine-agnostic ports of the `newsletter_signal_media` batch:
//! newsletter CRUD/reactions, Signal session/encrypt/decrypt primitives, usync
//! device discovery, participant-node creation, media conn/download/upload,
//! media reupload, plus the two standalone poll-vote helpers.
//!
//! Logic is reproduced 1:1 from the old `src/wasm_client.rs`; only the JS-interop
//! boundary changed: dynamic JS values become [`BridgeValue`] (built manually for
//! `BinaryNode`/usync-device shapes to match the old `node_to_js` output exactly),
//! and typed inputs (`MediaType`, `PollVoterEntry`) arrive as `serde_json::Value`.

use wacore_binary::jid::Jid;
use wacore_binary::node::{Node, NodeContent};

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::methods::snake;
use crate::value::BridgeValue;
use crate::{helpers, result_types};

/// Convert a wacore `Node` → a `BridgeValue` `BinaryNode` object
/// `{ tag, attrs: Record<string,string>, content? }`. Reproduces the old
/// `node_to_js` 1:1 (attrs flattened to string→string, content as nested
/// nodes / `Uint8Array` bytes / string).
fn node_to_bridge(node: &Node) -> BridgeValue {
    let mut fields: Vec<(String, BridgeValue)> = Vec::with_capacity(3);
    fields.push(("tag".to_string(), BridgeValue::str(node.tag.as_ref())));

    let attrs: Vec<(String, BridgeValue)> = node
        .attrs
        .iter()
        .map(|(key, value)| {
            (
                key.as_ref().to_string(),
                BridgeValue::Str(value.as_str().into_owned()),
            )
        })
        .collect();
    fields.push(("attrs".to_string(), BridgeValue::Object(attrs)));

    if let Some(content) = &node.content {
        let content_bv = match content {
            NodeContent::Nodes(nodes) => {
                BridgeValue::Array(nodes.iter().map(node_to_bridge).collect())
            }
            NodeContent::Bytes(bytes) => BridgeValue::Bytes(bytes.clone()),
            NodeContent::String(s) => BridgeValue::Str(s.to_string()),
        };
        fields.push(("content".to_string(), content_bv));
    }

    BridgeValue::Object(fields)
}

/// Convert `NewsletterMetadata` → the typed result struct (ported from the old
/// `newsletter_metadata_to_result`; engine-agnostic, so it lives inline here).
fn newsletter_metadata_to_result(
    meta: &whatsapp_rust::features::NewsletterMetadata,
) -> result_types::NewsletterMetadataResult {
    result_types::NewsletterMetadataResult {
        jid: meta.jid.to_string(),
        name: meta.name.to_string(),
        description: meta.description.clone(),
        subscriber_count: meta.subscriber_count as f64,
        verification: format!("{:?}", meta.verification),
        state: format!("{:?}", meta.state),
        picture_url: meta.picture_url.clone(),
        preview_url: meta.preview_url.clone(),
        invite_code: meta.invite_code.clone(),
        role: meta.role.as_ref().map(|r| format!("{:?}", r)),
        creation_time: meta.creation_time.map(|v| v as f64),
    }
}

/// Deserialize the `MediaType` string-enum from the JS boundary value.
fn parse_media_type(input: serde_json::Value) -> Result<result_types::MediaType, BridgeError> {
    serde_json::from_value(input).map_err(|e| errors::invalid_arg("media_type", e.to_string()))
}

impl WhatsAppClientCore {
    // ── Newsletter ────────────────────────────────────────────────────────

    pub async fn newsletter_create(
        &self,
        name: &str,
        description: Option<String>,
    ) -> Result<BridgeValue, BridgeError> {
        let result = self
            .client
            .newsletter()
            .create(name, description.as_deref())
            .await?;
        snake(&newsletter_metadata_to_result(&result))
    }

    pub async fn newsletter_metadata(&self, jid: &str) -> Result<BridgeValue, BridgeError> {
        let target = helpers::parse_jid(jid)?;
        let result = self.client.newsletter().get_metadata(&target).await?;
        snake(&newsletter_metadata_to_result(&result))
    }

    pub async fn newsletter_subscribe(&self, jid: &str) -> Result<BridgeValue, BridgeError> {
        let target = helpers::parse_jid(jid)?;
        let result = self.client.newsletter().join(&target).await?;
        snake(&newsletter_metadata_to_result(&result))
    }

    pub async fn newsletter_unsubscribe(&self, jid: &str) -> Result<(), BridgeError> {
        let target = helpers::parse_jid(jid)?;
        self.client
            .newsletter()
            .leave(&target)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn newsletter_react_message(
        &self,
        jid: &str,
        server_id: &str,
        reaction: Option<String>,
    ) -> Result<(), BridgeError> {
        let target = helpers::parse_jid(jid)?;
        let sid: u64 = server_id
            .parse()
            .map_err(|e| errors::internal(format!("invalid server_id: {e}")))?;
        self.client
            .newsletter()
            .send_reaction(&target, sid, reaction.as_deref().unwrap_or(""))
            .await
            .map_err(BridgeError::from)
    }

    // ── Media reupload ──────────────────────────────────────────────────────

    pub async fn request_media_reupload(
        &self,
        msg_id: &str,
        chat_jid: &str,
        media_key: &[u8],
        is_from_me: bool,
        participant: Option<String>,
    ) -> Result<String, BridgeError> {
        let chat = helpers::parse_jid(chat_jid)?;
        let participant_jid = participant.as_deref().map(helpers::parse_jid).transpose()?;

        let req = whatsapp_rust::MediaReuploadRequest {
            msg_id,
            chat_jid: &chat,
            media_key,
            is_from_me,
            participant: participant_jid.as_ref(),
        };

        let result = self.client.media_reupload().request(&req).await?;

        match result {
            whatsapp_rust::MediaRetryResult::Success { direct_path } => Ok(direct_path),
            whatsapp_rust::MediaRetryResult::NotFound => {
                Err(errors::internal("Media not found on server"))
            }
            whatsapp_rust::MediaRetryResult::DecryptionError => {
                Err(errors::internal("Media decryption error"))
            }
            whatsapp_rust::MediaRetryResult::GeneralError => {
                Err(errors::internal("Media reupload failed"))
            }
        }
    }

    // ── Media ───────────────────────────────────────────────────────────────

    pub async fn get_media_conn(&self, force: bool) -> Result<BridgeValue, BridgeError> {
        let conn = self.client.refresh_media_conn(force).await?;
        let result = result_types::MediaConnResult {
            auth: conn.auth.clone(),
            ttl: conn.ttl as f64,
            hosts: conn
                .hosts
                .iter()
                .map(|h| result_types::MediaHost {
                    hostname: h.hostname.clone(),
                })
                .collect(),
        };
        snake(&result)
    }

    pub async fn download_media(
        &self,
        direct_path: &str,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        media_type: serde_json::Value,
    ) -> Result<Vec<u8>, BridgeError> {
        let mt: wacore::download::MediaType = parse_media_type(media_type)?.into();
        let params = whatsapp_rust::download::DownloadParams::encrypted(
            direct_path,
            media_key,
            file_sha256,
            file_enc_sha256,
            file_length as u64,
            mt,
        );
        let data = self.client.download_from_params(&params).await?;
        Ok(data)
    }

    pub async fn upload_media(
        &self,
        data: &[u8],
        media_type: serde_json::Value,
    ) -> Result<BridgeValue, BridgeError> {
        let mt: wacore::download::MediaType = parse_media_type(media_type)?.into();
        let resp = self
            .client
            .upload(data.to_vec(), mt, Default::default())
            .await?;
        let result = result_types::UploadMediaResult {
            url: resp.url,
            direct_path: resp.direct_path,
            media_key: resp.media_key,
            file_sha256: resp.file_sha256,
            file_enc_sha256: resp.file_enc_sha256,
            file_length: resp.file_length as f64,
        };
        snake(&result)
    }

    // ── Sessions / usync ──────────────────────────────────────────────────────

    pub async fn assert_sessions(
        &self,
        jids: Vec<String>,
        _force: bool,
    ) -> Result<bool, BridgeError> {
        let parsed: Vec<Jid> = jids
            .iter()
            .map(|j| helpers::parse_jid(j))
            .collect::<Result<_, _>>()?;
        self.client.signal().assert_sessions(&parsed).await?;
        Ok(true)
    }

    pub async fn get_usync_devices(
        &self,
        jids: Vec<String>,
        _use_cache: bool,
        _ignore_zero_devices: bool,
    ) -> Result<BridgeValue, BridgeError> {
        let parsed: Vec<Jid> = jids
            .iter()
            .map(|j| helpers::parse_jid(j))
            .collect::<Result<_, _>>()?;
        let devices = self.client.signal().get_user_devices(&parsed).await?;
        // Return as JidWithDevice[] = { user: string, device?: number, jid: string }
        let arr: Vec<BridgeValue> = devices
            .iter()
            .map(|jid| {
                let mut fields: Vec<(String, BridgeValue)> = Vec::with_capacity(3);
                fields.push(("user".to_string(), BridgeValue::str(jid.user.as_str())));
                if jid.device != 0 {
                    fields.push(("device".to_string(), BridgeValue::F64(jid.device as f64)));
                }
                fields.push(("jid".to_string(), BridgeValue::str(jid.to_string())));
                BridgeValue::Object(fields)
            })
            .collect();
        Ok(BridgeValue::Array(arr))
    }

    // ── Signal protocol ──────────────────────────────────────────────────────

    pub async fn signal_encrypt_message(
        &self,
        jid: &str,
        data: &[u8],
    ) -> Result<BridgeValue, BridgeError> {
        let parsed = helpers::parse_jid(jid)?;
        let (msg_type, ciphertext) = self.client.signal().encrypt_message(&parsed, data).await?;
        Ok(BridgeValue::object([
            ("type".to_string(), BridgeValue::str(msg_type.as_wire_str())),
            ("ciphertext".to_string(), BridgeValue::Bytes(ciphertext)),
        ]))
    }

    pub async fn signal_decrypt_message(
        &self,
        jid: &str,
        msg_type: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, BridgeError> {
        let parsed = helpers::parse_jid(jid)?;
        let enc_type = wacore::message_processing::EncType::from_wire(msg_type)
            .ok_or_else(|| errors::internal(format!("invalid msg_type: {msg_type}")))?;
        let plaintext = self
            .client
            .signal()
            .decrypt_message(&parsed, enc_type, ciphertext)
            .await?;
        Ok(plaintext)
    }

    pub async fn signal_encrypt_group_message(
        &self,
        group_jid: &str,
        data: &[u8],
        _me_id: &str,
    ) -> Result<BridgeValue, BridgeError> {
        let parsed = helpers::parse_jid(group_jid)?;
        let (skdm, ciphertext) = self
            .client
            .signal()
            .encrypt_group_message(&parsed, data)
            .await?;
        let skdm_bv = match skdm {
            Some(bytes) => BridgeValue::Bytes(bytes),
            None => BridgeValue::Null,
        };
        Ok(BridgeValue::object([
            ("senderKeyDistributionMessage".to_string(), skdm_bv),
            ("ciphertext".to_string(), BridgeValue::Bytes(ciphertext)),
        ]))
    }

    pub async fn signal_decrypt_group_message(
        &self,
        group_jid: &str,
        author_jid: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, BridgeError> {
        let group = helpers::parse_jid(group_jid)?;
        let sender = helpers::parse_jid(author_jid)?;
        let plaintext = self
            .client
            .signal()
            .decrypt_group_message(&group, &sender, msg)
            .await?;
        Ok(plaintext)
    }

    pub async fn signal_validate_session(&self, jid: &str) -> Result<bool, BridgeError> {
        let parsed = helpers::parse_jid(jid)?;
        self.client
            .signal()
            .validate_session(&parsed)
            .await
            .map_err(BridgeError::from)
    }

    pub async fn signal_delete_sessions(&self, jids: Vec<String>) -> Result<(), BridgeError> {
        let parsed: Vec<Jid> = jids
            .iter()
            .map(|j| helpers::parse_jid(j))
            .collect::<Result<_, _>>()?;
        self.client
            .signal()
            .delete_sessions(&parsed)
            .await
            .map_err(BridgeError::from)
    }

    // ── Participant node creation ──────────────────────────────────────────────

    pub async fn create_participant_nodes_bytes(
        &self,
        jids: Vec<String>,
        bytes: &[u8],
    ) -> Result<BridgeValue, BridgeError> {
        use prost::Message;
        let recipient_jids: Vec<Jid> = jids
            .iter()
            .map(|j| helpers::parse_jid(j))
            .collect::<Result<_, _>>()?;

        let msg = waproto::whatsapp::Message::decode(bytes)
            .map_err(|e| errors::internal(format!("invalid message bytes: {e}")))?;

        let (nodes, should_include_device_identity) = self
            .client
            .signal()
            .create_participant_nodes(&recipient_jids, &msg)
            .await?;

        let nodes_bv = BridgeValue::Array(nodes.iter().map(node_to_bridge).collect());
        Ok(BridgeValue::object([
            ("nodes".to_string(), nodes_bv),
            (
                "shouldIncludeDeviceIdentity".to_string(),
                BridgeValue::Bool(should_include_device_identity),
            ),
        ]))
    }
}

// ---------------------------------------------------------------------------
// Poll vote decryption — standalone functions (not on WhatsAppClientCore)
// ---------------------------------------------------------------------------

/// Decrypt a poll vote. Returns selected option names.
pub fn decrypt_poll_vote(
    enc_payload: &[u8],
    enc_iv: &[u8],
    message_secret: &[u8],
    poll_msg_id: &str,
    poll_creator_jid: &str,
    voter_jid: &str,
    option_names: Vec<String>,
) -> Result<Vec<String>, BridgeError> {
    let creator = helpers::parse_jid(poll_creator_jid)?;
    let voter = helpers::parse_jid(voter_jid)?;

    // Client-less context: no LID/PN swap fallback available (that needs a
    // `Client`), so call the underlying primitive directly with `None`.
    let creator_str = creator.to_non_ad().to_string();
    let voter_str = voter.to_non_ad().to_string();
    let selected_hashes = wacore::poll::decrypt_poll_vote_with_fallback(
        wacore::poll::PollVoteCiphertext {
            enc_payload,
            enc_iv,
        },
        message_secret,
        poll_msg_id,
        wacore::poll::PollVoteAddressing {
            poll_creator_jid: &creator_str,
            voter_jid: &voter_str,
        },
        None,
    )?;

    // Map hashes back to option names
    let option_map: Vec<([u8; 32], &str)> = option_names
        .iter()
        .map(|n| (wacore::poll::compute_option_hash(n), n.as_str()))
        .collect();

    let mut result = Vec::new();
    for hash in &selected_hashes {
        if let Ok(hash_arr) = <[u8; 32]>::try_from(hash.as_slice())
            && let Some((_, name)) = option_map.iter().find(|(h, _)| *h == hash_arr)
        {
            result.push(name.to_string());
        }
    }
    Ok(result)
}

/// Aggregate all votes for a poll. Returns a `PollAggregateResult` per option,
/// serialized to `BridgeValue`.
pub fn get_aggregate_votes_in_poll_message(
    option_names: Vec<String>,
    voters: Vec<result_types::PollVoterEntry>,
    message_secret: &[u8],
    poll_msg_id: &str,
    poll_creator_jid: &str,
) -> Result<Vec<BridgeValue>, BridgeError> {
    use std::collections::HashMap;

    let creator = helpers::parse_jid(poll_creator_jid)?;
    let creator_str = creator.to_non_ad().to_string();

    let option_hashes: Vec<[u8; 32]> = option_names
        .iter()
        .map(|name| wacore::poll::compute_option_hash(name))
        .collect();

    // Client-less aggregation: mirrors core `Polls::aggregate_votes` last-vote-wins
    // semantics, minus the LID/PN swap fallback (which requires a `Client`).
    // Keyed by the as-received non-AD voter JID; votes are oldest-first.
    let mut latest_votes: HashMap<String, Vec<Vec<u8>>> = HashMap::with_capacity(voters.len());
    for v in voters {
        let voter_jid: Jid = v.voter.parse()?;
        let voter_str = voter_jid.to_non_ad().to_string();
        match wacore::poll::decrypt_poll_vote_with_fallback(
            wacore::poll::PollVoteCiphertext {
                enc_payload: &v.enc_payload,
                enc_iv: &v.enc_iv,
            },
            message_secret,
            poll_msg_id,
            wacore::poll::PollVoteAddressing {
                poll_creator_jid: &creator_str,
                voter_jid: &voter_str,
            },
            None,
        ) {
            Ok(selected_hashes) if selected_hashes.is_empty() => {
                latest_votes.remove(&voter_str);
            }
            Ok(selected_hashes) => {
                latest_votes.insert(voter_str, selected_hashes);
            }
            Err(e) => log::warn!("Failed to decrypt vote from {voter_jid}: {e}"),
        }
    }

    // Move each option name into its result bucket (option_names is no longer
    // borrowed once option_hashes is built).
    let mut results: Vec<result_types::PollAggregateResult> = option_names
        .into_iter()
        .map(|name| result_types::PollAggregateResult {
            name,
            voters: Vec::new(),
        })
        .collect();

    // Consume the map so each voter's String moves into its option bucket;
    // only a voter who picked multiple options pays for a clone.
    for (voter_str, selected_hashes) in latest_votes {
        let mut matched = selected_hashes.iter().filter_map(|hash| {
            let hash_arr = <[u8; 32]>::try_from(hash.as_slice()).ok()?;
            option_hashes.iter().position(|h| *h == hash_arr)
        });
        if let Some(first) = matched.next() {
            for idx in matched {
                results[idx].voters.push(voter_str.clone());
            }
            results[first].voters.push(voter_str);
        }
    }

    results.iter().map(snake).collect()
}
