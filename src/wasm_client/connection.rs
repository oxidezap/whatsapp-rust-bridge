//! Connection, device props, pairing and state getters.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

fn abort_teardown_task(slot: &Rc<RefCell<Option<wacore::runtime::AbortHandle>>>) {
    let handle = slot.borrow_mut().take();
    if let Some(handle) = handle {
        handle.abort();
    }
}

fn detach_teardown_task(slot: &Rc<RefCell<Option<wacore::runtime::AbortHandle>>>) {
    let handle = slot.borrow_mut().take();
    if let Some(handle) = handle {
        handle.detach();
    }
}

impl WasmWhatsAppClient {
    fn execute_run(&mut self) -> Result<(), crate::errors::BridgeError> {
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
            *self.sync_worker_handle.borrow_mut() = Some(handle);
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
            let result = run_completion_to_result(generation, &client.run().await);
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
        *self.run_handle.borrow_mut() = Some(handle);

        Ok(())
    }

    fn prepare_wait_for_run_completion(
        &self,
    ) -> impl std::future::Future<Output = Result<JsValue, JsValue>> + use<> + 'static {
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
        async move {
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
        }
    }

    fn prepare_connect(
        &self,
    ) -> impl std::future::Future<Output = Result<JsValue, JsValue>> + use<> + 'static {
        if self.connection_handle.borrow().is_some() {
            return futures::future::Either::Left(async {
                Err(bridge_error_to_js_value(&crate::errors::invalid_arg(
                    "connect",
                    "connect() is already in progress",
                )))
            });
        }

        let client = self.client.unwaited(Unwaited::ThisSocket).clone();
        let runtime = self.runtime.clone();
        let connection_handle = self.connection_handle.clone();
        let connection_established = self.connection_established.clone();
        let reader_handle_slot = connection_handle.clone();
        let reader_established = connection_established.clone();
        let (handshake_tx, handshake_rx) = async_channel::bounded(1);

        // Clone every value used after the first await. This keeps `free()`
        // from dropping a wrapper borrowed by a pending connect promise.
        let handle = runtime.spawn(Box::pin(async move {
            match client.connect().await {
                Err(e) => {
                    let _ = handshake_tx.send(Err(e)).await;
                }
                Ok(connection) => {
                    reader_established.store(true, std::sync::atomic::Ordering::Release);
                    let _ = handshake_tx.send(Ok(())).await;
                    let _ = connection.read_until_disconnected().await;
                    info!("Client connection reader exited.");
                    reader_established.store(false, std::sync::atomic::Ordering::Release);
                    detach_teardown_task(&reader_handle_slot);
                }
            }
        }));
        // Publish the abort handle before waiting for the handshake. A host
        // can free a client while transport.connect() is still pending.
        *connection_handle.borrow_mut() = Some(handle);

        futures::future::Either::Right(async move {
            let handshake = handshake_rx
                .recv()
                .await
                .map_err(|_| bridge_error_to_js_value(&crate::errors::BridgeError::NotConnected))?;

            match handshake {
                Ok(()) => Ok(JsValue::UNDEFINED),
                Err(e) => {
                    connection_established.store(false, std::sync::atomic::Ordering::Release);
                    abort_teardown_task(&connection_handle);
                    Err(bridge_error_to_js_value(&crate::errors::BridgeError::from(
                        e,
                    )))
                }
            }
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

#[cfg(test)]
use super::connect_stage_str;
use crate::result_types::run_completion_to_result;
#[cfg(test)]
use crate::result_types::{
    connect_error_to_result, disconnect_reason_to_result, protocol_terminal_to_result,
};
#[cfg(test)]
use crate::result_types::{handshake_failure_to_result, noise_handshake_failure_to_result};

#[cfg(test)]
mod run_completion_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn local_task_slot_releases_its_borrow_before_cancellation() {
        let slot = Rc::new(RefCell::new(None));
        let weak = Rc::downgrade(&slot);
        let cancellations = Rc::new(std::cell::Cell::new(0));
        let observed = cancellations.clone();
        *slot.borrow_mut() = Some(wacore::runtime::AbortHandle::new(move || {
            assert!(weak.upgrade().unwrap().borrow_mut().is_none());
            observed.set(observed.get() + 1);
        }));
        abort_teardown_task(&slot);
        abort_teardown_task(&slot);
        assert_eq!(cancellations.get(), 1);

        let observed = cancellations.clone();
        *slot.borrow_mut() = Some(wacore::runtime::AbortHandle::new(move || {
            observed.set(observed.get() + 1);
        }));
        detach_teardown_task(&slot);
        assert!(slot.borrow().is_none());
        assert_eq!(cancellations.get(), 1);
    }

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

        for (cause, expected) in [
            (whatsapp_rust::ConflictKind::Replaced, "replaced"),
            (whatsapp_rust::ConflictKind::DeviceRemoved, "device_removed"),
            (whatsapp_rust::ConflictKind::Unknown, "unknown"),
        ] {
            let conflict = protocol_terminal_to_result(&P::Conflict(cause));
            let payload = serde_json::to_value(&conflict).expect("serializes");
            assert_eq!(payload["kind"], "conflict");
            assert_eq!(payload["cause"], expected);
        }
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

include!("generated/connection.rs");

include!("generated/teardown.rs");
