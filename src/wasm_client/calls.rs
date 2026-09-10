//! Signaling-only call control: reject and terminate.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.
//!
//! No media crosses here. The core's `Client::voip()` reject/terminate paths
//! stay available without `voip-runtime` (their stanza builders live in the
//! core), so this domain enables no VoIP feature and adds no dependency.
//! Incoming call state already crosses as `incoming_call` / `missed_call` /
//! `call_ended_elsewhere` events, and the core acks every `<call>` stanza
//! itself, so there is no ack method to expose. Shaping those events for a
//! downstream `WACallEvent` consumer (including the offer cache that fills in
//! `isVideo` on non-offer updates) belongs to the JS adapter above, which is
//! the only place that knows that shape.

use super::*;

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Calls ────────────────────────────────────────────────────────────

    /// Decline an incoming call.
    ///
    /// Fire-and-forget: no server response is expected, and resolving means
    /// the `<reject>` stanza went out, not that the peer stopped ringing.
    /// Take the identifiers from the `incoming_call` event that rang:
    /// `callId` is the action's call id, `peer` the event's sender, and
    /// `callCreator` the action's call creator. The two JIDs stay separate
    /// because companion-device signaling can address them differently.
    #[wasm_bindgen(js_name = rejectCall, unchecked_return_type = "Promise<void>")]
    pub fn reject_call(
        &self,
        call_id: String,
        peer: String,
        call_creator: String,
    ) -> js_sys::Promise {
        // Synchronous prefix, owned future: a future that first-polls
        // after `free()` must never touch freed wrapper memory (see
        // `CoreClient`). The peer JIDs parse without the wrapper; the
        // core client and the offer cache cross owned.
        let core = self.client.clone();
        #[cfg(feature = "client-calls-audio")]
        let offers = self.call_offers.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            // Mapped explicitly: `From<BridgeError> for JsValue` exists
            // only on wasm32, and `?` inside a JsValue future would lean
            // on it, breaking host builds.
            let peer = parse_named_jid("peer", &peer).map_err(|e| bridge_error_to_js_value(&e))?;
            let call_creator = parse_named_jid("callCreator", &call_creator)
                .map_err(|e| bridge_error_to_js_value(&e))?;
            // ConnectionBound, not `online()`: a reject names a ringing
            // call, and a reconnect in flight may already have ended it.
            // Held past the new socket it would decline a call that is
            // gone, so it fails instead of waiting, like a receipt or
            // an ack.
            core.unwaited(Unwaited::ConnectionBound)
                .voip()
                .reject_call(&call_id, &peer, &call_creator)
                .await
                .map_err(crate::errors::BridgeError::from)
                .map_err(|e| bridge_error_to_js_value(&e))?;
            // The ringing is over by our own hand: answering afterwards
            // would answer a declined call, so the retained offer goes
            // with it. A failed send keeps the offer — the call may
            // still be ringing.
            #[cfg(feature = "client-calls-audio")]
            super::calls_audio::evict_offer(&offers, &call_id);
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Hang up an active call.
    ///
    /// Same fire-and-forget shape as [`Self::reject_call`]: resolving means
    /// the `<terminate>` stanza went out. Same argument sources, and the same
    /// gate for the same reason — a terminate names a live call, not a state
    /// worth carrying across a reconnect.
    #[wasm_bindgen(js_name = terminateCall, unchecked_return_type = "Promise<void>")]
    pub fn terminate_call(
        &self,
        call_id: String,
        peer: String,
        call_creator: String,
    ) -> js_sys::Promise {
        let core = self.client.clone();
        #[cfg(feature = "client-calls-audio")]
        let media = super::calls_audio::CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            let peer = parse_named_jid("peer", &peer).map_err(|e| bridge_error_to_js_value(&e))?;
            let call_creator = parse_named_jid("callCreator", &call_creator)
                .map_err(|e| bridge_error_to_js_value(&e))?;
            #[cfg(feature = "client-calls-audio")]
            {
                return super::calls_audio::terminate_call(
                    &media,
                    &core,
                    call_id,
                    peer,
                    call_creator,
                )
                .await
                .map_err(|e| bridge_error_to_js_value(&e))
                .map(|_| JsValue::UNDEFINED);
            }
            #[cfg(not(feature = "client-calls-audio"))]
            {
                core.unwaited(Unwaited::ConnectionBound)
                    .voip()
                    .terminate(&call_id, &peer, &call_creator)
                    .await
                    .map_err(crate::errors::BridgeError::from)
                    .map_err(|e| bridge_error_to_js_value(&e))?;
                Ok(JsValue::UNDEFINED)
            }
        })
    }
}
