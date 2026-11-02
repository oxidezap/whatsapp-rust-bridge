use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Send a raw binary node stanza to WhatsApp servers.
                /// Accepts a JS object matching `{ tag: string, attrs: Record<string, string>, content?: ... }`.
                #[wasm_bindgen(js_name = sendNode)]
            )),
            signature: parse_quote!(
                async fn send_node(&self, node_js: JsBinaryNode) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Node {
                binding: parse_quote!(node),
                input: parse_quote!(node_js),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Opaque),
            call: parse_quote!(client.send_node(node)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Send an IQ node and return the matching response node.
                #[wasm_bindgen(js_name = queryNode)]
            )),
            signature: parse_quote!(
                async fn query_node(
                    &self,
                    node_js: JsBinaryNode,
                    timeout_ms: Option<f64>,
                ) -> Result<JsBinaryNode, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Node {
                    binding: parse_quote!(node),
                    input: parse_quote!(node_js),
                },
                Input::Timeout {
                    binding: parse_quote!(timeout),
                    field: parse_quote!("timeoutMs"),
                    value: parse_quote!(timeout_ms),
                    optional: true,
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Opaque),
            call: parse_quote!(client.send_iq_node(node, timeout)),
            output: ReturnAdapter::Codec(OutputCodec::Serialize {
                serializer: Serializer::NodeRef,
                member: None,
                optional: false,
                cast: true,
                error: SerializationError::From,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Confirm an inbound stanza through the core-owned acknowledgement path.
                #[wasm_bindgen(js_name = acknowledgeStanza)]
            )),
            signature: parse_quote!(
                async fn acknowledge_stanza(
                    &self,
                    stanza_js: JsBinaryNode,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Node {
                binding: parse_quote!(stanza),
                input: parse_quote!(stanza_js),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.acknowledge_stanza(&stanza.as_node_ref())),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Reject an inbound stanza through the core-owned acknowledgement path.
                #[wasm_bindgen(js_name = rejectStanza)]
            )),
            signature: parse_quote!(
                async fn reject_stanza(
                    &self,
                    stanza_js: JsBinaryNode,
                    error_code: i32,
                    failure_reason: Option<i32>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Node {
                    binding: parse_quote!(stanza),
                    input: parse_quote!(stanza_js),
                },
                Input::Preparation(Preparation::StanzaRejection),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.reject_stanza(&stanza.as_node_ref(), rejection)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Request retransmission without acknowledging the original stanza.
                #[wasm_bindgen(js_name = requestMessageRetry)]
            )),
            signature: parse_quote!(
                async fn request_message_retry(
                    &self,
                    stanza_js: JsBinaryNode,
                    force_include_keys: Option<bool>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Node {
                    binding: parse_quote!(stanza),
                    input: parse_quote!(stanza_js),
                },
                Input::Preparation(Preparation::RetryOptions),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.request_message_retry(&stanza.as_node_ref(), options)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Execute a validated typed USync query through the core-owned operation.
                /// The bridge performs exactly one Serde decode and one Serde encode; it
                /// neither constructs protocol nodes nor translates consumer-facing names.
                #[wasm_bindgen(js_name = queryUsync)]
            )),
            signature: parse_quote!(
                async fn query_usync(
                    &self,
                    query: JsUsyncQuery,
                ) -> Result<JsUsyncResponse, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Preparation(Preparation::Usync)],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.query_usync(query)),
            output: ReturnAdapter::Codec(OutputCodec::Serialize {
                serializer: Serializer::Proto,
                member: None,
                optional: false,
                cast: true,
                error: SerializationError::From,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Ensure E2E Signal sessions exist for the given JIDs.
                /// Returns true after sessions are established.
                ///
                /// `force` has no effect — the core exposes no such switch.
                #[wasm_bindgen(js_name = assertSessions)]
            )),
            signature: parse_quote!(
                async fn assert_sessions(
                    &self,
                    jids: Vec<String>,
                    _force: bool,
                ) -> Result<bool, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jids {
                binding: parse_quote!(parsed),
                value: parse_quote!(jids),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.signal().assert_sessions(&parsed)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::True,
                error: Some(ResultError::From),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the list of known devices for the given user JIDs via usync query.
                /// Returns an array of JID strings (one per device).
                ///
                /// `useCache` and `ignoreZeroDevices` have no effect — the core's device
                /// query takes neither.
                #[wasm_bindgen(js_name = getUSyncDevices)]
            )),
            signature: parse_quote!(
                async fn get_usync_devices(
                    &self,
                    jids: Vec<String>,
                    _use_cache: bool,
                    _ignore_zero_devices: bool,
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jids {
                binding: parse_quote!(parsed),
                value: parse_quote!(jids),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.signal().get_user_devices(&parsed)),
            output: ReturnAdapter::Codec(OutputCodec::Object(ObjectShape::UsyncDevices)),
        },
        Operation {
            attributes: attributes(quote!(
                /// Encrypt plaintext for a single recipient.
                /// Returns `{ type: "msg"|"pkmsg", ciphertext: Uint8Array }`.
                #[wasm_bindgen(js_name = signalEncryptMessage)]
            )),
            signature: parse_quote!(
                async fn signal_encrypt_message(
                    &self,
                    jid: &str,
                    data: &[u8],
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(parsed),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().encrypt_message(&parsed, data)),
            output: ReturnAdapter::Codec(OutputCodec::Object(ObjectShape::SignalMessage)),
        },
        Operation {
            attributes: attributes(quote!(
                /// Decrypt a Signal protocol message. `msg_type` is "msg", "pkmsg", or "skmsg".
                #[wasm_bindgen(js_name = signalDecryptMessage)]
            )),
            signature: parse_quote!(
                async fn signal_decrypt_message(
                    &self,
                    jid: &str,
                    msg_type: &str,
                    ciphertext: &[u8],
                ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(parsed),
                    value: parse_quote!(jid),
                },
                Input::Preparation(Preparation::DecryptType),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(
                client
                    .signal()
                    .decrypt_message(&parsed, enc_type, ciphertext)
            ),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::Bytes,
                error: Some(ResultError::SignalType),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Encrypt plaintext for a group (sender key).
                /// Returns `{ senderKeyDistributionMessage: Uint8Array, ciphertext: Uint8Array }`.
                ///
                /// `meId` has no effect — the core reads the sender from the store.
                #[wasm_bindgen(js_name = signalEncryptGroupMessage)]
            )),
            signature: parse_quote!(
                async fn signal_encrypt_group_message(
                    &self,
                    group_jid: &str,
                    data: &[u8],
                    _me_id: &str,
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(parsed),
                value: parse_quote!(group_jid),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().encrypt_group_message(&parsed, data)),
            output: ReturnAdapter::Codec(OutputCodec::Object(ObjectShape::SignalGroup)),
        },
        Operation {
            attributes: attributes(quote!(
                /// Decrypt a group (sender-key) message.
                #[wasm_bindgen(js_name = signalDecryptGroupMessage)]
            )),
            signature: parse_quote!(
                async fn signal_decrypt_group_message(
                    &self,
                    group_jid: &str,
                    author_jid: &str,
                    msg: &[u8],
                ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(sender),
                    value: parse_quote!(author_jid),
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().decrypt_group_message(&group, &sender, msg)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::Bytes,
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Install a supplied pairwise pre-key bundle.
                #[wasm_bindgen(js_name = signalInstallPreKeyBundle)]
            )),
            signature: parse_quote!(
                async fn signal_install_prekey_bundle(
                    &self,
                    jid: &str,
                    #[wasm_bindgen(unchecked_param_type = "SignalSessionBundleInput")] input: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Decode {
                    binding: parse_quote!(input),
                    ty: parse_quote!(crate::result_types::SignalSessionBundleInput),
                    field: parse_quote!("input"),
                    value: parse_quote!(input),
                },
                Input::Jid {
                    binding: parse_quote!(parsed),
                    value: parse_quote!(jid),
                },
                Input::Preparation(Preparation::PreKeyBundle),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().install_prekey_bundle(&parsed, &bundle)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Replenish the server's one-time key pool only when it is low.
                #[wasm_bindgen(js_name = ensurePreKeys)]
            )),
            signature: parse_quote!(
                async fn ensure_pre_keys(&self) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Redriven),
            call: parse_quote!(client.ensure_pre_keys()),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Validate the server-side key-bundle digest against local state.
                #[wasm_bindgen(js_name = validateKeyBundle)]
            )),
            signature: parse_quote!(
                async fn validate_key_bundle(&self) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Redriven),
            call: parse_quote!(client.validate_digest_key()),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Rotate the signed key advertised by the server.
                #[wasm_bindgen(js_name = rotateSignedKey)]
            )),
            signature: parse_quote!(
                async fn rotate_signed_key(&self) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Redriven),
            call: parse_quote!(client.rotate_signed_pre_key()),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Process a raw sender-key distribution payload.
                #[wasm_bindgen(js_name = signalProcessSenderKeyDistribution)]
            )),
            signature: parse_quote!(
                async fn signal_process_sender_key_distribution(
                    &self,
                    group_jid: &str,
                    sender_jid: &str,
                    distribution: &[u8],
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(sender),
                    value: parse_quote!(sender_jid),
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().process_sender_key_distribution(
                &group,
                &sender,
                distribution
            )),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Create the current sender-key distribution payload for a group.
                #[wasm_bindgen(js_name = signalGetSenderKeyDistribution)]
            )),
            signature: parse_quote!(
                async fn signal_get_sender_key_distribution(
                    &self,
                    group_jid: &str,
                    sender_jid: &str,
                ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(sender),
                    value: parse_quote!(sender_jid),
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().sender_key_distribution(&group, &sender)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::OwnedByteArray,
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Check whether sender-key state exists for a group and sender.
                #[wasm_bindgen(js_name = signalHasSenderKey)]
            )),
            signature: parse_quote!(
                async fn signal_has_sender_key(
                    &self,
                    group_jid: &str,
                    sender_jid: &str,
                ) -> Result<bool, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(sender),
                    value: parse_quote!(sender_jid),
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().has_sender_key(&group, &sender)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::Identity,
                error: Some(ResultError::From),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete one sender-key chain from live and durable state.
                #[wasm_bindgen(js_name = signalDeleteSenderKey)]
            )),
            signature: parse_quote!(
                async fn signal_delete_sender_key(
                    &self,
                    group_jid: &str,
                    sender_jid: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(group),
                    value: parse_quote!(group_jid),
                },
                Input::Jid {
                    binding: parse_quote!(sender),
                    value: parse_quote!(sender_jid),
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().delete_sender_key(&group, &sender)),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Inspect the currently open pairwise session for a JID.
                #[wasm_bindgen(js_name = signalGetSessionInfo)]
            )),
            signature: parse_quote!(
                async fn signal_get_session_info(
                    &self,
                    jid: &str,
                ) -> Result<Option<Ts<crate::result_types::SignalSessionInfoResult>>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(parsed),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().session_info(&parsed)),
            output: ReturnAdapter::Dto {
                projection: Projection::SignalSessionInfo,
                envelope: Envelope::Optional,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Add linked-identifier mappings through one durable core batch.
                #[wasm_bindgen(js_name = addLidPnMappings)]
            )),
            signature: parse_quote!(
                async fn add_lid_pn_mappings(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "LidPnMappingInput[]")] mappings: JsValue,
                ) -> Result<u32, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Preparation(Preparation::LidPnMappings)],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(
                client
                    .add_lid_pn_mappings(pairs, whatsapp_rust::lid_pn_cache::LearningSource::Other)
            ),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::CheckedU32("mapping count exceeds u32"),
                error: None,
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Move pairwise sessions between phone-number and linked-identifier namespaces.
                #[wasm_bindgen(js_name = signalMigrateSessions)]
            )),
            signature: parse_quote!(
                async fn signal_migrate_sessions(
                    &self,
                    from_jid: &str,
                    to_jid: &str,
                ) -> Result<Ts<crate::result_types::SignalSessionMigrationResult>, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(from),
                    value: parse_quote!(from_jid),
                },
                Input::Jid {
                    binding: parse_quote!(to),
                    value: parse_quote!(to_jid),
                },
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().migrate_sessions(&from, &to)),
            output: ReturnAdapter::Dto {
                projection: Projection::SignalSessionMigration,
                envelope: Envelope::Value,
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Check whether a Signal session exists for the given JID.
                #[wasm_bindgen(js_name = signalValidateSession)]
            )),
            signature: parse_quote!(
                async fn signal_validate_session(
                    &self,
                    jid: &str,
                ) -> Result<bool, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jid {
                binding: parse_quote!(parsed),
                value: parse_quote!(jid),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().validate_session(&parsed)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Delete Signal sessions for the given JIDs.
                #[wasm_bindgen(js_name = signalDeleteSessions)]
            )),
            signature: parse_quote!(
                async fn signal_delete_sessions(
                    &self,
                    jids: Vec<String>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Jids {
                binding: parse_quote!(parsed),
                value: parse_quote!(jids),
            }],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().delete_sessions(&parsed)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
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
            )),
            signature: parse_quote!(
                async fn lid_for_pn(
                    &self,
                    jid: &str,
                ) -> Result<Option<String>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Codec(InputCodec::FullOrBareJid {
                binding: parse_quote!(parsed),
                value: parse_quote!(jid),
                family: JidFamily::Pn,
                list: false,
            })],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.get_lid_pn_entry(&parsed)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::OptionalAddress {
                    field: parse_quote!(lid),
                    server: parse_quote!("lid"),
                },
                error: Some(ResultError::From),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Look up the phone number JID corresponding to a given LID JID.
                ///
                /// Accepts a bare LID user-part, a `<user>@lid` JID, or any LID/PN JID.
                /// Returns the full PN JID string (e.g. `559980000001@s.whatsapp.net`) or
                /// `null` when no mapping is known. Same cache-aside semantics as
                /// `lidForPn` — see that doc.
                #[wasm_bindgen(js_name = pnForLid)]
            )),
            signature: parse_quote!(
                async fn pn_for_lid(
                    &self,
                    jid: &str,
                ) -> Result<Option<String>, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Codec(InputCodec::FullOrBareJid {
                binding: parse_quote!(parsed),
                value: parse_quote!(jid),
                family: JidFamily::Lid,
                list: false,
            })],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.get_lid_pn_entry(&parsed)),
            output: ReturnAdapter::Codec(OutputCodec::Value {
                codec: ValueCodec::OptionalAddress {
                    field: parse_quote!(phone_number),
                    server: parse_quote!("s.whatsapp.net"),
                },
                error: Some(ResultError::From),
            }),
        },
        Operation {
            attributes: attributes(quote!(
                /// Create encrypted participant `<to>` nodes for recipient JIDs.
                /// Returns `{ nodes: [...], shouldIncludeDeviceIdentity: boolean }`.
                /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
                ///
                /// `extraAttrs` has no effect — the core sets the node attributes itself.
                #[wasm_bindgen(js_name = createParticipantNodesBytes)]
            )),
            signature: parse_quote!(
                async fn create_participant_nodes_bytes(
                    &self,
                    jids: Vec<String>,
                    bytes: &[u8],
                    _extra_attrs: JsValue,
                ) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jids {
                    binding: parse_quote!(recipient_jids),
                    value: parse_quote!(jids),
                },
                Input::Codec(InputCodec::MessageBytes {
                    binding: parse_quote!(msg),
                    value: parse_quote!(bytes),
                }),
            ],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .signal()
                    .create_participant_nodes(&recipient_jids, &msg)
            ),
            output: ReturnAdapter::Codec(OutputCodec::Object(ObjectShape::ParticipantNodes)),
        },
        Operation {
            attributes: attributes(quote!(
                /// Send pre-marshaled bytes through the noise socket.
                #[wasm_bindgen(js_name = sendRawMessage)]
            )),
            signature: parse_quote!(
                async fn send_raw_message(&self, data: &[u8]) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Opaque),
            call: parse_quote!(client.send_raw_bytes(data.to_vec())),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Force-refresh the server's one-time key pool.
                #[wasm_bindgen(js_name = refreshPreKeys)]
            )),
            signature: parse_quote!(
                async fn refresh_pre_keys(
                    &self,
                    count: Option<u32>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncOwned,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Redriven),
            call: parse_quote!({
                match count {
                    Some(count) => client.refresh_pre_keys_with_count(count as usize).await?,
                    None => client.refresh_pre_keys().await?,
                }
                Ok(())
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Convert a JID string to its Signal protocol address representation.
                #[wasm_bindgen(js_name = jidToSignalProtocolAddress)]
            )),
            signature: parse_quote!(
                fn jid_to_signal_protocol_address(
                    &self,
                    jid: &str,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::LocalConversion),
            call: parse_quote!({
                use wacore::types::jid::JidExt;
                let parsed = parse_jid(jid)?;
                Ok(parsed.to_protocol_address_string())
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(
                quote!(// ── Signal / low-level protocol ──────────────────────────────────────

    /// Enable or disable raw node forwarding. When enabled, a `raw_node` event
    /// is emitted for every decoded stanza before internal dispatch.
    #[wasm_bindgen(js_name = setRawNodeForwarding)]),
            ),
            signature: parse_quote!(
                fn set_raw_node_forwarding(&self, enabled: bool)
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::RawForwarding),
            call: parse_quote!(self.execute_set_raw_node_forwarding(enabled)),
            output: ReturnAdapter::Direct,
        },
    ]
}
