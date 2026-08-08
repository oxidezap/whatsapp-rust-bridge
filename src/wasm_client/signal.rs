//! Calls, Signal protocol and raw transport.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Calls ────────────────────────────────────────────────────────────

    /// Reject an incoming call.
    #[wasm_bindgen(js_name = rejectCall)]
    pub async fn reject_call(
        &self,
        call_id: &str,
        peer: &str,
        call_creator: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let peer = parse_jid(peer)?;
        let call_creator = parse_jid(call_creator)?;
        self.client
            .voip()
            .reject_call(call_id, &peer, &call_creator)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Signal / low-level protocol ──────────────────────────────────────

    /// Enable or disable raw node forwarding. When enabled, a `raw_node` event
    /// is emitted for every decoded stanza before internal dispatch.
    #[wasm_bindgen(js_name = setRawNodeForwarding)]
    pub fn set_raw_node_forwarding(&self, enabled: bool) {
        let mut lease = self
            .raw_node_lease
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if enabled {
            if lease.is_none() {
                *lease = Some(self.client.acquire_raw_node_forwarding());
            }
        } else {
            lease.take();
        }
    }

    /// Send a raw binary node stanza to WhatsApp servers.
    /// Accepts a JS object matching `{ tag: string, attrs: Record<string, string>, content?: ... }`.
    #[wasm_bindgen(js_name = sendNode)]
    pub async fn send_node(&self, node_js: JsBinaryNode) -> Result<(), crate::errors::BridgeError> {
        let node_js: JsValue = node_js.into();
        let node = js_to_node(&node_js)?;
        self.client
            .send_node(node)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Send an IQ node and return the matching response node.
    #[wasm_bindgen(js_name = queryNode)]
    pub async fn query_node(
        &self,
        node_js: JsBinaryNode,
        timeout_ms: Option<f64>,
    ) -> Result<JsBinaryNode, crate::errors::BridgeError> {
        let node_js: JsValue = node_js.into();
        let node = js_to_node(&node_js)?;
        let timeout = parse_optional_timeout_ms("timeoutMs", timeout_ms)?;
        let response = self.client.send_iq_node(node, timeout).await?;
        Ok(node_ref_to_js(response.get())?.unchecked_into())
    }

    /// Confirm an inbound stanza through the core-owned acknowledgement path.
    #[wasm_bindgen(js_name = acknowledgeStanza)]
    pub async fn acknowledge_stanza(
        &self,
        stanza_js: JsBinaryNode,
    ) -> Result<(), crate::errors::BridgeError> {
        let stanza_js: JsValue = stanza_js.into();
        let stanza = js_to_node(&stanza_js)?;
        self.client
            .acknowledge_stanza(&stanza.as_node_ref())
            .await?;
        Ok(())
    }

    /// Reject an inbound stanza through the core-owned acknowledgement path.
    #[wasm_bindgen(js_name = rejectStanza)]
    pub async fn reject_stanza(
        &self,
        stanza_js: JsBinaryNode,
        error_code: i32,
        failure_reason: Option<i32>,
    ) -> Result<(), crate::errors::BridgeError> {
        let stanza_js: JsValue = stanza_js.into();
        let stanza = js_to_node(&stanza_js)?;
        let reason = whatsapp_rust::NackReason::from(error_code);
        let rejection = if reason == whatsapp_rust::NackReason::InvalidProtobuf {
            whatsapp_rust::StanzaRejection::invalid_protobuf(failure_reason)
        } else {
            whatsapp_rust::StanzaRejection::new(reason)
        };
        self.client
            .reject_stanza(&stanza.as_node_ref(), rejection)
            .await?;
        Ok(())
    }

    /// Request retransmission without acknowledging the original stanza.
    #[wasm_bindgen(js_name = requestMessageRetry)]
    pub async fn request_message_retry(
        &self,
        stanza_js: JsBinaryNode,
        force_include_keys: Option<bool>,
    ) -> Result<(), crate::errors::BridgeError> {
        let stanza_js: JsValue = stanza_js.into();
        let stanza = js_to_node(&stanza_js)?;
        let options = whatsapp_rust::RetryRequestOptions::new()
            .with_force_include_keys(force_include_keys.unwrap_or(false));
        self.client
            .request_message_retry(&stanza.as_node_ref(), options)
            .await?;
        Ok(())
    }

    /// Execute a validated typed USync query through the core-owned operation.
    /// The bridge performs exactly one Serde decode and one Serde encode; it
    /// neither constructs protocol nodes nor translates consumer-facing names.
    #[wasm_bindgen(js_name = queryUsync)]
    pub async fn query_usync(
        &self,
        query: JsUsyncQuery,
    ) -> Result<JsUsyncResponse, crate::errors::BridgeError> {
        let query =
            crate::proto::from_js_value::<whatsapp_rust::usync::UsyncQuery>(query.into(), "query")?;
        let response = self.client.query_usync(query).await?;
        Ok(crate::proto::to_js_value(&response)?.unchecked_into())
    }

    /// Ensure E2E Signal sessions exist for the given JIDs.
    /// Returns true after sessions are established.
    #[wasm_bindgen(js_name = assertSessions)]
    pub async fn assert_sessions(
        &self,
        jids: Vec<String>,
        _force: bool,
    ) -> Result<bool, crate::errors::BridgeError> {
        let parsed: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;
        self.client.signal().assert_sessions(&parsed).await?;
        Ok(true)
    }

    /// Get the list of known devices for the given user JIDs via usync query.
    /// Returns an array of JID strings (one per device).
    #[wasm_bindgen(js_name = getUSyncDevices)]
    pub async fn get_usync_devices(
        &self,
        jids: Vec<String>,
        _use_cache: bool,
        _ignore_zero_devices: bool,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;
        let devices = self.client.signal().get_user_devices(&parsed).await?;
        // Return as JidWithDevice[] = { user: string, device?: number, jid: string }
        let arr = js_sys::Array::new_with_length(devices.len() as u32);
        for (i, jid) in devices.iter().enumerate() {
            let obj = js_sys::Object::new();
            js_sys::Reflect::set(&obj, &"user".into(), &jid.user.as_str().into())
                .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
            if jid.device != 0 {
                js_sys::Reflect::set(&obj, &"device".into(), &(jid.device as f64).into())
                    .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
            }
            js_sys::Reflect::set(&obj, &"jid".into(), &jid.to_string().into())
                .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
            arr.set(i as u32, obj.into());
        }
        Ok(arr.into())
    }

    // ── Signal protocol ──────────────────────────────────────────────────

    /// Encrypt plaintext for a single recipient.
    /// Returns `{ type: "msg"|"pkmsg", ciphertext: Uint8Array }`.
    #[wasm_bindgen(js_name = signalEncryptMessage)]
    pub async fn signal_encrypt_message(
        &self,
        jid: &str,
        data: &[u8],
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed = parse_jid(jid)?;
        let (msg_type, ciphertext) = self.client.signal().encrypt_message(&parsed, data).await?;
        let obj = js_sys::Object::new();
        js_sys::Reflect::set(&obj, &"type".into(), &msg_type.as_wire_str().into())
            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        js_sys::Reflect::set(
            &obj,
            &"ciphertext".into(),
            &js_sys::Uint8Array::from(ciphertext.as_slice()).into(),
        )
        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        Ok(obj.into())
    }

    /// Decrypt a Signal protocol message. `msg_type` is "msg", "pkmsg", or "skmsg".
    #[wasm_bindgen(js_name = signalDecryptMessage)]
    pub async fn signal_decrypt_message(
        &self,
        jid: &str,
        msg_type: &str,
        ciphertext: &[u8],
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let parsed = parse_jid(jid)?;
        let enc_type = wacore::message_processing::EncType::from_wire(msg_type)
            .ok_or_else(|| crate::errors::invalid_arg("msgType", format!("unknown: {msg_type}")))?;
        let plaintext = self
            .client
            .signal()
            .decrypt_message(&parsed, enc_type, ciphertext)
            .await?;
        Ok(js_sys::Uint8Array::from(plaintext.as_slice()))
    }

    /// Encrypt plaintext for a group (sender key).
    /// Returns `{ senderKeyDistributionMessage: Uint8Array, ciphertext: Uint8Array }`.
    #[wasm_bindgen(js_name = signalEncryptGroupMessage)]
    pub async fn signal_encrypt_group_message(
        &self,
        group_jid: &str,
        data: &[u8],
        _me_id: &str,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed = parse_jid(group_jid)?;
        let (skdm, ciphertext) = self
            .client
            .signal()
            .encrypt_group_message(&parsed, data)
            .await?;
        let obj = js_sys::Object::new();
        let skdm_js = match &skdm {
            Some(bytes) => js_sys::Uint8Array::from(bytes.as_slice()).into(),
            None => JsValue::UNDEFINED,
        };
        js_sys::Reflect::set(&obj, &"senderKeyDistributionMessage".into(), &skdm_js)
            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        js_sys::Reflect::set(
            &obj,
            &"ciphertext".into(),
            &js_sys::Uint8Array::from(ciphertext.as_slice()).into(),
        )
        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        Ok(obj.into())
    }

    /// Decrypt a group (sender-key) message.
    #[wasm_bindgen(js_name = signalDecryptGroupMessage)]
    pub async fn signal_decrypt_group_message(
        &self,
        group_jid: &str,
        author_jid: &str,
        msg: &[u8],
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(author_jid)?;
        let plaintext = self
            .client
            .signal()
            .decrypt_group_message(&group, &sender, msg)
            .await?;
        Ok(js_sys::Uint8Array::from(plaintext.as_slice()))
    }

    /// Install a supplied pairwise pre-key bundle.
    #[wasm_bindgen(js_name = signalInstallPreKeyBundle)]
    pub async fn signal_install_prekey_bundle(
        &self,
        jid: &str,
        #[wasm_bindgen(unchecked_param_type = "SignalSessionBundleInput")] input: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let input = from_js_input::<crate::result_types::SignalSessionBundleInput>("input", input)?;
        use wacore::libsignal::protocol::{IdentityKey, PreKeyBundle, PublicKey};

        let parsed = parse_jid(jid)?;
        let identity_key = PublicKey::from_djb_public_key_bytes(&input.identity_key)
            .map(IdentityKey::new)
            .map_err(|error| crate::errors::invalid_arg("identityKey", error.to_string()))?;
        let signed_pre_key = PublicKey::from_djb_public_key_bytes(&input.signed_pre_key.public_key)
            .map_err(|error| {
                crate::errors::invalid_arg("signedPreKey.publicKey", error.to_string())
            })?;
        let pre_key = input
            .pre_key
            .map(|pre_key| {
                PublicKey::from_djb_public_key_bytes(&pre_key.public_key)
                    .map(|public_key| (pre_key.key_id.into(), public_key))
                    .map_err(|error| {
                        crate::errors::invalid_arg("preKey.publicKey", error.to_string())
                    })
            })
            .transpose()?;
        let bundle = PreKeyBundle::new(
            input.registration_id,
            u32::from(parsed.device).into(),
            pre_key,
            input.signed_pre_key.key_id.into(),
            signed_pre_key,
            input.signed_pre_key.signature,
            identity_key,
        )
        .map_err(|error| crate::errors::invalid_arg("bundle", error.to_string()))?;

        self.client
            .signal()
            .install_prekey_bundle(&parsed, &bundle)
            .await?;
        Ok(())
    }

    /// Force-refresh the server's one-time key pool.
    #[wasm_bindgen(js_name = refreshPreKeys)]
    pub async fn refresh_pre_keys(
        &self,
        count: Option<u32>,
    ) -> Result<(), crate::errors::BridgeError> {
        match count {
            Some(count) => {
                self.client
                    .refresh_pre_keys_with_count(count as usize)
                    .await?
            }
            None => self.client.refresh_pre_keys().await?,
        }
        Ok(())
    }

    /// Replenish the server's one-time key pool only when it is low.
    #[wasm_bindgen(js_name = ensurePreKeys)]
    pub async fn ensure_pre_keys(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.ensure_pre_keys().await?;
        Ok(())
    }

    /// Validate the server-side key-bundle digest against local state.
    #[wasm_bindgen(js_name = validateKeyBundle)]
    pub async fn validate_key_bundle(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.validate_digest_key().await?;
        Ok(())
    }

    /// Rotate the signed key advertised by the server.
    #[wasm_bindgen(js_name = rotateSignedKey)]
    pub async fn rotate_signed_key(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.rotate_signed_pre_key().await?;
        Ok(())
    }

    /// Process a raw sender-key distribution payload.
    #[wasm_bindgen(js_name = signalProcessSenderKeyDistribution)]
    pub async fn signal_process_sender_key_distribution(
        &self,
        group_jid: &str,
        sender_jid: &str,
        distribution: &[u8],
    ) -> Result<(), crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        self.client
            .signal()
            .process_sender_key_distribution(&group, &sender, distribution)
            .await?;
        Ok(())
    }

    /// Create the current sender-key distribution payload for a group.
    #[wasm_bindgen(js_name = signalGetSenderKeyDistribution)]
    pub async fn signal_get_sender_key_distribution(
        &self,
        group_jid: &str,
        sender_jid: &str,
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        let distribution = self
            .client
            .signal()
            .sender_key_distribution(&group, &sender)
            .await?;
        Ok(crate::wasm_utils::byte_array(&distribution))
    }

    /// Check whether sender-key state exists for a group and sender.
    #[wasm_bindgen(js_name = signalHasSenderKey)]
    pub async fn signal_has_sender_key(
        &self,
        group_jid: &str,
        sender_jid: &str,
    ) -> Result<bool, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        Ok(self.client.signal().has_sender_key(&group, &sender).await?)
    }

    /// Delete one sender-key chain from live and durable state.
    #[wasm_bindgen(js_name = signalDeleteSenderKey)]
    pub async fn signal_delete_sender_key(
        &self,
        group_jid: &str,
        sender_jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        self.client
            .signal()
            .delete_sender_key(&group, &sender)
            .await?;
        Ok(())
    }

    /// Inspect the currently open pairwise session for a JID.
    #[wasm_bindgen(js_name = signalGetSessionInfo)]
    pub async fn signal_get_session_info(
        &self,
        jid: &str,
    ) -> Result<Option<crate::result_types::SignalSessionInfoResult>, crate::errors::BridgeError>
    {
        let parsed = parse_jid(jid)?;
        Ok(self
            .client
            .signal()
            .session_info(&parsed)
            .await?
            .map(|info| crate::result_types::SignalSessionInfoResult {
                base_key: info.base_key,
                registration_id: info.registration_id,
            }))
    }

    /// Add linked-identifier mappings through one durable core batch.
    #[wasm_bindgen(js_name = addLidPnMappings)]
    pub async fn add_lid_pn_mappings(
        &self,
        #[wasm_bindgen(unchecked_param_type = "LidPnMappingInput[]")] mappings: JsValue,
    ) -> Result<u32, crate::errors::BridgeError> {
        let mappings =
            from_js_input::<Vec<crate::result_types::LidPnMappingInput>>("mappings", mappings)?;
        let mut pairs = Vec::with_capacity(mappings.len());
        for (index, mapping) in mappings.into_iter().enumerate() {
            let lid = parse_jid(&mapping.lid)?;
            if !lid.server.is_lid_family() {
                return Err(crate::errors::invalid_arg(
                    format!("mappings[{index}].lid"),
                    "must use a linked-identifier namespace",
                ));
            }

            let pn = parse_jid(&mapping.pn)?;
            if !pn.server.is_pn_family() {
                return Err(crate::errors::invalid_arg(
                    format!("mappings[{index}].pn"),
                    "must use a phone-number namespace",
                ));
            }
            pairs.push((lid.user.to_string(), pn.user.to_string()));
        }

        let written = self
            .client
            .add_lid_pn_mappings(pairs, whatsapp_rust::lid_pn_cache::LearningSource::Other)
            .await?;
        u32::try_from(written).map_err(|_| crate::errors::internal("mapping count exceeds u32"))
    }

    /// Move pairwise sessions between phone-number and linked-identifier namespaces.
    #[wasm_bindgen(js_name = signalMigrateSessions)]
    pub async fn signal_migrate_sessions(
        &self,
        from_jid: &str,
        to_jid: &str,
    ) -> Result<crate::result_types::SignalSessionMigrationResult, crate::errors::BridgeError> {
        let from = parse_jid(from_jid)?;
        let to = parse_jid(to_jid)?;
        let result = self.client.signal().migrate_sessions(&from, &to).await?;
        Ok(crate::result_types::SignalSessionMigrationResult {
            migrated: result.migrated as u32,
            skipped: result.skipped as u32,
            total: result.total as u32,
        })
    }

    /// Check whether a Signal session exists for the given JID.
    #[wasm_bindgen(js_name = signalValidateSession)]
    pub async fn signal_validate_session(
        &self,
        jid: &str,
    ) -> Result<bool, crate::errors::BridgeError> {
        let parsed = parse_jid(jid)?;
        self.client
            .signal()
            .validate_session(&parsed)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete Signal sessions for the given JIDs.
    #[wasm_bindgen(js_name = signalDeleteSessions)]
    pub async fn signal_delete_sessions(
        &self,
        jids: Vec<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let parsed: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;
        self.client
            .signal()
            .delete_sessions(&parsed)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Look up the LID JID corresponding to a given phone number JID.
    ///
    /// Accepts a bare phone number (treated as PN), a `<phone>@s.whatsapp.net`
    /// JID, or any LID/PN JID. Returns the full LID JID string (e.g.
    /// `100000012345678@lid`) or `null` when no mapping is known. Backed by
    /// the core's cache-aside `get_lid_pn_entry`: hits the in-memory cache
    /// first, then falls through to `backend.get_pn_mapping(user)` so a JS
    /// `JsStoreCallbacks` backend without a list primitive still resolves
    /// every persisted mapping without an extra usync round trip.
    #[wasm_bindgen(js_name = lidForPn)]
    pub async fn lid_for_pn(
        &self,
        jid: &str,
    ) -> Result<Option<String>, crate::errors::BridgeError> {
        let parsed = if jid.contains('@') {
            parse_jid(jid)?
        } else {
            Jid::pn(jid)
        };
        Ok(self
            .client
            .get_lid_pn_entry(&parsed)
            .await?
            .map(|e| format!("{}@lid", e.lid)))
    }

    /// Look up the phone number JID corresponding to a given LID JID.
    ///
    /// Accepts a bare LID user-part, a `<user>@lid` JID, or any LID/PN JID.
    /// Returns the full PN JID string (e.g. `559980000001@s.whatsapp.net`) or
    /// `null` when no mapping is known. Same cache-aside semantics as
    /// `lidForPn` — see that doc.
    #[wasm_bindgen(js_name = pnForLid)]
    pub async fn pn_for_lid(
        &self,
        jid: &str,
    ) -> Result<Option<String>, crate::errors::BridgeError> {
        let parsed = if jid.contains('@') {
            parse_jid(jid)?
        } else {
            Jid::lid(jid)
        };
        Ok(self
            .client
            .get_lid_pn_entry(&parsed)
            .await?
            .map(|e| format!("{}@s.whatsapp.net", e.phone_number)))
    }

    /// Convert a JID string to its Signal protocol address representation.
    #[wasm_bindgen(js_name = jidToSignalProtocolAddress)]
    pub fn jid_to_signal_protocol_address(
        &self,
        jid: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        use wacore::types::jid::JidExt;
        let parsed = parse_jid(jid)?;
        Ok(parsed.to_protocol_address_string())
    }

    // ── Participant node creation ────────────────────────────────────────

    /// Create encrypted participant `<to>` nodes for recipient JIDs.
    /// Returns `{ nodes: [...], shouldIncludeDeviceIdentity: boolean }`.
    /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
    #[wasm_bindgen(js_name = createParticipantNodesBytes)]
    pub async fn create_participant_nodes_bytes(
        &self,
        jids: Vec<String>,
        bytes: &[u8],
        _extra_attrs: JsValue,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let recipient_jids: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;

        let msg = waproto::codec::message_decode(bytes)
            .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;

        let (nodes, should_include_device_identity) = self
            .client
            .signal()
            .create_participant_nodes(&recipient_jids, &msg)
            .await?;

        let obj = js_sys::Object::new();
        let nodes_js = nodes_to_js_array(&nodes)
            .map_err(|e| crate::errors::internal(format!("node serialization failed: {e:?}")))?;
        js_sys::Reflect::set(&obj, &"nodes".into(), &nodes_js)
            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        js_sys::Reflect::set(
            &obj,
            &"shouldIncludeDeviceIdentity".into(),
            &should_include_device_identity.into(),
        )
        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        Ok(obj.into())
    }

    // ── Raw transport ────────────────────────────────────────────────────

    /// Send pre-marshaled bytes through the noise socket.
    #[wasm_bindgen(js_name = sendRawMessage)]
    pub async fn send_raw_message(&self, data: &[u8]) -> Result<(), crate::errors::BridgeError> {
        self.client
            .send_raw_bytes(data.to_vec())
            .await
            .map_err(crate::errors::BridgeError::from)
    }
}
