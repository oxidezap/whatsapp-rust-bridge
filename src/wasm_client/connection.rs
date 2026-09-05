//! Connection, device props, pairing and state getters.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Connection ───────────────────────────────────────────────────────

    /// Start the main client loop in the background.
    ///
    /// Spawns the connection loop (connect, handshake, message loop, reconnect)
    /// as a background task and returns immediately. The loop runs until `disconnect()`
    /// is called.
    ///
    /// Not `async` to avoid holding a wasm-bindgen borrow on `self` that would
    /// prevent calling other methods (disconnect, etc.).
    pub fn run(&mut self) -> Result<(), crate::errors::BridgeError> {
        if self.sync_rx.is_none() {
            return Err(crate::errors::internal("run() has already been called"));
        }
        let client = self.client.unwaited(Unwaited::ThisSocket).clone();
        let runtime = self.runtime.clone();
        let observation = self.run_observation.clone();
        let sync_rx = self.sync_rx.take();

        // Sync worker — processes history sync and app state sync tasks.
        // Must drain promptly to prevent the sync channel (capacity 32) from
        // blocking the message processing loop.
        if let Some(receiver) = sync_rx {
            let worker_client = client.clone();
            let handle = runtime.spawn(Box::pin(async move {
                while let Ok(task) = receiver.recv().await {
                    worker_client.process_sync_task(task).await;
                }
                info!("Sync worker shutting down.");
            }));
            *self
                .sync_worker_handle
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }

        // The generation is claimed here, before the task is spawned, so a
        // waiter registering between the two still keys to this run.
        let generation = {
            let mut obs = observation.lock().unwrap_or_else(|e| e.into_inner());
            let generation = obs.started_runs;
            obs.started_runs += 1;
            obs.live_generation = Some(generation);
            obs.completed = None;
            generation
        };

        let handle = runtime.spawn(Box::pin(async move {
            let result = run_completion_to_result(generation, &client.run_with_reason().await);
            let waiters = {
                let mut obs = observation.lock().unwrap_or_else(|e| e.into_inner());
                // A stale generation finishing after a newer run started must
                // not rewrite the newer observation; late client activity never
                // touches it either, so what a waiter read stays read.
                if obs.live_generation != Some(generation) {
                    info!("Discarding run completion for a superseded generation.");
                    return;
                }
                obs.completed = Some(result.clone());
                core::mem::take(&mut obs.waiters)
            };
            for waiter in waiters {
                let _ = waiter.send(result.clone());
            }
            info!("Client run loop exited.");
        }));
        *self.run_handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);

        Ok(())
    }

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
    pub fn wait_for_run_completion(&self) -> js_sys::Promise {
        enum Admission {
            Ready(Result<crate::result_types::RunCompletionResult, crate::errors::BridgeError>),
            Pending(futures::channel::oneshot::Receiver<crate::result_types::RunCompletionResult>),
        }

        // Admitted here, synchronously at call time: a microtask later the
        // state may already describe a newer call, so deferring this read
        // would let a wait issued before `run()` observe the run it predates
        // instead of taking the before-run rejection it was owed.
        let admission = {
            let mut obs = self
                .run_observation
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(result) = obs.completed.clone() {
                Admission::Ready(Ok(result))
            } else if obs.host_torn_down {
                Admission::Ready(Err(crate::errors::BridgeError::NotConnected))
            } else if obs.live_generation.is_none() {
                Admission::Ready(Err(crate::errors::invalid_arg(
                    "waitForRunCompletion",
                    "run() has not been called",
                )))
            } else {
                let (tx, rx) = futures::channel::oneshot::channel();
                obs.waiters.push(tx);
                Admission::Pending(rx)
            }
        };

        // Only the owned verdict or receiver crosses into the promise; no
        // borrow on the client outlives the call.
        wasm_bindgen_futures::future_to_promise(async move {
            let outcome = match admission {
                Admission::Ready(outcome) => outcome,
                // The sender lives in the run task (or dies with the client
                // on `free()`): a cancellation here is the host teardown
                // path, never a reconnect the caller could wait out.
                Admission::Pending(rx) => match rx.await {
                    Ok(result) => Ok(result),
                    Err(_) => Err(crate::errors::BridgeError::NotConnected),
                },
            };
            match outcome {
                Ok(result) => serde_wasm_bindgen::to_value(&result)
                    .map_err(|e| bridge_error_to_js_value(&crate::errors::internal(e.to_string()))),
                Err(e) => Err(bridge_error_to_js_value(&e)),
            }
        })
    }

    /// Connect to WhatsApp servers (single connection, no auto-reconnect).
    ///
    /// Resolves once the handshake is done, then a background task reads the
    /// connection until it ends. The core hands `connect()` back a `Connection`
    /// that decodes nothing until it is driven, so without that reader no event
    /// would ever fire and every request would time out.
    pub async fn connect(&self) -> Result<(), crate::errors::BridgeError> {
        let client = self.client.unwaited(Unwaited::ThisSocket).clone();
        let (handshake_tx, handshake_rx) = async_channel::bounded(1);

        // Connecting and reading live in one task because `Connection` borrows
        // the client it came from: the borrow cannot outlive this call, so the
        // handshake result travels back over the channel instead.
        let handle = self.runtime.spawn(Box::pin(async move {
            match client.connect().await {
                Err(e) => {
                    let _ = handshake_tx.send(Err(e)).await;
                }
                Ok(connection) => {
                    let _ = handshake_tx.send(Ok(())).await;
                    let _ = connection.read_until_disconnected().await;
                    info!("Client connection reader exited.");
                }
            }
        }));

        let handshake = handshake_rx.recv().await.map_err(|_| {
            crate::errors::internal("connect task ended without reporting a handshake result")
        })?;

        match handshake {
            Ok(()) => {
                *self
                    .connection_handle
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(handle);
                Ok(())
            }
            // The task is already finished; keep the slot pointing at whatever
            // reader is still live rather than at this failed attempt.
            Err(e) => {
                handle.abort();
                Err(crate::errors::BridgeError::from(e))
            }
        }
    }

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
    pub async fn fetch_reachout_timelock(&self) -> Result<JsValue, crate::errors::BridgeError> {
        let payload = self
            .client
            .online()
            .await?
            .mex()
            .fetch_reachout_timelock()
            .await?;
        serde_wasm_bindgen::to_value(&payload)
            .map_err(|e| crate::errors::internal(format!("serialize reachout payload: {e}")))
    }

    /// Disconnect the client and flush pending state to storage.
    pub async fn disconnect(&self) {
        self.client
            .unwaited(Unwaited::ThisSocket)
            .disconnect()
            .await;
        // Core disconnect owns the final persistence flush. Abort the bridge
        // background saver afterwards so its pending timer cannot keep the
        // host event loop alive.
        if let Some(handle) = self
            .saver_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        // The run task is deliberately not aborted: disconnecting is what ends
        // supervision, and the task publishes that ending (`shutdown-requested`
        // or whatever the core observed) to the run observation. Aborting it
        // here would destroy the reason `waitForRunCompletion()` exists to
        // carry. `Drop` still aborts it for the `free()`-without-disconnect
        // path, where no completion can be published anymore.
        if let Some(handle) = self
            .sync_worker_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
    }

    /// Logout from WhatsApp — deregisters this companion device and disconnects.
    ///
    /// Sends `remove-companion-device` IQ to the server (best-effort),
    /// then disconnects. Does NOT clear stored keys — the caller should
    /// delete the store to fully clear credentials.
    pub async fn logout(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.unwaited(Unwaited::ThisSocket).logout().await;
        if let Some(handle) = self
            .saver_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        // As in `disconnect()`: the run task publishes the supervision ending
        // that logging out produced, so it is left to finish rather than
        // aborted. Whatever the core observed (a shutdown or a
        // reconnect-disabled exit while deregistration was in flight) crosses
        // unchanged.
        if let Some(handle) = self
            .sync_worker_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        Ok(())
    }

    /// Enable or disable automatic reconnection on disconnect.
    /// Enabled by default. When disabled, the client will not attempt
    /// to reconnect after an unexpected disconnection.
    #[wasm_bindgen(js_name = setAutoReconnect)]
    pub fn set_auto_reconnect(&self, enabled: bool) {
        self.client
            .unwaited(Unwaited::Local)
            .enable_auto_reconnect
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Drop the current connection and reconnect immediately, picking up
    /// any profile changes (e.g. `setClientProfile`) on the new handshake.
    /// The `run()` loop continues — only the in-flight WebSocket is reset.
    #[wasm_bindgen(js_name = reconnect)]
    pub async fn reconnect(&self) {
        self.client
            .unwaited(Unwaited::ThisSocket)
            .reconnect_immediately()
            .await;
    }

    /// Check if the client is connected.
    #[wasm_bindgen(js_name = isConnected)]
    pub fn is_connected(&self) -> bool {
        self.client.unwaited(Unwaited::ThisSocket).is_connected()
    }

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
    pub fn reachability(
        &self,
    ) -> Result<Ts<crate::result_types::Reachability>, crate::errors::BridgeError> {
        to_ts(self.client.reachability())
    }

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
    pub async fn wait_until_reachable(
        &self,
    ) -> Result<Ts<crate::result_types::Reachability>, crate::errors::BridgeError> {
        to_ts(self.client.wait_until_reachable().await)
    }

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
    pub fn withdraw_parked_calls(&self) -> u32 {
        self.client.withdraw_parked()
    }

    /// Check if the client is logged in (paired).
    #[wasm_bindgen(js_name = isLoggedIn)]
    pub fn is_logged_in(&self) -> bool {
        self.client.unwaited(Unwaited::ThisSocket).is_logged_in()
    }

    /// Wait until the socket is connected, or the timeout elapses.
    ///
    /// Resolves as soon as the socket is up — login may still be pending. The
    /// core rejects on expiry rather than reporting it, so this rejects with
    /// `kind === 'timeout'`.
    #[wasm_bindgen(js_name = waitForSocket)]
    pub async fn wait_for_socket(&self, timeout_ms: f64) -> Result<(), crate::errors::BridgeError> {
        let timeout = parse_timeout_ms("timeoutMs", timeout_ms)?;
        self.client
            .unwaited(Unwaited::ThisSocket)
            .wait_for_socket(timeout)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Wait until the client is connected and logged in, or the timeout elapses.
    ///
    /// Stricter than `waitForSocket`: the core resolves this only once the
    /// connection is fully ready. Rejects with `kind === 'timeout'` on expiry.
    #[wasm_bindgen(js_name = waitForConnected)]
    pub async fn wait_for_connected(
        &self,
        timeout_ms: f64,
    ) -> Result<(), crate::errors::BridgeError> {
        let timeout = parse_timeout_ms("timeoutMs", timeout_ms)?;
        self.client
            .unwaited(Unwaited::ThisSocket)
            .wait_for_connected(timeout)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Acknowledge a server dirty bit so the server stops re-announcing it.
    ///
    /// `dirtyType` is the wire string (`account_sync`, `groups`,
    /// `syncd_app_state`, `newsletter_metadata`); an unrecognized one is
    /// forwarded as-is, matching the core's fallback.
    #[wasm_bindgen(js_name = cleanDirtyBits)]
    pub async fn clean_dirty_bits(
        &self,
        dirty_type: &str,
        timestamp: Option<f64>,
    ) -> Result<(), crate::errors::BridgeError> {
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
        self.client
            .online()
            .await?
            .clean_dirty_bits(bit)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Fetch the bot directory the server offers this account.
    ///
    /// There is no server push for it, so a caller that wants fresh data calls
    /// this again.
    #[wasm_bindgen(js_name = getBotList)]
    pub async fn get_bot_list(
        &self,
    ) -> Result<Ts<crate::result_types::BotListResult>, crate::errors::BridgeError> {
        let list = self.client.online().await?.bots().list().await?;
        to_ts(bot_list_to_result(&list))
    }

    /// Fetch how many new chats this account may still start this cycle.
    #[wasm_bindgen(js_name = fetchNewChatMessageCappingInfo)]
    pub async fn fetch_new_chat_message_capping_info(
        &self,
    ) -> Result<Ts<crate::result_types::NewChatMessageCappingResult>, crate::errors::BridgeError>
    {
        let capping = self
            .client
            .online()
            .await?
            .mex()
            .fetch_new_chat_message_capping_info()
            .await?;
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
    }

    // ── Device props ─────────────────────────────────────────────────────

    /// Persist a push name before the first connection handshake.
    ///
    /// This is the WASM equivalent of whatsapp-rust's
    /// `BotBuilder::with_push_name`: it only forwards the value into the
    /// core device state and adds no bridge-specific protocol behavior.
    #[wasm_bindgen(js_name = setInitialPushName)]
    pub async fn set_initial_push_name(&self, name: String) {
        self.client
            .unwaited(Unwaited::Local)
            .persistence_manager()
            .process_command(whatsapp_rust::wacore::store::DeviceCommand::SetPushName(
                name,
            ))
            .await;
    }

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
    pub async fn set_device_props(
        &self,
        #[wasm_bindgen(unchecked_param_type = "DevicePropsInput")] input: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let input = from_js_input::<crate::device_props::DevicePropsInput>("input", input)?;
        self.client
            .unwaited(Unwaited::Local)
            .set_device_props(input.into())
            .await;
        Ok(())
    }

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
    pub async fn set_client_profile(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ClientProfileInput")] input: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let input = from_js_input::<crate::client_profile::ClientProfileInput>("input", input)?;
        self.client
            .unwaited(Unwaited::Local)
            .set_client_profile(input.into())
            .await;
        Ok(())
    }

    // ── Pairing ──────────────────────────────────────────────────────────

    /// Request a pairing code for phone number login (alternative to QR).
    ///
    /// Returns the 8-character pairing code to enter on the phone. On error,
    /// throws a `WhatsAppError` with structured fields (`kind`, `serverCode`,
    /// `serverText`, etc.) — see `errors::BridgeError`.
    #[wasm_bindgen(js_name = requestPairingCode)]
    pub async fn request_pairing_code(
        &self,
        phone_number: &str,
        custom_code: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        use whatsapp_rust::pair_code::PairCodeOptions;
        let options = PairCodeOptions {
            phone_number: phone_number.to_string(),
            custom_code,
            ..Default::default()
        };
        let code = self
            .client
            .unwaited(Unwaited::ThisSocket)
            .pair_with_code(options)
            .await?;
        Ok(code)
    }

    // ── State getters ────────────────────────────────────────────────────

    /// Get the current push name.
    #[wasm_bindgen(js_name = getPushName)]
    pub async fn get_push_name(&self) -> String {
        // Sync since whatsapp-rust #808 (cached Arc<Device> snapshot); kept
        // async so the JS surface stays Promise-based.
        self.client.unwaited(Unwaited::Local).push_name()
    }

    /// Get the own JID (phone number JID) if logged in.
    ///
    /// Returns the non-AD JID (without device suffix), e.g. "559980000014@s.whatsapp.net".
    /// This is the JID used for addressing in messages.
    #[wasm_bindgen(js_name = getJid)]
    pub async fn get_jid(&self) -> Option<String> {
        self.client
            .unwaited(Unwaited::Local)
            .pn()
            .map(|j| j.to_non_ad().to_string())
    }

    /// Get the own LID (linked identity) if available.
    ///
    /// Returns the non-AD LID (without device suffix), e.g. "100000012345678@lid".
    #[wasm_bindgen(js_name = getLid)]
    pub async fn get_lid(&self) -> Option<String> {
        self.client
            .unwaited(Unwaited::Local)
            .lid()
            .map(|j| j.to_non_ad().to_string())
    }

    /// Get the ADV signed device identity (account), if available.
    /// Exposes the persisted account identity to credential consumers.
    #[wasm_bindgen(js_name = getAccount)]
    pub async fn get_account(&self) -> Result<JsValue, crate::errors::BridgeError> {
        let snapshot = self
            .client
            .unwaited(Unwaited::Local)
            .persistence_manager()
            .get_device_snapshot();
        match &snapshot.account {
            Some(account) => crate::camel_serializer::to_js_value_camel(account)
                .map_err(|e| crate::errors::internal(format!("account serialization: {e:?}"))),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Returns a snapshot of internal memory diagnostics (cache sizes, session counts, etc.).
    #[wasm_bindgen(js_name = getMemoryDiagnostics)]
    pub async fn get_memory_diagnostics(
        &self,
    ) -> Result<Ts<crate::result_types::MemoryDiagnosticsResult>, crate::errors::BridgeError> {
        let report = self
            .client
            .unwaited(Unwaited::Local)
            .resource_report()
            .await;
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
            group_distribution_lock_evictions: d.group_distribution_lock_evictions as f64,
            group_distribution_lock_eviction_blocks: d.group_distribution_lock_eviction_blocks
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
            http_inflight_bytes: report.http.and_then(|v| v.inflight_bytes).map(|v| v as f64),
            resource_estimated_bytes: report.total_estimated_bytes() as f64,
        })
    }

    /// Allocation churn for work polled by whatsapp-rust's instrumented
    /// runtime. It overlaps the global allocator totals but uniquely separates
    /// core task work from bridge/host-boundary work.
    #[wasm_bindgen(js_name = getCoreAllocationSnapshot)]
    pub fn get_core_allocation_snapshot(
        &self,
    ) -> Result<Ts<crate::result_types::CoreAllocationSnapshotResult>, crate::errors::BridgeError>
    {
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
    }
}

// ---------------------------------------------------------------------------
// Run completion observation mapping
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
fn bridge_error_to_js_value(e: &crate::errors::BridgeError) -> JsValue {
    crate::errors::to_js_error(e)
}

/// Host-target builds never drive the promise future; the rejection shape
/// only has to be a `JsValue` so the export keeps one surface per target.
#[cfg(not(target_arch = "wasm32"))]
fn bridge_error_to_js_value(e: &crate::errors::BridgeError) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// The core's completion reason, typed per branch for the `waitForRunCompletion`
/// promise. Known variants are named explicitly rather than rendered through
/// `Debug`; the wildcard the core's `#[non_exhaustive]` forces keeps the
/// core's own rendering as its detail.
fn run_completion_to_result(
    generation: u64,
    reason: &whatsapp_rust::RunCompletionReason,
) -> crate::result_types::RunCompletionResult {
    use crate::result_types::RunCompletionResult as R;
    use whatsapp_rust::RunCompletionReason as C;
    let generation = generation as f64;
    match reason {
        C::ShutdownRequested => R::ShutdownRequested { generation },
        C::AutoReconnectDisabled {
            connection,
            connect_error,
            protocol_error,
        } => R::AutoReconnectDisabled {
            generation,
            connection: connection.as_ref().map(disconnect_reason_to_result),
            connect_error: connect_error.as_ref().map(connect_error_to_result),
            protocol_error: protocol_error.as_ref().map(protocol_terminal_to_result),
        },
        C::Stopped => R::Stopped { generation },
        C::AlreadyRunning => R::AlreadyRunning { generation },
        other => R::Unknown {
            generation,
            detail: format!("{other:?}"),
        },
    }
}

fn disconnect_reason_to_result(
    reason: &whatsapp_rust::wacore::net::DisconnectReason,
) -> crate::result_types::DisconnectReasonResult {
    use crate::result_types::DisconnectReasonResult as R;
    use whatsapp_rust::wacore::net::DisconnectReason as D;
    match reason {
        D::ServerClose { code, reason } => R::ServerClose {
            code: code.map(|c| c as f64),
            reason: reason.clone(),
        },
        D::StreamEnded => R::StreamEnded,
        D::ReadError(message) => R::ReadError {
            message: message.clone(),
        },
        D::Unknown => R::Unknown,
    }
}

/// See [`run_completion_to_result`]. The `anyhow` leaves cross as their
/// rendered message: diagnostic text, not a boundary contract.
fn connect_error_to_result(
    error: &whatsapp_rust::ConnectError,
) -> crate::result_types::ConnectErrorResult {
    use crate::result_types::ConnectErrorResult as R;
    use whatsapp_rust::ConnectError as E;
    match error {
        E::AlreadyConnected => R::AlreadyConnected,
        E::NotActivated => R::NotActivated,
        E::Shutdown => R::Shutdown,
        E::Paused => R::Paused,
        E::Timeout { stage, timeout } => R::Timeout {
            stage: connect_stage_str(stage),
            timeout_ms: timeout.as_millis() as f64,
        },
        E::Version(message) => R::Version {
            message: message.to_string(),
        },
        E::Transport(message) => R::Transport {
            message: message.to_string(),
        },
        E::Handshake(failure) => R::Handshake {
            reason: handshake_failure_to_result(failure),
        },
        other => R::Unknown {
            detail: format!("{other:?}"),
        },
    }
}

/// See [`run_completion_to_result`]. Known stages use the boundary spelling;
/// the wildcard the core's `#[non_exhaustive]` forces keeps the core's own
/// rendering as its detail rather than collapsing into a neighbour's name.
fn connect_stage_str(stage: &whatsapp_rust::ConnectStage) -> String {
    use whatsapp_rust::ConnectStage as S;
    match stage {
        S::VersionFetch => "version-fetch".into(),
        S::Transport => "transport".into(),
        S::Socket => "socket".into(),
        S::Ready => "ready".into(),
        other => format!("{other:?}"),
    }
}

/// See [`run_completion_to_result`]. The lower Noise failure keeps its own
/// typed shape rather than flattening to one string.
fn handshake_failure_to_result(
    failure: &whatsapp_rust::handshake::HandshakeError,
) -> crate::result_types::HandshakeFailureResult {
    use crate::result_types::HandshakeFailureResult as R;
    use whatsapp_rust::handshake::HandshakeError as H;
    match failure {
        H::Transport(message) => R::Transport {
            message: message.to_string(),
        },
        H::Core(failure) => R::Core {
            reason: noise_handshake_failure_to_result(failure),
        },
        H::Timeout => R::Timeout,
        H::StreamClosed => R::StreamClosed,
        H::Disconnected => R::Disconnected,
        H::UnexpectedEvent(detail) => R::UnexpectedEvent {
            detail: detail.clone(),
        },
        other => R::Unknown {
            detail: format!("{other:?}"),
        },
    }
}

/// See [`run_completion_to_result`]. The core enum is exhaustive, so a cause
/// added upstream stops the build here and is given a shape deliberately.
fn noise_handshake_failure_to_result(
    failure: &whatsapp_rust::wacore::handshake::HandshakeError,
) -> crate::result_types::NoiseHandshakeFailureResult {
    use crate::result_types::NoiseHandshakeFailureResult as R;
    use whatsapp_rust::wacore::handshake::HandshakeError as N;
    match failure {
        N::ProtoDecode(message) => R::ProtoDecode {
            message: message.to_string(),
        },
        N::IncompleteResponse => R::IncompleteResponse,
        N::Crypto(detail) => R::Crypto {
            detail: detail.clone(),
        },
        N::CertVerification(detail) => R::CertVerification {
            detail: detail.clone(),
        },
        N::InvalidLength {
            name,
            expected,
            got,
        } => R::InvalidLength {
            name: name.clone(),
            expected: *expected as f64,
            got: *got as f64,
        },
        N::InvalidKeyLength => R::InvalidKeyLength,
        N::Noise(message) => R::Noise {
            message: message.to_string(),
        },
    }
}

/// See [`run_completion_to_result`]. The connect-failure spelling is the one
/// the `connect_failure` event already carries.
fn protocol_terminal_to_result(
    reason: &whatsapp_rust::ProtocolTerminalReason,
) -> crate::result_types::ProtocolTerminalReasonResult {
    use crate::result_types::ProtocolTerminalReasonResult as R;
    use whatsapp_rust::ProtocolTerminalReason as P;
    match reason {
        P::StreamErrorCode(code) => R::StreamError { code: *code as f64 },
        P::ConnectFailure(reason) => R::ConnectFailure {
            reason: super::connect_failure_reason_str(reason),
        },
        P::Conflict => R::Conflict,
        other => R::Unknown {
            detail: format!("{other:?}"),
        },
    }
}

#[cfg(test)]
mod run_completion_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn payload_of(result: &crate::result_types::RunCompletionResult) -> serde_json::Value {
        serde_json::to_value(result).expect("a run completion result serializes")
    }

    #[test]
    fn terminal_branches_keep_their_names_and_generation() {
        use whatsapp_rust::RunCompletionReason as C;

        for (reason, name) in [
            (C::ShutdownRequested, "shutdown-requested"),
            (C::Stopped, "stopped"),
            (C::AlreadyRunning, "already-running"),
        ] {
            let payload = payload_of(&run_completion_to_result(3, &reason));
            assert_eq!(payload["reason"], name);
            assert_eq!(payload["generation"], 3.0);
        }
    }

    /// Absence is the contract on the one branch that carries causes: a run
    /// that never established a connection has no reader outcome to report,
    /// and a missing key is what the host branches on.
    #[test]
    fn an_auto_reconnect_exit_without_causes_omits_every_cause_key() {
        use whatsapp_rust::RunCompletionReason as C;

        let payload = payload_of(&run_completion_to_result(
            0,
            &C::AutoReconnectDisabled {
                connection: None,
                connect_error: None,
                protocol_error: None,
            },
        ));
        assert_eq!(payload["reason"], "auto-reconnect-disabled");
        assert!(
            payload.get("connection").is_none(),
            "an absent reader outcome must not become a key, got {payload}"
        );
        assert!(
            payload.get("connectError").is_none(),
            "an absent connect failure must not become a key, got {payload}"
        );
        assert!(
            payload.get("protocolError").is_none(),
            "an absent protocol cause must not become a key, got {payload}"
        );
    }

    #[test]
    fn every_disconnect_reason_crosses_typed() {
        use whatsapp_rust::wacore::net::DisconnectReason as D;

        let server_close = disconnect_reason_to_result(&D::ServerClose {
            code: Some(1000),
            reason: "normal".into(),
        });
        let payload = serde_json::to_value(&server_close).expect("serializes");
        assert_eq!(payload["kind"], "server-close");
        assert_eq!(payload["code"], 1000.0);
        assert_eq!(payload["reason"], "normal");

        let codeless = disconnect_reason_to_result(&D::ServerClose {
            code: None,
            reason: String::new(),
        });
        let payload = serde_json::to_value(&codeless).expect("serializes");
        assert!(
            payload.get("code").is_none(),
            "a close frame without a code omits it, got {payload}"
        );

        for (reason, kind) in [
            (D::StreamEnded, "stream-ended"),
            (D::ReadError("reset".into()), "read-error"),
            (D::Unknown, "unknown"),
        ] {
            let payload =
                serde_json::to_value(disconnect_reason_to_result(&reason)).expect("serializes");
            assert_eq!(payload["kind"], kind);
        }
    }

    #[test]
    fn every_connect_error_crosses_typed() {
        use whatsapp_rust::{ConnectError as E, ConnectStage as S};

        let timeout = connect_error_to_result(&E::Timeout {
            stage: S::Socket,
            timeout: std::time::Duration::from_secs(5),
        });
        let payload = serde_json::to_value(&timeout).expect("serializes");
        assert_eq!(payload["kind"], "timeout");
        assert_eq!(payload["stage"], "socket");
        assert_eq!(payload["timeoutMs"], 5000.0);

        let version = connect_error_to_result(&E::Version(anyhow::anyhow!("no version")));
        let payload = serde_json::to_value(&version).expect("serializes");
        assert_eq!(payload["kind"], "version");
        assert_eq!(payload["message"], "no version");

        for (error, kind) in [
            (E::AlreadyConnected, "already-connected"),
            (E::NotActivated, "not-activated"),
            (E::Shutdown, "shutdown"),
            (E::Paused, "paused"),
        ] {
            let payload =
                serde_json::to_value(connect_error_to_result(&error)).expect("serializes");
            assert_eq!(payload["kind"], kind);
        }
    }

    #[test]
    fn connect_stages_keep_their_spelling() {
        use whatsapp_rust::ConnectStage as S;

        for (stage, expected) in [
            (S::VersionFetch, "version-fetch"),
            (S::Transport, "transport"),
            (S::Socket, "socket"),
            (S::Ready, "ready"),
        ] {
            assert_eq!(connect_stage_str(&stage), expected);
        }
    }
    #[test]
    fn handshake_failures_cross_typed() {
        use whatsapp_rust::handshake::HandshakeError as H;

        let transport = handshake_failure_to_result(&H::Transport(anyhow::anyhow!("socket gone")));
        let payload = serde_json::to_value(&transport).expect("serializes");
        assert_eq!(payload["kind"], "transport");
        assert_eq!(payload["message"], "socket gone");

        let core = handshake_failure_to_result(&H::Core(
            whatsapp_rust::wacore::handshake::HandshakeError::Crypto("bug".into()),
        ));
        let payload = serde_json::to_value(&core).expect("serializes");
        assert_eq!(payload["kind"], "core");
        assert_eq!(payload["reason"]["kind"], "crypto");
        assert_eq!(payload["reason"]["detail"], "bug");

        for (failure, kind) in [
            (H::Timeout, "timeout"),
            (H::StreamClosed, "stream-closed"),
            (H::Disconnected, "disconnected"),
        ] {
            let payload =
                serde_json::to_value(handshake_failure_to_result(&failure)).expect("serializes");
            assert_eq!(payload["kind"], kind);
        }

        let unexpected = handshake_failure_to_result(&H::UnexpectedEvent("hello".into()));
        let payload = serde_json::to_value(&unexpected).expect("serializes");
        assert_eq!(payload["kind"], "unexpected-event");
        assert_eq!(payload["detail"], "hello");
    }

    #[test]
    fn every_noise_handshake_cause_keeps_its_shape() {
        use whatsapp_rust::wacore::handshake::HandshakeError as N;

        let decode = noise_handshake_failure_to_result(&N::ProtoDecode(
            whatsapp_rust::buffa::DecodeError::UnexpectedEof,
        ));
        let payload = serde_json::to_value(&decode).expect("serializes");
        assert_eq!(payload["kind"], "proto-decode");
        assert!(
            payload["message"]
                .as_str()
                .unwrap_or("")
                .contains("unexpected end")
        );

        let cert = noise_handshake_failure_to_result(&N::CertVerification("bad chain".into()));
        let payload = serde_json::to_value(&cert).expect("serializes");
        assert_eq!(payload["kind"], "cert-verification");
        assert_eq!(payload["detail"], "bad chain");

        let length = noise_handshake_failure_to_result(&N::InvalidLength {
            name: "server ephemeral key".into(),
            expected: 32,
            got: 10,
        });
        let payload = serde_json::to_value(&length).expect("serializes");
        assert_eq!(payload["kind"], "invalid-length");
        assert_eq!(payload["name"], "server ephemeral key");
        assert_eq!(payload["expected"], 32.0);
        assert_eq!(payload["got"], 10.0);

        let noise = noise_handshake_failure_to_result(&N::Noise(
            whatsapp_rust::wacore::noise::NoiseError::CiphertextTooShort,
        ));
        let payload = serde_json::to_value(&noise).expect("serializes");
        assert_eq!(payload["kind"], "noise");
        assert!(payload["message"].as_str().is_some());

        for (failure, kind) in [
            (N::IncompleteResponse, "incomplete-response"),
            (N::InvalidKeyLength, "invalid-key-length"),
        ] {
            let payload = serde_json::to_value(noise_handshake_failure_to_result(&failure))
                .expect("serializes");
            assert_eq!(payload["kind"], kind);
        }

        let crypto = noise_handshake_failure_to_result(&N::Crypto("bug".into()));
        let payload = serde_json::to_value(&crypto).expect("serializes");
        assert_eq!(payload["kind"], "crypto");
        assert_eq!(payload["detail"], "bug");
    }

    /// The `unknown` fallbacks carry the core's own rendering as their detail:
    /// a cause added upstream stays identifiable instead of taking a
    /// neighbour's name, and the key is always present when the kind is.
    #[test]
    fn unknown_fallbacks_carry_a_detail() {
        use crate::result_types::{
            ConnectErrorResult, HandshakeFailureResult, ProtocolTerminalReasonResult,
            RunCompletionResult,
        };

        let payload = payload_of(&RunCompletionResult::Unknown {
            generation: 0.0,
            detail: "SomeFutureReason".into(),
        });
        assert_eq!(payload["reason"], "unknown");
        assert_eq!(payload["detail"], "SomeFutureReason");

        for (payload, kind) in [
            (
                serde_json::to_value(&ConnectErrorResult::Unknown {
                    detail: "E::Future".into(),
                })
                .expect("serializes"),
                "unknown",
            ),
            (
                serde_json::to_value(&HandshakeFailureResult::Unknown {
                    detail: "H::Future".into(),
                })
                .expect("serializes"),
                "unknown",
            ),
            (
                serde_json::to_value(&ProtocolTerminalReasonResult::Unknown {
                    detail: "P::Future".into(),
                })
                .expect("serializes"),
                "unknown",
            ),
        ] {
            assert_eq!(payload["kind"], kind);
            assert!(payload["detail"].as_str().is_some());
        }
    }
    #[test]
    fn protocol_causes_cross_typed_with_the_event_spelling() {
        use whatsapp_rust::ProtocolTerminalReason as P;
        use whatsapp_rust::wacore::types::events::ConnectFailureReason as F;

        let code = protocol_terminal_to_result(&P::StreamErrorCode(401));
        let payload = serde_json::to_value(&code).expect("serializes");
        assert_eq!(payload["kind"], "stream-error");
        assert_eq!(payload["code"], 401.0);

        let failure = protocol_terminal_to_result(&P::ConnectFailure(F::LoggedOut));
        let payload = serde_json::to_value(&failure).expect("serializes");
        assert_eq!(payload["kind"], "connect-failure");
        assert_eq!(payload["reason"], "LoggedOut");

        let conflict = protocol_terminal_to_result(&P::Conflict);
        let payload = serde_json::to_value(&conflict).expect("serializes");
        assert_eq!(payload["kind"], "conflict");
    }

    /// The payload the host actually reads: the branch the reconnect-disabled
    /// exit took, with a connect failure and no reader outcome.
    #[test]
    fn a_failed_first_connect_reports_only_its_failure() {
        use whatsapp_rust::{ConnectError as E, RunCompletionReason as C};

        let payload = payload_of(&run_completion_to_result(
            0,
            &C::AutoReconnectDisabled {
                connection: None,
                connect_error: Some(E::Transport(anyhow::anyhow!("boom"))),
                protocol_error: None,
            },
        ));
        assert_eq!(payload["reason"], "auto-reconnect-disabled");
        assert_eq!(payload["connectError"]["kind"], "transport");
        assert_eq!(payload["connectError"]["message"], "boom");
        assert!(payload.get("connection").is_none());
        assert!(payload.get("protocolError").is_none());
    }
}
