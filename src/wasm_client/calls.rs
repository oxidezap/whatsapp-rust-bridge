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
        #[cfg(feature = "client-calls-media")]
        let offers = self.call_offers.clone();
        #[cfg(feature = "client-calls-media")]
        let offer_gen = super::calls_audio::current_offer_generation(&offers, &call_id);
        promise_void(async move {
            let peer = parse_named_jid("peer", &peer)?;
            let call_creator = parse_named_jid("callCreator", &call_creator)?;
            // ConnectionBound, not `online()`: a reject names a ringing
            // call, and a reconnect in flight may already have ended it.
            // Held past the new socket it would decline a call that is
            // gone, so it fails instead of waiting, like a receipt or
            // an ack.
            core.unwaited(Unwaited::ConnectionBound)
                .voip()
                .reject_call(&call_id, &peer, &call_creator)
                .await
                .map_err(crate::errors::BridgeError::from)?;
            // The ringing is over by our own hand: answering afterwards
            // would answer a declined call, so the retained offer goes
            // with it. A failed send keeps the offer — the call may
            // still be ringing. Only evict the generation that was rejected:
            // a replacement offer that arrived mid-await stays intact.
            #[cfg(feature = "client-calls-media")]
            if let Some(generation) = offer_gen {
                super::calls_audio::evict_offer_generation(&offers, &call_id, generation);
            }
            Ok(())
        })
    }

    /// Hang up an active call and return the core's termination outcome.
    #[wasm_bindgen(js_name = terminateCall, unchecked_return_type = "Promise<CallEndResult>")]
    pub fn terminate_call(
        &self,
        call_id: String,
        peer: String,
        call_creator: String,
    ) -> js_sys::Promise {
        let core = self.client.clone();
        #[cfg(feature = "client-calls-media")]
        let media = super::calls_audio::CallMedia::of(self);
        #[cfg(feature = "client-calls-media")]
        return super::calls_audio::promise_serialized(async move {
            let peer = parse_named_jid("peer", &peer)?;
            let call_creator = parse_named_jid("callCreator", &call_creator)?;
            super::calls_audio::terminate_call(&media, &core, call_id, peer, call_creator).await
        });
        #[cfg(not(feature = "client-calls-media"))]
        super::promise_serialized(async move {
            let peer = parse_named_jid("peer", &peer)?;
            let call_creator = parse_named_jid("callCreator", &call_creator)?;
            core.unwaited(Unwaited::ConnectionBound)
                .voip()
                .terminate(&call_id, &peer, &call_creator)
                .await
                .map_err(crate::errors::BridgeError::from)?;
            Ok(crate::result_types::CallEndResult::PeerNotified)
        })
    }
}
