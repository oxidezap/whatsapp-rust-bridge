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
    #[wasm_bindgen(js_name = rejectCall)]
    pub async fn reject_call(
        &self,
        call_id: &str,
        peer: &str,
        call_creator: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let peer = parse_named_jid("peer", peer)?;
        let call_creator = parse_named_jid("callCreator", call_creator)?;
        // ConnectionBound, not `online()`: a reject names a ringing call, and
        // a reconnect in flight may already have ended it. Held past the new
        // socket it would decline a call that is gone, so it fails instead of
        // waiting, like a receipt or an ack.
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .reject_call(call_id, &peer, &call_creator)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        // The ringing is over by our own hand: answering afterwards would
        // answer a declined call, so the retained offer goes with it. A
        // failed send keeps the offer — the call may still be ringing.
        #[cfg(feature = "client-calls-audio")]
        super::calls_audio::evict_offer(&self.call_offers, call_id);
        Ok(())
    }

    /// Hang up an active call.
    ///
    /// Same fire-and-forget shape as [`Self::reject_call`]: resolving means
    /// the `<terminate>` stanza went out. Same argument sources, and the same
    /// gate for the same reason — a terminate names a live call, not a state
    /// worth carrying across a reconnect.
    #[wasm_bindgen(js_name = terminateCall)]
    pub async fn terminate_call(
        &self,
        call_id: &str,
        peer: &str,
        call_creator: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let peer = parse_named_jid("peer", peer)?;
        let call_creator = parse_named_jid("callCreator", call_creator)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .terminate(call_id, &peer, &call_creator)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        // Same ownership as the reject above: success ends the ringing,
        // failure keeps the offer for a retry.
        #[cfg(feature = "client-calls-audio")]
        super::calls_audio::evict_offer(&self.call_offers, call_id);
        Ok(())
    }
}
