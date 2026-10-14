use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Reject an incoming call.
                #[wasm_bindgen(js_name = rejectCall)]
            )),
            signature: parse_quote!(
                async fn reject_call(
                    &self,
                    call_id: &str,
                    peer: &str,
                    call_creator: &str,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![
                Input::Jid {
                    binding: parse_quote!(peer),
                    value: parse_quote!(peer),
                },
                Input::Jid {
                    binding: parse_quote!(call_creator),
                    value: parse_quote!(call_creator),
                },
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::ConnectionBound),
            call: parse_quote!(client.voip().reject_call(call_id, &peer, &call_creator)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Send a raw binary node stanza to WhatsApp servers.
                /// Accepts a JS object matching `{ tag: string, attrs: Record<string, string>, content?: ... }`.
                #[wasm_bindgen(js_name = sendNode)]
            )),
            signature: parse_quote!(
                async fn send_node(&self, node_js: JsBinaryNode) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Convert(parse_quote!({
                let node_js: JsValue = node_js.into();
                let node = js_to_node(&node_js)?;
            }))],
            boundary: Boundary::AsyncBorrowed,
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
            inputs: vec![Input::Convert(parse_quote!({
                let node_js: JsValue = node_js.into();
                let node = js_to_node(&node_js)?;
                let timeout = parse_optional_timeout_ms("timeoutMs", timeout_ms)?;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Opaque),
            call: parse_quote!(client.send_iq_node(node, timeout)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(response),
                body: parse_quote!({ Ok(node_ref_to_js(response.get())?.unchecked_into()) }),
            },
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
            inputs: vec![Input::Convert(parse_quote!({
                let stanza_js: JsValue = stanza_js.into();
                let stanza = js_to_node(&stanza_js)?;
            }))],
            boundary: Boundary::AsyncBorrowed,
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
            inputs: vec![Input::Convert(parse_quote!({
                let stanza_js: JsValue = stanza_js.into();
                let stanza = js_to_node(&stanza_js)?;
                let reason = whatsapp_rust::NackReason::from(error_code);
                let rejection = if reason == whatsapp_rust::NackReason::InvalidProtobuf {
                    whatsapp_rust::StanzaRejection::invalid_protobuf(failure_reason)
                } else {
                    whatsapp_rust::StanzaRejection::new(reason)
                };
            }))],
            boundary: Boundary::AsyncBorrowed,
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
            inputs: vec![Input::Convert(parse_quote!({
                let stanza_js: JsValue = stanza_js.into();
                let stanza = js_to_node(&stanza_js)?;
                let options = whatsapp_rust::RetryRequestOptions::new()
                    .with_force_include_keys(force_include_keys.unwrap_or(false));
            }))],
            boundary: Boundary::AsyncBorrowed,
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
            inputs: vec![Input::Convert(parse_quote!({
                let query = crate::proto::from_js_value::<whatsapp_rust::usync::UsyncQuery>(
                    query.into(),
                    "query",
                )?;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.query_usync(query)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(response),
                body: parse_quote!({ Ok(crate::proto::to_js_value(&response)?.unchecked_into()) }),
            },
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
            inputs: vec![Input::Convert(parse_quote!({
                let parsed: Vec<wacore_binary::jid::Jid> = jids
                    .iter()
                    .map(|j| parse_jid(j))
                    .collect::<Result<_, _>>()?;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.signal().assert_sessions(&parsed)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    boundary_result?;
                    Ok(true)
                }),
            },
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
            inputs: vec![Input::Convert(parse_quote!({
                let parsed: Vec<wacore_binary::jid::Jid> = jids
                    .iter()
                    .map(|j| parse_jid(j))
                    .collect::<Result<_, _>>()?;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.signal().get_user_devices(&parsed)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(devices),
                body: parse_quote!({
                    let arr = js_sys::Array::new_with_length(devices.len() as u32);
                    for (i, jid) in devices.iter().enumerate() {
                        let obj = js_sys::Object::new();
                        js_sys::Reflect::set(&obj, &"user".into(), &jid.user.as_str().into())
                            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
                        if jid.device != 0 {
                            js_sys::Reflect::set(
                                &obj,
                                &"device".into(),
                                &(jid.device as f64).into(),
                            )
                            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
                        }
                        js_sys::Reflect::set(&obj, &"jid".into(), &jid.to_string().into())
                            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
                        arr.set(i as u32, obj.into());
                    }
                    Ok(arr.into())
                }),
            },
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().encrypt_message(&parsed, data)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let (msg_type, ciphertext) = boundary_result?;
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
                }),
            },
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
                Input::Convert(parse_quote!({
                    let enc_type = wacore::message_processing::EncType::from_wire(msg_type)
                        .ok_or_else(|| {
                            crate::errors::invalid_arg("msgType", format!("unknown: {msg_type}"))
                        })?;
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(
                client
                    .signal()
                    .decrypt_message(&parsed, enc_type, ciphertext)
            ),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let plaintext = boundary_result.map_err(|e| match e {
                        whatsapp_rust::features::SignalError::Unsupported(detail) => {
                            crate::errors::invalid_arg("msgType", detail)
                        }
                        other => other.into(),
                    })?;
                    Ok(js_sys::Uint8Array::from(plaintext.as_slice()))
                }),
            },
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().encrypt_group_message(&parsed, data)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let (skdm, ciphertext) = boundary_result?;
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
                }),
            },
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().decrypt_group_message(&group, &sender, msg)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(plaintext),
                body: parse_quote!({ Ok(js_sys::Uint8Array::from(plaintext.as_slice())) }),
            },
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
                Input::Convert(parse_quote!({
                    use wacore::libsignal::protocol::{IdentityKey, PreKeyBundle, PublicKey};
                })),
                Input::Jid {
                    binding: parse_quote!(parsed),
                    value: parse_quote!(jid),
                },
                Input::Convert(parse_quote!({
                    let identity_key = PublicKey::from_djb_public_key_bytes(&input.identity_key)
                        .map(IdentityKey::new)
                        .map_err(|error| {
                            crate::errors::invalid_arg("identityKey", error.to_string())
                        })?;
                    let signed_pre_key =
                        PublicKey::from_djb_public_key_bytes(&input.signed_pre_key.public_key)
                            .map_err(|error| {
                                crate::errors::invalid_arg(
                                    "signedPreKey.publicKey",
                                    error.to_string(),
                                )
                            })?;
                    let pre_key = input
                        .pre_key
                        .map(|pre_key| {
                            PublicKey::from_djb_public_key_bytes(&pre_key.public_key)
                                .map(|public_key| (pre_key.key_id.into(), public_key))
                                .map_err(|error| {
                                    crate::errors::invalid_arg(
                                        "preKey.publicKey",
                                        error.to_string(),
                                    )
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
                })),
            ],
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().sender_key_distribution(&group, &sender)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(distribution),
                body: parse_quote!({ Ok(crate::wasm_utils::byte_array(&distribution)) }),
            },
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().has_sender_key(&group, &sender)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({ Ok(boundary_result?) }),
            },
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
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().session_info(&parsed)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    to_ts_opt(boundary_result?.map(|info| {
                        crate::result_types::SignalSessionInfoResult {
                            base_key: info.base_key,
                            registration_id: info.registration_id,
                        }
                    }))
                }),
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
            inputs: vec![Input::Convert(parse_quote!({
                let mappings = from_js_input::<Vec<crate::result_types::LidPnMappingInput>>(
                    "mappings", mappings,
                )?;
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
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(
                client
                    .add_lid_pn_mappings(pairs, whatsapp_rust::lid_pn_cache::LearningSource::Other)
            ),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(written),
                body: parse_quote!({
                    u32::try_from(written)
                        .map_err(|_| crate::errors::internal("mapping count exceeds u32"))
                }),
            },
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
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.signal().migrate_sessions(&from, &to)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(result),
                body: parse_quote!({
                    to_ts(crate::result_types::SignalSessionMigrationResult {
                        migrated: result.migrated as u32,
                        skipped: result.skipped as u32,
                        total: result.total as u32,
                    })
                }),
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
            boundary: Boundary::AsyncBorrowed,
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
            inputs: vec![Input::Convert(parse_quote!({
                let parsed: Vec<wacore_binary::jid::Jid> = jids
                    .iter()
                    .map(|j| parse_jid(j))
                    .collect::<Result<_, _>>()?;
            }))],
            boundary: Boundary::AsyncBorrowed,
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
            inputs: vec![Input::Convert(parse_quote!({
                let parsed = if jid.contains('@') {
                    parse_jid(jid)?
                } else {
                    Jid::pn(jid)
                };
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.get_lid_pn_entry(&parsed)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({ Ok(boundary_result?.map(|e| format!("{}@lid", e.lid))) }),
            },
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
            inputs: vec![Input::Convert(parse_quote!({
                let parsed = if jid.contains('@') {
                    parse_jid(jid)?
                } else {
                    Jid::lid(jid)
                };
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.get_lid_pn_entry(&parsed)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    Ok(boundary_result?.map(|e| format!("{}@s.whatsapp.net", e.phone_number)))
                }),
            },
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
            inputs: vec![Input::Convert(parse_quote!({
                let recipient_jids: Vec<wacore_binary::jid::Jid> = jids
                    .iter()
                    .map(|j| parse_jid(j))
                    .collect::<Result<_, _>>()?;
                let msg = waproto::codec::message_decode(bytes)
                    .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(
                client
                    .signal()
                    .create_participant_nodes(&recipient_jids, &msg)
            ),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let (nodes, should_include_device_identity) = boundary_result?;
                    let obj = js_sys::Object::new();
                    let nodes_js = nodes_to_js_array(&nodes).map_err(|e| {
                        crate::errors::internal(format!("node serialization failed: {e:?}"))
                    })?;
                    js_sys::Reflect::set(&obj, &"nodes".into(), &nodes_js)
                        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
                    js_sys::Reflect::set(
                        &obj,
                        &"shouldIncludeDeviceIdentity".into(),
                        &should_include_device_identity.into(),
                    )
                    .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
                    Ok(obj.into())
                }),
            },
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
            boundary: Boundary::AsyncBorrowed,
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
            boundary: Boundary::AsyncBorrowed,
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
