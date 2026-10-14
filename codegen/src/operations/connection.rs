use super::*;

pub(super) fn catalog() -> Vec<Operation> {
    vec![
        Operation {
            attributes: attributes(quote!(
                /// Fetch the account's reachout-timelock state.
                ///
                /// Wraps the `WAWebMexFetchReachoutTimelockJobQuery` MEX persisted
                /// query (id sourced from `wacore::iq::mex_operations::fetch_reachout_timelock`)
                /// and returns the `xwa2_fetch_account_reachout_timelock` payload as a
                /// raw JSON object — typically:
                ///
                /// ```json
                /// { "is_active": true,
                ///   "time_enforcement_ends": "1734567890",
                ///   "enforcement_type": "BIZ_COMMERCE_VIOLATION_…" }
                /// ```
                ///
                /// Returns `null` when the server has no timelock for this account.
                /// Callers map snake_case → idiomatic shape themselves.
                #[wasm_bindgen(js_name = "fetchReachoutTimelock")]
            )),
            signature: parse_quote!(
                async fn fetch_reachout_timelock(&self) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.mex().fetch_reachout_timelock()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(payload),
                body: parse_quote!({
                    serde_wasm_bindgen::to_value(&payload).map_err(|e| {
                        crate::errors::internal(format!("serialize reachout payload: {e}"))
                    })
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Enable or disable automatic reconnection on disconnect.
                /// Enabled by default. When disabled, the client will not attempt
                /// to reconnect after an unexpected disconnection.
                #[wasm_bindgen(js_name = setAutoReconnect)]
            )),
            signature: parse_quote!(
                fn set_auto_reconnect(&self, enabled: bool)
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.set_auto_reconnect(enabled)),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Drop the current connection and reconnect immediately, picking up
                /// any profile changes (e.g. `setClientProfile`) on the new handshake.
                /// The `run()` loop continues — only the in-flight WebSocket is reset.
                #[wasm_bindgen(js_name = reconnect)]
            )),
            signature: parse_quote!(
                async fn reconnect(&self)
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::ThisSocket),
            call: parse_quote!(client.reconnect_immediately()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Check if the client is connected.
                #[wasm_bindgen(js_name = isConnected)]
            )),
            signature: parse_quote!(
                fn is_connected(&self) -> bool
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::ThisSocket),
            call: parse_quote!(client.is_socket_ready()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Check if the client is logged in (paired).
                #[wasm_bindgen(js_name = isLoggedIn)]
            )),
            signature: parse_quote!(
                fn is_logged_in(&self) -> bool
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::ThisSocket),
            call: parse_quote!(client.is_logged_in()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Wait until the socket is connected, or the timeout elapses.
                ///
                /// Resolves as soon as the socket is up — login may still be pending. The
                /// core rejects on expiry rather than reporting it, so this rejects with
                /// `kind === 'timeout'`.
                #[wasm_bindgen(js_name = waitForSocket)]
            )),
            signature: parse_quote!(
                async fn wait_for_socket(&self, timeout_ms: f64) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Timeout {
                binding: parse_quote!(timeout),
                field: parse_quote!("timeoutMs"),
                value: parse_quote!(timeout_ms),
                optional: false,
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::ThisSocket),
            call: parse_quote!(client.wait_for_socket_ready(timeout)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Wait until the client is connected and logged in, or the timeout elapses.
                ///
                /// Stricter than `waitForSocket`: the core resolves this only once the
                /// connection is fully ready. Rejects with `kind === 'timeout'` on expiry.
                #[wasm_bindgen(js_name = waitForConnected)]
            )),
            signature: parse_quote!(
                async fn wait_for_connected(
                    &self,
                    timeout_ms: f64,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Timeout {
                binding: parse_quote!(timeout),
                field: parse_quote!("timeoutMs"),
                value: parse_quote!(timeout_ms),
                optional: false,
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::ThisSocket),
            call: parse_quote!(client.wait_for_session_ready(timeout)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Acknowledge a server dirty bit so the server stops re-announcing it.
                ///
                /// `dirtyType` is the wire string (`account_sync`, `groups`,
                /// `syncd_app_state`, `newsletter_metadata`); an unrecognized one is
                /// forwarded as-is, matching the core's fallback.
                #[wasm_bindgen(js_name = cleanDirtyBits)]
            )),
            signature: parse_quote!(
                async fn clean_dirty_bits(
                    &self,
                    dirty_type: &str,
                    timestamp: Option<f64>,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Convert(parse_quote!({
                use wacore::iq::dirty::{DirtyBit, DirtyType};
                if dirty_type.is_empty() {
                    return Err(crate::errors::BridgeError::InvalidArgument {
                        field: "dirtyType".into(),
                        reason: "must not be empty".into(),
                    });
                }
                let kind = DirtyType::from(dirty_type);
                let bit = match timestamp {
                    Some(value) if is_representable_millis(value) => {
                        DirtyBit::with_timestamp(kind, value as u64)
                    }
                    Some(_) => {
                        return Err(crate::errors::BridgeError::InvalidArgument {
                            field: "timestamp".into(),
                            reason: MILLIS_RANGE.into(),
                        });
                    }
                    None => DirtyBit::new(kind),
                };
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Online,
            call: parse_quote!(client.clean_dirty_bits(bit)),
            output: ReturnAdapter::MapError,
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch the bot directory the server offers this account.
                ///
                /// There is no server push for it, so a caller that wants fresh data calls
                /// this again.
                #[wasm_bindgen(js_name = getBotList)]
            )),
            signature: parse_quote!(
                async fn get_bot_list(
                    &self,
                ) -> Result<Ts<crate::result_types::BotListResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.bots().list()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(list),
                body: parse_quote!({ to_ts(bot_list_to_result(&list)) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Fetch how many new chats this account may still start this cycle.
                #[wasm_bindgen(js_name = fetchNewChatMessageCappingInfo)]
            )),
            signature: parse_quote!(
                async fn fetch_new_chat_message_capping_info(
                    &self,
                ) -> Result<Ts<crate::result_types::NewChatMessageCappingResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Online,
            call: parse_quote!(client.mex().fetch_new_chat_message_capping_info()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(capping),
                body: parse_quote!({
                    to_ts(crate::result_types::NewChatMessageCappingResult {
                        capping_status: capping
                            .capping_status
                            .as_ref()
                            .map(|s| s.as_str().to_owned()),
                        ote_status: capping.ote_status.as_ref().map(|s| s.as_str().to_owned()),
                        mv_status: capping.mv_status.as_ref().map(|s| s.as_str().to_owned()),
                        total_quota: capping.total_quota.map(|v| v as f64),
                        used_quota: capping.used_quota.map(|v| v as f64),
                        cycle_start_timestamp: capping.cycle_start_timestamp.map(|v| v as f64),
                        cycle_end_timestamp: capping.cycle_end_timestamp.map(|v| v as f64),
                        server_sent_timestamp: capping.server_sent_timestamp.map(|v| v as f64),
                        remaining_quota: capping.remaining_quota().map(|v| v as f64),
                    })
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Persist a push name before the first connection handshake.
                ///
                /// This is the WASM equivalent of whatsapp-rust's
                /// `BotBuilder::with_push_name`: it only forwards the value into the
                /// core device state and adds no bridge-specific protocol behavior.
                #[wasm_bindgen(js_name = setInitialPushName)]
            )),
            signature: parse_quote!(
                async fn set_initial_push_name(&self, name: String)
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.persistence_manager().process_command(
                whatsapp_rust::wacore::store::DeviceCommand::SetPushName(name,)
            )),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Override `DeviceProps` before initial pairing. Only takes effect on
                /// the registration node — for already paired sessions this is a no-op
                /// on the wire and the core logs a warning.
                ///
                /// Setting `platformType: 'ANDROID_PHONE'` flips the phone's "Linked
                /// Devices" display to Android and unlocks server-side feature gating
                /// (e.g. view-once delivered as payload instead of `absent` stub) WITHOUT
                /// switching the underlying transport — the client still speaks the web
                /// protocol. Real Android companion mode (CRSC v2/v3, TEE attestation)
                /// is NOT implemented; if the server starts enforcing companion-type
                /// crypto, those connections may break.
                #[wasm_bindgen(js_name = setDeviceProps)]
            )),
            signature: parse_quote!(
                async fn set_device_props(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "DevicePropsInput")] input: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Decode {
                binding: parse_quote!(input),
                ty: parse_quote!(crate::device_props::DevicePropsInput),
                field: parse_quote!("input"),
                value: parse_quote!(input),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.set_device_props(input.into())),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Override the noise-handshake `ClientPayload` profile (UserAgent
                /// platform/device/os_version/manufacturer + `web_info` presence).
                ///
                /// Independent of `setDeviceProps`: that one drives the "Linked Devices"
                /// display on the phone; this one drives what the server sees during the
                /// noise handshake. Use `{ preset: 'android', osVersion: '13' }` to set
                /// `UserAgent.platform = ANDROID` and omit `web_info`.
                ///
                /// Runtime-only — the field is `#[serde(skip)]` in the persisted Device,
                /// so re-apply on every fresh process before `connect()`.
                #[wasm_bindgen(js_name = setClientProfile)]
            )),
            signature: parse_quote!(
                async fn set_client_profile(
                    &self,
                    #[wasm_bindgen(unchecked_param_type = "ClientProfileInput")] input: JsValue,
                ) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![Input::Decode {
                binding: parse_quote!(input),
                ty: parse_quote!(crate::client_profile::ClientProfileInput),
                field: parse_quote!("input"),
                value: parse_quote!(input),
            }],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.set_client_profile(input.into())),
            output: ReturnAdapter::Unit,
        },
        Operation {
            attributes: attributes(quote!(
                /// Request a pairing code for phone number login (alternative to QR).
                ///
                /// Returns the 8-character pairing code to enter on the phone. On error,
                /// throws a `WhatsAppError` with structured fields (`kind`, `serverCode`,
                /// `serverText`, etc.) — see `errors::BridgeError`.
                #[wasm_bindgen(js_name = requestPairingCode)]
            )),
            signature: parse_quote!(
                async fn request_pairing_code(
                    &self,
                    phone_number: &str,
                    custom_code: Option<String>,
                ) -> Result<String, crate::errors::BridgeError>
            ),
            inputs: vec![Input::Convert(parse_quote!({
                use whatsapp_rust::pair_code::PairCodeOptions;
                let options = PairCodeOptions {
                    phone_number: phone_number.to_string(),
                    custom_code,
                    ..Default::default()
                };
            }))],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::AwaitFallible,
            admission: Admission::Unwaited(Unwaited::ThisSocket),
            call: parse_quote!(client.pair_with_code(options)),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(code),
                body: parse_quote!({ Ok(code) }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the current push name.
                #[wasm_bindgen(js_name = getPushName)]
            )),
            signature: parse_quote!(
                async fn get_push_name(&self) -> String
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.push_name()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the own JID (phone number JID) if logged in.
                ///
                /// Returns the non-AD JID (without device suffix), e.g. "559980000014@s.whatsapp.net".
                /// This is the JID used for addressing in messages.
                #[wasm_bindgen(js_name = getJid)]
            )),
            signature: parse_quote!(
                async fn get_jid(&self) -> Option<String>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.pn().map(|j| j.to_non_ad().to_string())),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the own LID (linked identity) if available.
                ///
                /// Returns the non-AD LID (without device suffix), e.g. "100000012345678@lid".
                #[wasm_bindgen(js_name = getLid)]
            )),
            signature: parse_quote!(
                async fn get_lid(&self) -> Option<String>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.lid().map(|j| j.to_non_ad().to_string())),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Get the ADV signed device identity (account), if available.
                /// Exposes the persisted account identity to credential consumers.
                #[wasm_bindgen(js_name = getAccount)]
            )),
            signature: parse_quote!(
                async fn get_account(&self) -> Result<JsValue, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.persistence_manager().get_device_snapshot()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let snapshot = boundary_result;
                    match &snapshot.account {
                        Some(account) => crate::camel_serializer::to_js_value_camel(account)
                            .map_err(|e| {
                                crate::errors::internal(format!("account serialization: {e:?}"))
                            }),
                        None => Ok(JsValue::UNDEFINED),
                    }
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// Returns a snapshot of internal memory diagnostics (cache sizes, session counts, etc.).
                #[wasm_bindgen(js_name = getMemoryDiagnostics)]
            )),
            signature: parse_quote!(
                async fn get_memory_diagnostics(
                    &self,
                ) -> Result<Ts<crate::result_types::MemoryDiagnosticsResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Await,
            admission: Admission::Unwaited(Unwaited::Local),
            call: parse_quote!(client.resource_report()),
            output: ReturnAdapter::ProjectBlock {
                binding: parse_quote!(boundary_result),
                body: parse_quote!({
                    let report = boundary_result;
                    let d = &report.client;
                    to_ts(crate::result_types::MemoryDiagnosticsResult {
                        group_cache: d.group_cache.entries as f64,
                        group_cache_bytes: d.group_cache.bytes as f64,
                        device_registry_cache: d.device_registry_cache.entries as f64,
                        device_registry_cache_bytes: d.device_registry_cache.bytes as f64,
                        sender_key_device_cache: d.sender_key_device_cache.entries as f64,
                        sender_key_device_cache_bytes: d.sender_key_device_cache.bytes as f64,
                        group_devices_memo: d.group_devices_memo.entries as f64,
                        group_devices_memo_bytes: d.group_devices_memo.bytes as f64,
                        lid_pn_lid_entries: d.lid_pn_lid_entries.entries as f64,
                        lid_pn_lid_bytes: d.lid_pn_lid_entries.bytes as f64,
                        lid_pn_pn_entries: d.lid_pn_pn_entries.entries as f64,
                        lid_pn_pn_bytes: d.lid_pn_pn_entries.bytes as f64,
                        recent_messages: d.recent_messages.entries as f64,
                        recent_messages_bytes: d.recent_messages.bytes as f64,
                        message_retry_counts: d.message_retry_counts as f64,
                        undecryptable_dispatched: d.undecryptable_dispatched as f64,
                        pdo_pending_requests: d.pdo_pending_requests as f64,
                        pdo_requested: d.pdo_requested as f64,
                        session_locks: d.session_locks as f64,
                        chat_lanes: d.chat_lanes as f64,
                        group_distribution_locks: d.group_distribution_locks as f64,
                        group_distribution_lock_evictions: d.group_distribution_lock_evictions
                            as f64,
                        group_distribution_lock_eviction_blocks: d
                            .group_distribution_lock_eviction_blocks
                            as f64,
                        resend_rate_limiter_chats: d.resend_rate_limiter_chats as f64,
                        response_waiters: d.response_waiters as f64,
                        node_waiters: d.node_waiters as f64,
                        pending_retries: d.pending_retries as f64,
                        presence_subscriptions: d.presence_subscriptions as f64,
                        app_state_key_requests: d.app_state_key_requests as f64,
                        app_state_syncing: d.app_state_syncing as f64,
                        signal_cache_sessions: d.signal_sessions.entries as f64,
                        signal_cache_sessions_bytes: d.signal_sessions.bytes as f64,
                        signal_cache_identities: d.signal_identities.entries as f64,
                        signal_cache_identities_bytes: d.signal_identities.bytes as f64,
                        signal_cache_sender_keys: d.signal_sender_keys.entries as f64,
                        signal_cache_sender_keys_bytes: d.signal_sender_keys.bytes as f64,
                        history_sync_tasks: d.history_sync_tasks.entries as f64,
                        history_sync_payload_bytes: d.history_sync_tasks.bytes as f64,
                        history_sync_peak_tasks: d.history_sync_tasks_peak as f64,
                        history_sync_peak_payload_bytes: d.history_sync_payload_bytes_peak as f64,
                        chatstate_handlers: d.chatstate_handlers as f64,
                        custom_enc_handlers: d.custom_enc_handlers as f64,
                        client_estimated_bytes: d.total_estimated_bytes() as f64,
                        storage_memory_bytes: report.storage.memory_bytes.map(|v| v as f64),
                        storage_pages: report.storage.pages.map(|v| v as f64),
                        storage_io_read_bytes: report.storage.io_read_bytes.map(|v| v as f64),
                        storage_io_write_bytes: report.storage.io_write_bytes.map(|v| v as f64),
                        transport_read_buffer_bytes: report
                            .transport
                            .and_then(|v| v.read_buffer_bytes)
                            .map(|v| v as f64),
                        transport_write_buffer_bytes: report
                            .transport
                            .and_then(|v| v.write_buffer_bytes)
                            .map(|v| v as f64),
                        transport_tls_state_bytes: report
                            .transport
                            .and_then(|v| v.tls_state_bytes)
                            .map(|v| v as f64),
                        http_pool_connections: report
                            .http
                            .and_then(|v| v.pool_connections)
                            .map(|v| v as f64),
                        http_pool_buffer_bytes: report
                            .http
                            .and_then(|v| v.pool_buffer_bytes)
                            .map(|v| v as f64),
                        http_inflight_bytes: report
                            .http
                            .and_then(|v| v.inflight_bytes)
                            .map(|v| v as f64),
                        resource_estimated_bytes: report.total_estimated_bytes() as f64,
                    })
                }),
            },
        },
        Operation {
            attributes: attributes(quote!(
                /// What work handed to the client right now can expect.
                ///
                /// `isConnected()` and `isLoggedIn()` each answer half of it; this answers
                /// the question a refused caller actually has, which is whether asking
                /// again is worth it. `reconnecting` comes back on its own, `paused` comes
                /// back on `resume`, `unsupervised` needs a `run()`, and `finished` needs a
                /// new client.
                ///
                /// Read when the question comes up rather than carried on the error: a
                /// refusal is a fact about one instant, and a connection can be lost right
                /// after a call was admitted or restored right after one was refused.
                #[wasm_bindgen(js_name = reachability)]
            )),
            signature: parse_quote!(
                fn reachability(
                    &self,
                ) -> Result<Ts<crate::result_types::Reachability>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Reachability),
            call: parse_quote!({ to_ts(self.client.reachability()) }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Wait until the client can reach the server again, and report what ended
                /// the wait.
                ///
                /// Never resolves to `reconnecting` — that is the one state it waits out.
                /// Bounded by the client's lifetime rather than a duration, because the
                /// reconnect backoff is jittered and followed by a handshake; a caller that
                /// wants a deadline races this against one of its own.
                ///
                /// The calls that hold themselves need none of this. It is for the ones
                /// that do not — `sendNode`, `queryNode`, `sendRawMessage` — where only the
                /// caller knows whether the node it is holding survives a new socket.
                ///
                /// It is also how a caller bounds work rather than waiting. Racing a
                /// parked call against a timer abandons the promise but not the call, so a
                /// send still goes out when the reconnect lands; racing this instead
                /// leaves the decision to issue it with the caller.
                ///
                /// Not from an event handler: the core dispatches on its read loop, so a
                /// handler that waits here waits on the connection it is blocking.
                #[wasm_bindgen(js_name = waitUntilReachable)]
            )),
            signature: parse_quote!(
                async fn wait_until_reachable(
                    &self,
                ) -> Result<Ts<crate::result_types::Reachability>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::AsyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Reachability),
            call: parse_quote!({ to_ts(self.client.wait_until_reachable().await) }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Let go of every call waiting out a reconnect right now, and return how
                /// many were let go.
                ///
                /// A held call cannot be taken back by dropping its promise: wasm-bindgen
                /// drives the method to completion either way, so racing it against a
                /// deadline bounds the waiting and not the call — it still goes out when
                /// the reconnect lands, and asking again sends the same thing twice. This
                /// is how giving up is said instead: each released call rejects with
                /// `kind: 'withdrawn'` without reaching the core, so nothing it was about
                /// to do happened and re-issuing it is not a repeat.
                ///
                /// Releases what is waiting at the moment of the call and nothing else. It
                /// is not a mode — the next call waits like any other — and it does not
                /// touch the calls that never wait.
                #[wasm_bindgen(js_name = withdrawParkedCalls)]
            )),
            signature: parse_quote!(
                fn withdraw_parked_calls(&self) -> u32
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Withdrawal),
            call: parse_quote!({ self.client.withdraw_parked() }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Allocation churn for work polled by whatsapp-rust's instrumented
                /// runtime. It overlaps the global allocator totals but uniquely separates
                /// core task work from bridge/host-boundary work.
                #[wasm_bindgen(js_name = getCoreAllocationSnapshot)]
            )),
            signature: parse_quote!(
                fn get_core_allocation_snapshot(
                    &self,
                ) -> Result<Ts<crate::result_types::CoreAllocationSnapshotResult>, crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::SyncBorrowed,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Diagnostics),
            call: parse_quote!({
                let Some(meter) = &self.alloc_meter else {
                    return to_ts(crate::result_types::CoreAllocationSnapshotResult {
                        enabled: false,
                        allocated_bytes: 0.0,
                        freed_bytes: 0.0,
                        allocations: 0.0,
                        net_bytes: 0.0,
                    });
                };
                let snapshot = meter.snapshot();
                to_ts(crate::result_types::CoreAllocationSnapshotResult {
                    enabled: true,
                    allocated_bytes: snapshot.allocated_bytes as f64,
                    freed_bytes: snapshot.freed_bytes as f64,
                    allocations: snapshot.allocations as f64,
                    net_bytes: snapshot.net_bytes() as f64,
                })
            }),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(
                quote!(// ── Connection ───────────────────────────────────────────────────────

    /// Start the main client loop in the background.
    ///
    /// Spawns the connection loop (connect, handshake, message loop, reconnect)
    /// as a background task and returns immediately. The loop runs until `disconnect()`
    /// is called.
    ///
    /// Not `async` to avoid holding a wasm-bindgen borrow on `self` that would
    /// prevent calling other methods (disconnect, etc.).
#[wasm_bindgen(js_name = run)]),
            ),
            signature: parse_quote!(
                fn run(&mut self) -> Result<(), crate::errors::BridgeError>
            ),
            inputs: vec![],
            boundary: Boundary::SyncMutable,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Lifecycle),
            call: parse_quote!(self.execute_run()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Wait for the supervised run loop started by `run()` to end, and report
                /// why it ended.
                ///
                /// Resolves with the core's own completion reason, typed per branch: only
                /// `auto-reconnect-disabled` carries causes, and every absence is an
                /// absent key. The result is stored at termination, so a waiter that
                /// arrives after the run already ended reads the same completion, and any
                /// number of simultaneous waiters each receive it.
                ///
                /// A plain function returning a `Promise`, so the pending wait holds no
                /// borrow on the client: `disconnect()`, `free()` and a second `run()`
                /// all reach the client while it is outstanding.
                ///
                /// Three endings are told apart. A resolved promise is always the
                /// supervision's own completion. The bridge cancelling its waiter is a
                /// rejection on exactly one path: the host freed the client while the
                /// wait was pending, reported as `not-connected`. The host tearing down
                /// after the call returned is not awaited here and is not claimed. A
                /// manual `connect()` drives one connection outside supervision and never
                /// touches this observation.
                ///
                /// Rejects with `invalid-argument` when `run()` was never called.
                #[wasm_bindgen(js_name = waitForRunCompletion, unchecked_return_type = "Promise<RunCompletionResult>")]
            )),
            signature: parse_quote!(
                fn wait_for_run_completion(&self) -> js_sys::Promise
            ),
            inputs: vec![],
            boundary: Boundary::OwnedPromise,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Lifecycle),
            call: parse_quote!(self.prepare_wait_for_run_completion()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Connect to WhatsApp servers (single connection, no auto-reconnect).
                ///
                /// Resolves once the handshake is done, then a background task reads the
                /// connection until it ends. The core hands `connect()` back a `Connection`
                /// that decodes nothing until it is driven, so without that reader no event
                /// would ever fire and every request would time out.
                #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
                #[wasm_bindgen(js_name = connect)]
            )),
            signature: parse_quote!(
                fn connect(&self) -> js_sys::Promise
            ),
            inputs: vec![],
            boundary: Boundary::OwnedPromise,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Lifecycle),
            call: parse_quote!(self.prepare_connect()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Disconnect the client and flush pending state to storage.
                ///
                /// This is an explicit teardown barrier: the promise does not resolve until
                /// the core has flushed persistence and the bridge-owned background tasks
                /// have been stopped. The shared gate makes concurrent `disconnect()` and
                /// `logout()` calls idempotent instead of entering the core teardown paths
                /// together. If a host disconnect callback re-enters teardown, the callback
                /// wait is cancelled so the two teardown operations cannot wait on each other.
                #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
                #[wasm_bindgen(js_name = disconnect)]
            )),
            signature: parse_quote!(
                fn disconnect(&self) -> js_sys::Promise
            ),
            inputs: vec![],
            boundary: Boundary::OwnedPromise,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Lifecycle),
            call: parse_quote!(self.prepare_disconnect()),
            output: ReturnAdapter::Direct,
        },
        Operation {
            attributes: attributes(quote!(
                /// Logout from WhatsApp — deregisters this companion device and disconnects.
                ///
                /// Sends `remove-companion-device` IQ to the server (best-effort),
                /// then disconnects. Does NOT clear stored keys — the caller should
                /// delete the store to fully clear credentials.
                #[wasm_bindgen(unchecked_return_type = "Promise<void>")]
                #[wasm_bindgen(js_name = logout)]
            )),
            signature: parse_quote!(
                fn logout(&self) -> js_sys::Promise
            ),
            inputs: vec![],
            boundary: Boundary::OwnedPromise,
            evaluation: Evaluation::Immediate,
            admission: Admission::Adapter(AdmissionAdapter::Lifecycle),
            call: parse_quote!(self.prepare_logout()),
            output: ReturnAdapter::Direct,
        },
    ]
}
