//! Encoded-audio call media: answer, dial, push audio, stats, hangup.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.
//!
//! Signaling-only control stays in `calls.rs`. This module is the media half,
//! behind `client-calls-audio`, and it is what pulls the core's portable
//! `voip-encoded` profile: the sans-IO engine plus its crypto, no socket, no
//! codec. The relay socket is the host's: the bridge implements the core's
//! `RelayTransportProvider` over the callbacks installed with
//! `setRelayTransportProvider` (see `js_relay.rs`), and encoded packets cross
//! as `Uint8Array` views copied once each way.
//!
//! Retaining offers is the bridge's own job. `Voip::accept` borrows the full
//! `IncomingCall` — including media material that never crosses to JS — so
//! the offer cache below keeps what the `incoming_call` event carried until
//! the call is answered, superseded, or missed. Eviction mirrors the
//! downstream adapter's own offer cache: any non-offer update, miss, or
//! elsewhere-resolution for the id drops it.

use super::*;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use whatsapp_rust::voip::{CallHandle, CallTermination, VIDEO_UPGRADE_TIMEOUT};
use whatsapp_rust::wacore::types::call::IncomingCall;
use whatsapp_rust::wacore::types::events::Event;
use whatsapp_rust::wacore::types::group_call::{CallLinkMedia, ScreenShareState};
use whatsapp_rust::wacore::voip::{
    AudioCodec, AudioFormat, CallEvent, KeyframeUrgency, VideoFrame, VideoUpgradeToken,
};
use whatsapp_rust::{CallError, wacore};

/// Offers that rang and have not resolved yet, by call id. Inserted from the
/// event handler, consumed by `acceptCall`, evicted by anything that ends the
/// ringing. Bounded: concurrent ringing offers past this are absurd, and an
/// unbounded map would let a peer-sized trickle pin memory.
pub(super) type OfferCache = HashMap<String, IncomingCall>;

const OFFER_CACHE_CAPACITY: usize = 32;
/// Live calls by call id. A backstop, not a concurrency limit: the core owns
/// call policy, and 32 concurrent 1:1 calls is absurd — but absurd is not
/// impossible, so a full map refuses admission (before anything starts)
/// instead of silently dropping a live call, and a repeated id displaces
/// with a full terminate rather than coexisting.
const ACTIVE_CALL_CAPACITY: usize = 32;
/// Mic packets queued while the engine is busy. Voice cadence is one packet
/// per 20-60 ms, so this holds about a second; past it the bridge sheds
/// newest-first and says so, which is the loss-tolerant contract end to end.
const MIC_CHANNEL_CAPACITY: usize = 16;
/// Encoded packets queued for the host callback. The facade already sheds
/// into a full sink channel, so this only smooths callback jitter.
const SPEAKER_CHANNEL_CAPACITY: usize = 32;
/// Ended calls whose final counters stay readable. The core documents
/// post-end stats as the point of `media_stats`; evicting the record must
/// not take them with it.
const PAST_STATS_CAPACITY: usize = 8;

/// One live call: its handle, its mic queue, and the tasks pumping it.
pub(super) struct CallRecord {
    pub(super) handle: CallHandle,
    /// Distinguishes this registration from a same-id replacement. Every
    /// finish path removes only its own generation, so a racing end can
    /// never take down the call that superseded it.
    pub(super) generation: u64,
    pub(super) mic_tx: async_channel::Sender<Bytes>,
    /// A second reader on the mic queue, held so muting can drain the
    /// second of stale audio already queued (see `set_call_muted`).
    pub(super) mic_drain: async_channel::Receiver<Bytes>,
    /// Locally muted, applied at request time even when the announce
    /// below cannot reach the wire: `call_push_audio` sheds while set.
    pub(super) mic_muted: bool,
    /// A second reader on the decoded-audio queue, held so the watermark
    /// read does not disturb the pump that owns the first.
    pub(super) speaker_depth: async_channel::Receiver<wacore::voip::EncodedAudioFrame>,
    /// Camera queue and peer-video queue depth, present only while video
    /// is up. The pumps own the first readers; these seconds only measure.
    pub(super) video_tx: Option<async_channel::Sender<Vec<u8>>>,
    pub(super) video_in_depth: Option<async_channel::Receiver<VideoFrame>>,
    /// The peer's latest video-upgrade request, held because the token
    /// never crosses to JS and the core rejects a stale one on use.
    pub(super) pending_upgrade: Option<VideoUpgradeToken>,
    /// Speaker and end-watcher tasks, aborted at finish and at drop.
    pub(super) tasks: Vec<wacore::runtime::AbortHandle>,
    /// Event-forwarding task, aborted at drop only: it drains queued
    /// diagnostics after the record is gone, then exits on its own.
    pub(super) forwarder: Option<wacore::runtime::AbortHandle>,
}

/// Video pump bound, either way. Access units run larger than audio
/// packets (up to tens of kilobytes at 720p), so this holds fewer of
/// them for about the same fraction of a second.
const VIDEO_MIC_CAPACITY: usize = 8;
const VIDEO_SPK_CAPACITY: usize = 8;

/// Release one ringing offer by id. The reject/terminate methods call this
/// after a successful send; events call it through `note_call_event`.
/// Idempotent: resolving twice (a reject beside its own terminate event,
/// say) is not an error.
pub(super) fn evict_offer(cache: &Mutex<OfferCache>, call_id: &str) {
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(call_id);
}

/// Put a taken offer back after a failed start — only when nothing newer
/// took the slot meanwhile. A resolving event or a re-offer that arrived
/// mid-start owns it now; overwriting either would answer a dead call or
/// drop a live ringing.
fn restore_offer(cache: &Mutex<OfferCache>, call_id: String, offer: IncomingCall) {
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(call_id)
        .or_insert(offer);
}

/// Whether an action tag ends the ringing it arrived for. Only these
/// evict a retained offer: preaccept, transport candidates, relay
/// latency and video states all arrive while the call is still live,
/// and evicting on them would refuse an answer to a ringing call.
fn resolves_offer(action_tag: &str) -> bool {
    matches!(action_tag, "reject" | "terminate" | "accept")
}

/// Fold one core event into the offer cache. Offers are retained; anything
/// that resolves the ringing for an id — an update, a miss, an
/// elsewhere-resolution — releases it.
pub(super) fn note_call_event(cache: &Mutex<OfferCache>, event: &Event) {
    match event {
        Event::IncomingCall(call) => {
            // `wire_tag` rather than matching the variant: the action enum is
            // `#[non_exhaustive]`, and the tag is the whole of what the cache
            // decides on.
            if call.action.wire_tag() == "offer" {
                let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() >= OFFER_CACHE_CAPACITY && !cache.contains_key(call.action.call_id())
                {
                    // Arbitrary, and documented as such: past this many
                    // concurrent ringing offers something is wrong, and
                    // growing without bound is worse than dropping one ring.
                    if let Some(evicted) = cache.keys().next().cloned() {
                        cache.remove(&evicted);
                        log::warn!("Offer cache full; dropped ringing offer {evicted}");
                    }
                }
                cache.insert(call.action.call_id().to_owned(), (**call).clone());
            } else if resolves_offer(call.action.wire_tag()) {
                cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(call.action.call_id());
            }
        }
        Event::MissedCall(missed) => {
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&missed.call_id);
        }
        Event::CallEndedElsewhere(elsewhere) => {
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&elsewhere.call_id);
        }
        _ => {}
    }
}

/// One host media sink: the function plus the callbacks object it was read
/// off, which stays its receiver. A class-based host reads its state off
/// `this`, and invoking with NULL would detach it — the same reason the
/// relay adapter keeps its owning objects.
#[derive(Clone, Debug)]
pub(super) struct MediaCallback {
    func: js_sys::Function,
    this: JsValue,
}

impl MediaCallback {
    fn call(&self, arg: &JsValue) -> Result<JsValue, JsValue> {
        self.func.call1(&self.this, arg)
    }
}

/// Read one optional host callback off the callbacks object. Absent is the
/// normal case for a host that only signals; a present-but-unusable value is
/// ignored the same way, since these callbacks only ever fire into live
/// calls the host asked for.
/// Read one optional host callback off the callbacks object. Only
/// null/undefined is absent; a present-but-unusable value rejects client
/// construction, the way every other optional event method behaves — a
/// host that misspells the sink must hear it at install time, not as
/// silently missing audio on the first live call.
pub(super) fn media_callback(
    receiver: &JsValue,
    method: &'static str,
) -> Result<Option<MediaCallback>, crate::errors::BridgeError> {
    let value = js_sys::Reflect::get(receiver, &method.into()).map_err(|_| {
        crate::errors::invalid_arg("on_event", "could not read the media callbacks")
    })?;
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    let func = value.dyn_into::<js_sys::Function>().map_err(|_| {
        crate::errors::invalid_arg(format!("on_event.{method}"), "must be a function")
    })?;
    Ok(Some(MediaCallback {
        func,
        this: receiver.clone(),
    }))
}

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Call media ───────────────────────────────────────────────────────

    /// Answer a ringing call with encoded audio, and return its call id.
    ///
    /// The offer stays cached across a reconnect: a call held at the gate
    /// that is withdrawn or fails before starting leaves the cache intact,
    /// so asking again after the reconnect is a retry, not a repeat.
    /// Consumed only once the engine starts; a failed start keeps it for a
    /// retry with the other format.
    /// Answer a ringing call with encoded audio, and return its call id.
    ///
    /// The offer stays cached across a reconnect: a call held at the gate
    /// that is withdrawn or fails before starting leaves the cache intact,
    /// so asking again after the reconnect is a retry, not a repeat.
    /// Consumed only once the engine starts; a failed start keeps it for a
    /// retry with the other format.
    #[wasm_bindgen(js_name = acceptCall, unchecked_return_type = "Promise<string>")]
    pub fn accept_call(
        &self,
        call_id: String,
        #[wasm_bindgen(unchecked_param_type = "CallAudioFormat")] audio_format: JsValue,
    ) -> js_sys::Promise {
        // Synchronous prefix, owned future: see `CallMedia`.
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .accept_call(call_id, audio_format)
                .await
                .map(JsValue::from)
                .map_err(|e| bridge_error_to_js_value(&e))
        })
    }

    /// Dial a peer with encoded audio, and return the new call id.
    ///
    /// The handle is dormant until the server acks the offer with a relay;
    /// mic packets pushed before then queue bounded and shed oldest-first
    /// once live, so a host can start its capture at dial time.
    #[wasm_bindgen(js_name = dialCall, unchecked_return_type = "Promise<string>")]
    pub fn dial_call(
        &self,
        peer: String,
        #[wasm_bindgen(unchecked_param_type = "CallAudioFormat")] audio_format: JsValue,
    ) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .dial_call(peer, audio_format)
                .await
                .map(JsValue::from)
                .map_err(|e| bridge_error_to_js_value(&e))
        })
    }

    /// Push one encoded audio packet toward the peer.
    ///
    /// Resolves `true` when the packet entered the engine queue, `false`
    /// when it was shed under backpressure. Shedding is the normal
    /// loss-tolerant answer, not an error, which is why this resolves
    /// rather than rejects — and also the host's pacing signal, in place
    /// of a watermark readout the core does not expose yet.
    #[wasm_bindgen(js_name = callPushAudio)]
    pub fn call_push_audio(
        &self,
        call_id: &str,
        data: &[u8],
    ) -> Result<bool, crate::errors::BridgeError> {
        if data.is_empty() {
            return Err(crate::errors::invalid_arg(
                "data",
                "audio packet must not be empty",
            ));
        }
        let records = self.call_records.borrow();
        let Some(record) = records.get(call_id) else {
            return Err(unknown_call());
        };
        // A muted mic sheds like backpressure does: the peer hears nothing
        // either way, and the host paces on the same boolean.
        if record.mic_muted {
            return Ok(false);
        }
        // One copy on the way in, matching the documented boundary cost:
        // typed array into an owned packet the engine frames without
        // inspecting.
        match record.mic_tx.try_send(Bytes::copy_from_slice(data)) {
            Ok(()) => Ok(true),
            Err(async_channel::TrySendError::Full(_)) => Ok(false),
            // The engine is gone but the end watcher has not run yet; the
            // packet has nowhere to go, which reads the same as shed.
            Err(async_channel::TrySendError::Closed(_)) => Ok(false),
        }
    }

    /// End a live call through its handle, and report how much of the peer
    /// was told. The local side comes down whatever comes back — and it
    /// comes down now, not after a reconnect: this never parks behind one.
    /// A hangup held until a new socket would leave the peer talking while
    /// the user already hung up, and a withdrawn one would release without
    /// tearing anything down. The handle's own terminate degrades the same
    /// way, tearing down locally when the stanza cannot go out.
    #[wasm_bindgen(js_name = endCall, unchecked_return_type = "Promise<CallEndResult>")]
    pub fn end_call(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .end_call(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))
                .and_then(CallMedia::ok)
        })
    }

    /// Mute or unmute the mic on a live call.
    ///
    /// The local half applies at request time, reconnect or not: the mic
    /// stops queueing and the queued second drains, so the peer stops
    /// hearing audio now. The wire announce is best-effort past this point
    /// and its outcome is what crosses — an offline mute still holds
    /// locally, and asking again is idempotent.
    #[wasm_bindgen(js_name = setCallMuted, unchecked_return_type = "Promise<void>")]
    pub fn set_call_muted(&self, call_id: String, muted: bool) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .set_call_muted(call_id, muted)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Media counters for one call. Live calls read the handle; ended calls
    /// read the snapshot the finish path kept, which is what makes counters
    /// inspectable after `ended` fires.
    #[wasm_bindgen(js_name = getCallMediaStats)]
    pub fn call_media_stats(
        &self,
        call_id: &str,
    ) -> Result<Ts<crate::result_types::CallMediaStatsResult>, crate::errors::BridgeError> {
        if let Some(stats) = self
            .call_records
            .borrow()
            .get(call_id)
            .map(|record| call_media_stats_to_result(&record.handle.media_stats()))
        {
            return to_ts(stats);
        }
        if let Some(stats) = self
            .past_call_stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .find(|(id, _)| id == call_id)
            .map(|(_, stats)| stats.clone())
        {
            return to_ts(stats);
        }
        Err(unknown_call())
    }

    /// Every call the bridge currently holds a handle for.
    #[wasm_bindgen(js_name = getActiveCalls)]
    pub fn active_calls(&self) -> Vec<Ts<crate::result_types::ActiveCallResult>> {
        // `Ts` conversion cannot fail on these shapes (strings only), so a
        // bridge `internal` for a snapshot getter would be noise; hosts that
        // need failure semantics use the per-call methods.
        self.call_records
            .borrow()
            .values()
            .map(|record| {
                crate::result_types::ActiveCallResult {
                    call_id: record.handle.call_id().to_owned(),
                    peer_jid: record.handle.peer_jid().to_string(),
                }
                .into_ts()
                .expect("call id and peer JID serialize")
            })
            .collect()
    }

    /// Start sending our camera on a live call: attaches the video
    /// endpoints, enables the media plane, and offers the upgrade to the
    /// peer, which answers through the `video-state-changed` events. Pure
    /// encoded H.264 Annex-B — the bridge never touches pixels.
    ///
    /// Applies at request time like mute, not after a reconnect: the core
    /// validates (current handle, live client) before attaching anything,
    /// so an offline attempt fails without leaving a half-started plane.
    #[wasm_bindgen(js_name = startCallVideo, unchecked_return_type = "Promise<void>")]
    pub fn start_call_video(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .start_call_video(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Accept the peer's video upgrade request: attaches the endpoints and
    /// answers the handshake. The request token never crosses to JS — the
    /// forwarder holds the latest one per call, and the core rejects a
    /// stale token instead of attaching to the wrong request.
    #[wasm_bindgen(js_name = acceptCallVideo, unchecked_return_type = "Promise<void>")]
    pub fn accept_call_video(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .accept_call_video(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Stop our video direction: tears the local plane down first, then
    /// tells the peer. Audio is untouched. Idempotent.
    #[wasm_bindgen(js_name = stopCallVideo, unchecked_return_type = "Promise<void>")]
    pub fn stop_call_video(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .stop_call_video(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Re-add our stopped video direction without a second upgrade handshake.
    #[wasm_bindgen(js_name = resumeCallVideo, unchecked_return_type = "Promise<void>")]
    pub fn resume_call_video(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .resume_call_video(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Re-emit the video upgrade request for a live call. The core arms its
    /// direction-local timeout and leaves the attached endpoints in place.
    #[wasm_bindgen(js_name = retryCallVideoUpgrade, unchecked_return_type = "Promise<void>")]
    pub fn retry_call_video_upgrade(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .retry_call_video_upgrade(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Ask the peer for a video keyframe, by RTCP PLI. Call it when the
    /// decoder loses an access unit; the engine throttles, so every loss
    /// may ask. Fire-and-forget: nothing comes back.
    #[wasm_bindgen(js_name = requestCallKeyframe)]
    pub fn request_call_keyframe(
        &self,
        call_id: &str,
        #[wasm_bindgen(unchecked_param_type = "CallKeyframeUrgency")] urgency: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        // Input first, state second: a misspelled urgency is the caller's
        // own doing regardless of which call it names. Synchronous throughout,
        // so the wrapper borrow is always valid here.
        let urgency =
            from_js_input::<crate::result_types::CallKeyframeUrgency>("urgency", urgency)?;
        let handle = self
            .call_records
            .borrow()
            .get(call_id)
            .map(|record| record.handle.clone())
            .ok_or_else(unknown_call)?;
        let urgency = match urgency {
            crate::result_types::CallKeyframeUrgency::Coalesced => KeyframeUrgency::Coalesced,
            crate::result_types::CallKeyframeUrgency::Immediate => KeyframeUrgency::Immediate,
        };
        handle.request_peer_keyframe(urgency);
        Ok(())
    }

    /// Push one encoded H.264 Annex-B access unit toward the peer.
    /// Resolves `true` on queue, `false` on shed — the same loss-tolerant
    /// answer as audio, at video cadence.
    #[wasm_bindgen(js_name = callPushVideo)]
    pub fn call_push_video(
        &self,
        call_id: &str,
        data: &[u8],
    ) -> Result<bool, crate::errors::BridgeError> {
        if data.is_empty() {
            return Err(crate::errors::invalid_arg(
                "data",
                "video access unit must not be empty",
            ));
        }
        let records = self.call_records.borrow();
        let Some(record) = records.get(call_id) else {
            return Err(unknown_call());
        };
        let Some(video_tx) = record.video_tx.as_ref() else {
            return Err(crate::errors::invalid_arg(
                "callId",
                "video is not up for this call",
            ));
        };
        match video_tx.try_send(data.to_vec()) {
            Ok(()) => Ok(true),
            Err(async_channel::TrySendError::Full(_)) => Ok(false),
            Err(async_channel::TrySendError::Closed(_)) => Ok(false),
        }
    }

    /// Answer the eager preparation ping for an active group-call
    /// invitation. Moment-bound like a reject: a reconnect in flight may
    /// already have ended the ringing, so this fails instead of waiting.
    #[wasm_bindgen(js_name = preacceptGroupInvite, unchecked_return_type = "Promise<void>")]
    pub fn preaccept_group_invite(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .preaccept_group_invite(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Accept an active group-call invitation at the signaling level. The
    /// offer stays ringing afterwards, so a later slice can attach media
    /// to the same generation; joining live group media is not in this
    /// slice.
    #[wasm_bindgen(js_name = acceptGroupInvite, unchecked_return_type = "Promise<void>")]
    pub fn accept_group_invite(&self, call_id: String) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .accept_group_invite(call_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Create a reusable audio or video call link, and return its token
    /// and URL. An IQ round trip with no local effect, so it waits out a
    /// reconnect like any other query.
    #[wasm_bindgen(js_name = createCallLink, unchecked_return_type = "Promise<CallLinkResult>")]
    pub fn create_call_link(
        &self,
        #[wasm_bindgen(unchecked_param_type = "CallLinkMediaKind")] media: JsValue,
    ) -> js_sys::Promise {
        let media_ = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media_
                .create_call_link(media)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))
                .and_then(CallMedia::ok)
        })
    }

    /// Inspect a call link without joining it. Takes the token or the full
    /// `call.whatsapp.com` URL; the media must name the link's own mode.
    #[wasm_bindgen(js_name = previewCallLink, unchecked_return_type = "Promise<CallLinkPreviewResult>")]
    pub fn preview_call_link(
        &self,
        token_or_url: String,
        #[wasm_bindgen(unchecked_param_type = "CallLinkMediaKind")] media: JsValue,
    ) -> js_sys::Promise {
        let media_ = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media_
                .preview_call_link(token_or_url, media)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))
                .and_then(CallMedia::ok)
        })
    }

    /// Raise or lower our hand in a group call.
    #[wasm_bindgen(js_name = setGroupHandRaised, unchecked_return_type = "Promise<void>")]
    pub fn set_group_hand_raised(
        &self,
        call_id: String,
        call_creator: String,
        raised: bool,
    ) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .set_group_hand_raised(call_id, call_creator, raised)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Start or stop our screen share in a group call. The share id names
    /// our share stream when starting; it is absent when stopping.
    #[wasm_bindgen(js_name = setGroupScreenShare, unchecked_return_type = "Promise<void>")]
    pub fn set_group_screen_share(
        &self,
        call_id: String,
        call_creator: String,
        #[wasm_bindgen(unchecked_param_type = "GroupScreenShareState")] state: JsValue,
        screen_share_id: Option<f64>,
    ) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .set_group_screen_share(call_id, call_creator, state, screen_share_id)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Admit one user from a call-link waiting room.
    #[wasm_bindgen(js_name = admitWaitingUser, unchecked_return_type = "Promise<void>")]
    pub fn admit_waiting_user(
        &self,
        call_id: String,
        call_creator: String,
        user: String,
    ) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .admit_waiting_user(call_id, call_creator, user)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Deny one user from a call-link waiting room.
    #[wasm_bindgen(js_name = denyWaitingUser, unchecked_return_type = "Promise<void>")]
    pub fn deny_waiting_user(
        &self,
        call_id: String,
        call_creator: String,
        user: String,
    ) -> js_sys::Promise {
        let media = CallMedia::of(self);
        wasm_bindgen_futures::future_to_promise(async move {
            media
                .deny_waiting_user(call_id, call_creator, user)
                .await
                .map_err(|e| bridge_error_to_js_value(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Bridge pump depths for one call: packets queued, by direction. The
    /// host multiplies by its own packet duration for a live-bytes figure,
    /// which is the pacing signal the core does not expose yet.
    #[wasm_bindgen(js_name = getCallAudioBuffer)]
    pub fn call_audio_buffer(
        &self,
        call_id: &str,
    ) -> Result<Ts<crate::result_types::CallAudioBufferResult>, crate::errors::BridgeError> {
        let records = self.call_records.borrow();
        let Some(record) = records.get(call_id) else {
            return Err(unknown_call());
        };
        to_ts(crate::result_types::CallAudioBufferResult {
            outbound_queued: record.mic_tx.len() as f64,
            outbound_capacity: record.mic_tx.capacity().unwrap_or(0) as f64,
            inbound_queued: record.speaker_depth.len() as f64,
            inbound_capacity: record.speaker_depth.capacity().unwrap_or(0) as f64,
            video_outbound_queued: record.video_tx.as_ref().map(|tx| tx.len() as f64),
            video_inbound_queued: record.video_in_depth.as_ref().map(|rx| rx.len() as f64),
        })
    }

    /// Read the core's direction-local video state and timeout contract.
    #[wasm_bindgen(js_name = getCallVideoDiagnostics)]
    pub fn call_video_diagnostics(
        &self,
        call_id: &str,
    ) -> Result<Ts<crate::result_types::CallVideoDiagnosticsResult>, crate::errors::BridgeError>
    {
        to_ts(CallMedia::of(self).call_video_diagnostics(call_id)?)
    }

    /// Install the host's relay channel constructor. The core asks it for one
    /// channel per relay endpoint, and fails the call with a named setup
    /// error when none is installed.
    #[wasm_bindgen(js_name = setRelayTransportProvider)]
    pub fn set_relay_transport_provider(
        &self,
        #[wasm_bindgen(unchecked_param_type = "JsRelayProviderCallbacks")] provider: JsValue,
    ) -> Result<(), crate::errors::BridgeError> {
        let provider = crate::js_relay::JsRelayTransportProvider::from_js(provider)
            .map_err(|reason| crate::errors::invalid_arg("provider", reason))?;
        // Local install: nothing crosses the wire, so there is no connection
        // to wait for.
        self.client
            .unwaited(Unwaited::Local)
            .set_relay_transport_provider(Arc::new(provider));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Call registry
// ---------------------------------------------------------------------------

impl CallMedia {
    /// The handle and registration generation together, for post-await
    /// writes that must land on the same registration they read.
    fn live_record(&self, call_id: &str) -> Result<(CallHandle, u64), crate::errors::BridgeError> {
        self.call_records
            .borrow()
            .get(call_id)
            .map(|record| (record.handle.clone(), record.generation))
            .ok_or_else(unknown_call)
    }

    fn live_handle(&self, call_id: &str) -> Result<CallHandle, crate::errors::BridgeError> {
        self.live_record(call_id).map(|(handle, _)| handle)
    }

    /// A ringing offer by id, for the methods that answer one. Cloned out
    /// so no cache borrow crosses the core call; consumed or kept by the
    /// caller, never here.
    fn ringing_offer(&self, call_id: &str) -> Result<IncomingCall, crate::errors::BridgeError> {
        self.call_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(call_id)
            .cloned()
            .ok_or_else(|| {
                crate::errors::invalid_arg(
                    "callId",
                    "no live incoming offer for this call id (answered, missed, or never rang)",
                )
            })
    }

    /// Reserve a record slot before the first await. A count check alone
    /// is TOCTOU: concurrent starts each pass it, then all insert past
    /// capacity (and trip the debug assert below in debug builds). The
    /// reservation counts alongside the map until it converts into the
    /// record or its guard drops on any failure path. A repeated id never
    /// reserves: starting over it displaces instead of growing.
    fn reserve_call_slot(
        &self,
        call_id: Option<&str>,
        op: &'static str,
    ) -> Result<SlotGuard, crate::errors::BridgeError> {
        let known = call_id.is_some_and(|id| self.call_records.borrow().contains_key(id));
        let effective = self.call_records.borrow().len() + self.call_reserved.get() as usize;
        if !admits_call(effective, known) {
            // The operation, not an argument: no argument is wrong, and the
            // host remedies this by ending a call and retrying — the same
            // shape `connect()` uses when the call itself is the mistake.
            return Err(crate::errors::invalid_arg(
                op,
                "too many live calls (32); end one and retry",
            ));
        }
        if !known {
            self.call_reserved.set(self.call_reserved.get() + 1);
        }
        Ok(SlotGuard {
            reserved: if known {
                None
            } else {
                Some(self.call_reserved.clone())
            },
        })
    }

    /// End whatever holds this call id, so the newcomer takes a clean slot.
    ///
    /// Glare and retry can supersede a call under its own id. The old end
    /// watcher would otherwise outlive into the replacement and remove its
    /// record on firing — but `finish_call` aborts the old watcher's task
    /// with the old record, and every path here is synchronous past the
    /// terminate, so no interleaving can slip a removal between this and
    /// the insert below.
    async fn displace_call(&self, call_id: &str) {
        let old = self
            .call_records
            .borrow()
            .get(call_id)
            .map(|record| (record.handle.clone(), record.generation));
        let Some((old, generation)) = old else {
            return;
        };
        // The outcome is not the caller's: teardown runs regardless, and
        // `finish_call` reports the displacement through the ended event.
        // The generation pins the removal to what was read: a replacement
        // registered during the await keeps its record.
        let _ = old.terminate().await;
        self.finish_call(call_id, generation);
    }

    /// Store a started call, pump it, and return its id.
    fn register_call(
        &self,
        handle: CallHandle,
        mic_tx: async_channel::Sender<Bytes>,
        mic_drain: async_channel::Receiver<Bytes>,
        speaker_rx: async_channel::Receiver<wacore::voip::EncodedAudioFrame>,
    ) -> String {
        let speaker_depth = speaker_rx.clone();
        let call_id = handle.call_id().to_owned();
        // Reservation held across startup, displacement ran above, and no
        // await falls between here and the insert — so the bound holds and
        // the slot is this call's. The debug assert re-checks the bound,
        // not the decision.
        let generation = {
            let generation = self.call_generation.get();
            self.call_generation.set(generation.wrapping_add(1));
            generation
        };
        {
            let mut records = self.call_records.borrow_mut();
            debug_assert!(
                records.len() < ACTIVE_CALL_CAPACITY || records.contains_key(&call_id),
                "reservation held and displacement ran before registering"
            );
            records.insert(
                call_id.clone(),
                CallRecord {
                    handle: handle.clone(),
                    generation,
                    mic_tx,
                    mic_drain,
                    mic_muted: false,
                    speaker_depth,
                    video_tx: None,
                    video_in_depth: None,
                    pending_upgrade: None,
                    tasks: Vec::new(),
                    forwarder: None,
                },
            );
        }
        // The speaker and the end watcher are aborted at finish; the event
        // forwarder is not — it drains queued diagnostics after the record
        // is gone, then exits on its own (and at drop, like everything).
        // Every task learns the registration generation it serves, so a
        // same-id replacement never reads as its own call.
        let forwarder = self.spawn_call_event_task(&call_id, generation, handle.clone());
        let tasks = vec![
            self.spawn_speaker_task(&call_id, speaker_rx),
            self.spawn_call_end_task(&call_id, generation, handle),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if let Some(record) = self.call_records.borrow_mut().get_mut(&call_id) {
            record.tasks = tasks;
            record.forwarder = forwarder;
        }
        call_id
    }

    /// Forward encoded packets to the host audio callback. Only spawned when
    /// the host registered one; without it the facade sheds into the full
    /// sink channel, which is the same loss-tolerant answer with no task.
    fn spawn_speaker_task(
        &self,
        call_id: &str,
        speaker_rx: async_channel::Receiver<wacore::voip::EncodedAudioFrame>,
    ) -> Option<wacore::runtime::AbortHandle> {
        let callback = self.call_audio_callback.clone()?;
        let alive = self.calls_live.clone();
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            while let Ok(frame) = speaker_rx.recv().await {
                // Freed clients end here: aborting is signaled, and a pump
                // that outlives teardown must break rather than invoke a
                // host callback on its way out.
                if !alive.get() {
                    break;
                }
                let packet = js_sys::Object::new();
                let set =
                    |key: &str, value: &JsValue| js_sys::Reflect::set(&packet, &key.into(), value);
                // One copy on the way out: owned packet bytes into a typed
                // array the host decodes or plays.
                let data = js_sys::Uint8Array::from(frame.data.as_ref());
                if set("callId", &call_id.clone().into()).is_err()
                    || set("data", &data.into()).is_err()
                    || set("codec", &call_audio_codec_str(&frame.codec).into()).is_err()
                    || set("payloadType", &(f64::from(frame.payload_type)).into()).is_err()
                    || set("sequenceNumber", &(f64::from(frame.sequence_number)).into()).is_err()
                    || set("timestamp", &(f64::from(frame.timestamp)).into()).is_err()
                    || set("marker", &frame.marker.into()).is_err()
                {
                    log::error!("Audio packet object rejected its fields; stopping the pump");
                    break;
                }
                // A throwing callback is a broken host; stopping the pump
                // sheds into the facade's own drop counter rather than
                // throwing per packet for the rest of the call.
                if callback.call(&packet).is_err() {
                    log::error!("Call audio callback threw; stopping the pump for {call_id}");
                    break;
                }
            }
        })))
    }

    /// Forward encoded peer access units to the host video callback. Same
    /// loss-tolerant shape as the audio pump: only spawned with a
    /// registered sink, a throwing sink stops the pump, and the facade
    /// sheds into a full sink on its own.
    fn spawn_video_task(
        &self,
        call_id: &str,
        video_rx: async_channel::Receiver<VideoFrame>,
    ) -> Option<wacore::runtime::AbortHandle> {
        let callback = self.call_video_callback.clone()?;
        let alive = self.calls_live.clone();
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            while let Ok(frame) = video_rx.recv().await {
                if !alive.get() {
                    break;
                }
                let packet = js_sys::Object::new();
                let set =
                    |key: &str, value: &JsValue| js_sys::Reflect::set(&packet, &key.into(), value);
                // One copy on the way out: the AU bytes into a typed array.
                // Sender/device/pid stay out — they are group metadata, and
                // this slice carries no group media.
                let data = js_sys::Uint8Array::from(frame.data.as_slice());
                if set("callId", &call_id.clone().into()).is_err()
                    || set("data", &data.into()).is_err()
                    || set("keyframe", &frame.keyframe.into()).is_err()
                    || set("orientation", &(f64::from(frame.orientation)).into()).is_err()
                    || set("timestamp", &(f64::from(frame.timestamp)).into()).is_err()
                {
                    log::error!("Video packet object rejected its fields; stopping the pump");
                    break;
                }
                if callback.call(&packet).is_err() {
                    log::error!("Call video callback threw; stopping the pump for {call_id}");
                    break;
                }
            }
        })))
    }

    /// Forward the encoded-audio-relevant call events to the host event
    /// callback. Always spawned: the queue is bounded with eviction, so an
    /// undrained call would silently lose the events the host asked for.
    ///
    /// Outlives the record on purpose: when the call finishes while events
    /// are still queued, the loop below drains them instead of dropping a
    /// diagnostic the host never saw (a `media-setup-failed` arriving with
    /// the teardown, say). `ended` may already have fired ahead of them —
    /// the host correlates by call id, and a complete late picture beats a
    /// timely hole. The driver dropping its sender ends the loop; the
    /// iteration cap bounds a sender that never stops.
    fn spawn_call_event_task(
        &self,
        call_id: &str,
        generation: u64,
        handle: CallHandle,
    ) -> Option<wacore::runtime::AbortHandle> {
        let events = handle.events();
        let callback = self.call_event_callback.clone();
        let records = self.call_records.clone();
        let alive = self.calls_live.clone();
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            // The registration this task serves. A same-id replacement
            // carries another generation, and this task must neither
            // forward as it nor write its upgrade token.
            let current = || {
                records
                    .borrow()
                    .get(&call_id)
                    .map(|record| record.generation)
            };
            loop {
                // Freed clients end here instead of invoking a host
                // callback on the way out; aborting is signaled, and a
                // task that outlives teardown breaks instead.
                if !alive.get() {
                    break;
                }
                match current() {
                    Some(current) if current != generation => break,
                    None => {
                        // Finished, not replaced: drain what arrived, then
                        // exit. The drain is synchronous, so no replacement
                        // can slip in mid-loop; new arrivals past it lose
                        // the race openly rather than parking a dead task.
                        for _ in 0..FORWARDER_DRAIN_CAP {
                            if !alive.get() {
                                break;
                            }
                            let Ok(event) = events.try_recv() else {
                                break;
                            };
                            if !forward_engine_event(&records, &call_id, callback.as_ref(), &event)
                            {
                                break;
                            }
                        }
                        break;
                    }
                    _ => {}
                }
                match events.recv().await {
                    Ok(event) => {
                        // Revalidated after the await: a same-id replacement
                        // registered while suspended owns the id now, and
                        // this generation's event must neither read as its
                        // diagnostic nor write its upgrade token.
                        let current = records
                            .borrow()
                            .get(&call_id)
                            .map(|record| record.generation);
                        if current.is_some_and(|current| current != generation) {
                            break;
                        }
                        if !forward_engine_event(&records, &call_id, callback.as_ref(), &event) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })))
    }

    /// Watch for the end of the call, then release everything exactly once.
    /// Whoever removes the record — this watcher or `endCall` — emits the
    /// `ended` event; the other finds nothing and stays quiet.
    fn spawn_call_end_task(
        &self,
        call_id: &str,
        generation: u64,
        handle: CallHandle,
    ) -> Option<wacore::runtime::AbortHandle> {
        let records = self.call_records.clone();
        let past = self.past_call_stats.clone();
        let offers = self.call_offers.clone();
        let callback = self.call_event_callback.clone();
        let alive = self.calls_live.clone();
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            handle.wait_ended().await;
            if !alive.get() {
                return;
            }
            finish_call(
                &records,
                &past,
                &offers,
                callback.as_ref(),
                &alive,
                &call_id,
                generation,
            );
        })))
    }

    /// Whether an ended call still has readable counters.
    fn past_call_stats_has(&self, call_id: &str) -> bool {
        self.past_call_stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|(id, _)| id == call_id)
    }

    /// Release a call through the shared finish path below. The end watcher
    /// owns no client borrow, so both funnels meet there instead of here.
    /// The generation is the caller's own registration: a replacement that
    /// slipped in during the terminate keeps its record.
    fn finish_call(&self, call_id: &str, generation: u64) {
        finish_call(
            &self.call_records,
            &self.past_call_stats,
            &self.call_offers,
            self.call_event_callback.as_ref(),
            &self.calls_live,
            call_id,
            generation,
        );
    }
}

/// Forward one engine event, retaining a peer video-upgrade request on
/// the way past. The token never crosses to JS — the core rejects a stale
/// one on use — so the record holds the latest and the event only tells
/// the host to answer.
fn forward_engine_event(
    records: &RefCell<HashMap<String, CallRecord>>,
    call_id: &str,
    callback: Option<&MediaCallback>,
    event: &CallEvent,
) -> bool {
    if let CallEvent::VideoStateChanged {
        state,
        upgrade_token,
        ..
    } = event
    {
        // The token belongs to a live request only. A non-upgrade state
        // supersedes any request before it — a withdrawn or answered one
        // must not linger into a later accept as a seemingly pending
        // token the core would then refuse as stale.
        if state.is_upgrade_request() {
            if let Some(token) = upgrade_token
                && let Some(record) = records.borrow_mut().get_mut(call_id)
            {
                record.pending_upgrade = Some(*token);
            }
        } else if let Some(record) = records.borrow_mut().get_mut(call_id) {
            record.pending_upgrade = None;
        }
        let Some(callback) = callback else {
            return true;
        };
        let kind = if state.is_upgrade_request() {
            "video-upgrade-requested"
        } else {
            "video-state-changed"
        };
        let Ok(js_event) = call_event_object(call_id, kind) else {
            return true;
        };
        // The state crosses as the wire number the incoming-call event
        // already carries for video states — one spelling, both paths.
        if js_sys::Reflect::set(
            &js_event,
            &"state".into(),
            &(f64::from(state.code())).into(),
        )
        .is_err()
        {
            return true;
        }
        if callback.call(&js_event.into()).is_err() {
            log::error!("Call event callback threw; stopping forwarding for {call_id}");
            return false;
        }
        return true;
    }
    forward_call_event(call_id, callback, event)
}

/// Forward one engine event, when the host registered a sink and the
/// event has a shape in this slice. A throwing sink stops the pump;
/// shedding into the facade's counters beats throwing per event.
fn forward_call_event(call_id: &str, callback: Option<&MediaCallback>, event: &CallEvent) -> bool {
    let Some(callback) = callback else {
        return true;
    };
    let Some(js_event) = translate_call_event(call_id, event) else {
        return true;
    };
    if callback.call(&js_event).is_err() {
        log::error!("Call event callback threw; stopping forwarding for {call_id}");
        return false;
    }
    true
}

/// How many queued events the forwarder drains after the record is gone
/// before exiting regardless. teardown races can still be sending; an
/// unbounded drain would park a dead call's task on a wedged driver.
const FORWARDER_DRAIN_CAP: usize = 128;

/// Release a call: abort its media pump, drop its offer, keep its final
/// counters, and tell the host it ended. Idempotent — only the remover
/// emits, so a racing `endCall` and end watcher cannot double-report.
///
/// The event forwarder is deliberately not aborted here: it drains queued
/// diagnostics after the record is gone (see its task above). It is
/// aborted at drop, like everything, so no task outlives the client.
fn finish_call(
    records: &RefCell<HashMap<String, CallRecord>>,
    past: &Mutex<VecDeque<(String, crate::result_types::CallMediaStatsResult)>>,
    offers: &Mutex<OfferCache>,
    callback: Option<&MediaCallback>,
    live: &std::rc::Rc<std::cell::Cell<bool>>,
    call_id: &str,
    generation: u64,
) {
    // Generation-guarded: a same-id replacement registered while ending
    // (an endCall racing a new answer, a watcher outlived by a retry)
    // keeps its record, pumps, and controls. Only the remover emits.
    // Synchronous throughout, so the check and the removal are atomic.
    // The borrow scopes to the removal alone: the host callback below
    // may re-enter record-backed methods, and a live RefMut would turn
    // that ordinary reaction into a borrow panic.
    let record = {
        let mut records = records.borrow_mut();
        if records.get(call_id).map(|record| record.generation) != Some(generation) {
            return;
        }
        let Some(record) = records.remove(call_id) else {
            return;
        };
        record
    };
    for task in &record.tasks {
        task.abort();
    }
    offers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(call_id);
    let stats = call_media_stats_to_result(&record.handle.media_stats());
    // The final counters ride the event itself, so a host that only ever
    // listens learns the outcome without a follow-up read; the past-stats
    // map stays as the bounded convenience window for later reads.
    let stats_value = serde_wasm_bindgen::to_value(&stats)
        .map_err(|e| {
            log::error!("Ended stats refused to serialize: {e}");
        })
        .ok();
    {
        let mut past = past.lock().unwrap_or_else(|e| e.into_inner());
        past.push_back((call_id.to_owned(), stats));
        while past.len() > PAST_STATS_CAPACITY {
            past.pop_front();
        }
    }
    // A method future that outlives client teardown (an endCall parked
    // in terminate while the host frees) must not emit into it: the
    // liveness flag flips first in `Drop`, before any abort, so this
    // check observes teardown even though aborting is only signaled.
    if !live.get() {
        return;
    }
    if let Some(callback) = callback {
        match ended_event_object(call_id, stats_value.as_ref()) {
            Ok(event) => {
                if callback.call(&event.into()).is_err() {
                    log::error!("Call event callback threw on ended for {call_id}");
                }
            }
            Err(e) => log::error!("Ended event object rejected its fields: {e:?}"),
        }
    }
}

/// The `ended` event object: `{ callId, kind, stats? }`. The counters ride
/// along because the retention window above is bounded — nine endings
/// without a read would otherwise expire the oldest call's forensics.
fn ended_event_object(call_id: &str, stats: Option<&JsValue>) -> Result<js_sys::Object, JsValue> {
    let event = call_event_object(call_id, "ended")?;
    if let Some(stats) = stats {
        js_sys::Reflect::set(&event, &"stats".into(), stats)?;
    }
    Ok(event)
}

// ---------------------------------------------------------------------------
// Boundary shapes
// ---------------------------------------------------------------------------

/// Whether a call id may take a record slot: room, or a repeated id that
/// displaces instead of growing. Split out so the policy is pinnable
/// without a live call handle, which the test module cannot build.
fn admits_call(record_count: usize, known_id: bool) -> bool {
    known_id || record_count < ACTIVE_CALL_CAPACITY
}

/// Hang up by id: through the tracked handle when one exists, by raw
/// stanza otherwise. A live dial/accept handle owns pumps, tasks and
/// media the stanza alone never tears down; resolving after only sending
/// would leave the mic and relay running while reporting success — and a
/// disconnected send would reject while they keep running. The handle
/// path tears down locally whatever the wire did.
pub(super) async fn terminate_call(
    media: &CallMedia,
    core: &CoreClient,
    call_id: String,
    peer: Jid,
    call_creator: Jid,
) -> Result<(), crate::errors::BridgeError> {
    if media.call_records.borrow().contains_key(&call_id) {
        return media.end_call(call_id).await.map(|_| ());
    }
    core.unwaited(Unwaited::ConnectionBound)
        .voip()
        .terminate(&call_id, &peer, &call_creator)
        .await
        .map_err(crate::errors::BridgeError::from)?;
    // Success ends the ringing, failure keeps the offer for a retry.
    evict_offer(&media.call_offers, &call_id);
    Ok(())
}

/// Everything a call method touches, owned. Built synchronously at call
/// time while the wrapper is alive; the future then runs without a single
/// borrow on the wrapper, so a future that first-polls after `free()`
/// operates on live shared state instead of freed memory. That is the
/// whole of the free-safety story for this domain: `Drop` teardown still
/// runs at free, and the shutdown it signals is what settles the orphan.
#[derive(Clone)]
pub(super) struct CallMedia {
    client: CoreClient,
    call_records: std::rc::Rc<RefCell<HashMap<String, CallRecord>>>,
    call_offers: Arc<Mutex<OfferCache>>,
    past_call_stats: Arc<Mutex<VecDeque<(String, crate::result_types::CallMediaStatsResult)>>>,
    runtime: Arc<dyn wacore::runtime::Runtime>,
    call_audio_callback: Option<MediaCallback>,
    call_event_callback: Option<MediaCallback>,
    call_video_callback: Option<MediaCallback>,
    call_generation: std::rc::Rc<std::cell::Cell<u64>>,
    call_reserved: std::rc::Rc<std::cell::Cell<u32>>,
    calls_live: std::rc::Rc<std::cell::Cell<bool>>,
    call_admission: Arc<async_lock::Mutex<()>>,
}

impl CallMedia {
    pub(super) fn of(client: &WasmWhatsAppClient) -> Self {
        Self {
            client: client.client.clone(),
            call_records: client.call_records.clone(),
            call_offers: client.call_offers.clone(),
            past_call_stats: client.past_call_stats.clone(),
            runtime: client.runtime.clone(),
            call_audio_callback: client.call_audio_callback.clone(),
            call_event_callback: client.call_event_callback.clone(),
            call_video_callback: client.call_video_callback.clone(),
            call_generation: client.call_generation.clone(),
            call_reserved: client.call_reserved.clone(),
            calls_live: client.calls_live.clone(),
            call_admission: client.call_admission.clone(),
        }
    }

    /// Hand a typed result to the promise machinery, through the same
    /// JSON value the `Ts<T>` boundary would have carried: field names,
    /// number handling and absent keys match the declared TypeScript
    /// type exactly, because both read the same serde implementation.
    fn ok<T>(value: T) -> Result<JsValue, JsValue>
    where
        T: serde::Serialize,
    {
        serde_json::to_value(&value)
            .ok()
            .and_then(|json| serde_wasm_bindgen::to_value(&json).ok())
            .ok_or_else(|| {
                bridge_error_to_js_value(&crate::errors::internal(
                    "call result refused to serialize",
                ))
            })
    }

    async fn accept_call(
        &self,
        call_id: String,
        audio_format: JsValue,
    ) -> Result<String, crate::errors::BridgeError> {
        let format = call_audio_format(audio_format)?;
        let slot = self.reserve_call_slot(Some(&call_id), "acceptCall")?;
        let core = self.client.online().await?;
        // Taken, not cloned, and only after the gate: a concurrent second
        // answer must find nothing rather than answer the same offer twice,
        // and state may have changed while parked. A failed start puts it
        // back, so the refusal costs nothing either way.
        let offer = self
            .call_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&call_id)
            .ok_or_else(|| {
                crate::errors::invalid_arg(
                    "callId",
                    "no live incoming offer for this call id (answered, missed, or never rang)",
                )
            })?;
        let (mic_tx, mic_rx) = async_channel::bounded(MIC_CHANNEL_CAPACITY);
        let (speaker_tx, speaker_rx) = async_channel::bounded(SPEAKER_CHANNEL_CAPACITY);
        // A second reader for the mute drain; the engine owns the first
        // once the builder below takes it.
        let mic_drain = mic_rx.clone();
        let handle = core
            .voip()
            .accept(&offer)
            .encoded_audio(format, mic_rx, speaker_tx)
            .start()
            .await
            .map_err(|error| {
                restore_offer(&self.call_offers, call_id.clone(), offer);
                call_error_to_bridge(error)
            })?;
        // The engine owns the offer now; a re-answer would double-answer.
        // (The take above already consumed it; this only covers an offer
        // that arrived again under the same id while starting.)
        self.call_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(handle.call_id());
        // Serialized with concurrent starts under the admission lock:
        // two same-id starters would otherwise interleave termination and
        // insertion so the second insert silently drops the first
        // replacement's record. The lock spans the tail only, never core
        // startup, so unrelated calls never wait on each other.
        let _admission = self.call_admission.lock().await;
        self.displace_call(handle.call_id()).await;
        let id = self.register_call(handle, mic_tx, mic_drain, speaker_rx);
        slot.commit();
        Ok(id)
    }

    async fn end_call(
        &self,
        call_id: String,
    ) -> Result<crate::result_types::CallEndResult, crate::errors::BridgeError> {
        if !self.call_records.borrow().contains_key(&call_id) {
            // Gone already, or never live: ending it again is an answer
            // either way, and the past-stats map is what tells the two
            // apart for counters, not for this.
            if self.past_call_stats_has(&call_id) {
                return Ok(crate::result_types::CallEndResult::AlreadyEnded);
            }
            return Err(unknown_call());
        }
        // Read handle and generation together: a replacement registered
        // during the terminate below must not lose its record to the
        // finish, which removes only the generation it terminated.
        let watched = self
            .call_records
            .borrow()
            .get(&call_id)
            .map(|record| (record.handle.clone(), record.generation));
        let Some((handle, generation)) = watched else {
            return Ok(crate::result_types::CallEndResult::AlreadyEnded);
        };
        let outcome = handle.terminate().await;
        self.finish_call(&call_id, generation);
        Ok(call_termination_to_result(&outcome))
    }

    async fn start_call_video(&self, call_id: String) -> Result<(), crate::errors::BridgeError> {
        let (handle, generation) = self.live_record(&call_id)?;
        let (video_tx, video_rx) = async_channel::bounded(VIDEO_MIC_CAPACITY);
        let (sink_tx, sink_rx) = async_channel::bounded(VIDEO_SPK_CAPACITY);
        let sink_depth = sink_rx.clone();
        handle
            .start_video(video_rx, sink_tx)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        let mut records = self.call_records.borrow_mut();
        let Some(record) = records.get_mut(&call_id).filter(|record| {
            // A same-id replacement registered while starting owns the
            // slot now: storing our queues into it would cross two calls'
            // media. The core attached to our generation, and its own
            // teardown reaps what the replacement superseded.
            record.generation == generation
        }) else {
            return Ok(());
        };
        record.video_tx = Some(video_tx);
        record.video_in_depth = Some(sink_depth);
        if self.call_video_callback.is_some()
            && let Some(pump) = self.spawn_video_task(&call_id, sink_rx)
        {
            record.tasks.push(pump);
        }
        Ok(())
    }

    async fn accept_call_video(&self, call_id: String) -> Result<(), crate::errors::BridgeError> {
        let (handle, generation, token) = {
            let mut records = self.call_records.borrow_mut();
            let Some(record) = records.get_mut(&call_id) else {
                return Err(unknown_call());
            };
            let token = record.pending_upgrade.take();
            (record.handle.clone(), record.generation, token)
        };
        let Some(token) = token else {
            return Err(crate::errors::invalid_arg(
                "callId",
                "no pending video upgrade request for this call",
            ));
        };
        let (video_tx, video_rx) = async_channel::bounded(VIDEO_MIC_CAPACITY);
        let (sink_tx, sink_rx) = async_channel::bounded(VIDEO_SPK_CAPACITY);
        let sink_depth = sink_rx.clone();
        handle
            .accept_video(token, video_rx, sink_tx)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        if let Some(record) = self
            .call_records
            .borrow_mut()
            .get_mut(&call_id)
            .filter(|record| record.generation == generation)
        {
            record.video_tx = Some(video_tx);
            record.video_in_depth = Some(sink_depth);
            if self.call_video_callback.is_some()
                && let Some(pump) = self.spawn_video_task(&call_id, sink_rx)
            {
                record.tasks.push(pump);
            }
        }
        Ok(())
    }

    async fn stop_call_video(&self, call_id: String) -> Result<(), crate::errors::BridgeError> {
        let (handle, generation) = self.live_record(&call_id)?;
        handle
            .stop_video()
            .await
            .map_err(crate::errors::BridgeError::from)?;
        // Generation-guarded like the starts: clearing a replacement's
        // fresh video state for our stale stop would lie about its plane.
        if let Some(record) = self
            .call_records
            .borrow_mut()
            .get_mut(&call_id)
            .filter(|record| record.generation == generation)
        {
            record.video_tx = None;
            record.video_in_depth = None;
            record.pending_upgrade = None;
        }
        Ok(())
    }

    async fn resume_call_video(&self, call_id: String) -> Result<(), crate::errors::BridgeError> {
        let (handle, generation) = self.live_record(&call_id)?;
        let (video_tx, video_rx) = async_channel::bounded(VIDEO_MIC_CAPACITY);
        let (sink_tx, sink_rx) = async_channel::bounded(VIDEO_SPK_CAPACITY);
        let sink_depth = sink_rx.clone();
        handle
            .resume_video(video_rx, sink_tx)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        if let Some(record) = self
            .call_records
            .borrow_mut()
            .get_mut(&call_id)
            .filter(|record| record.generation == generation)
        {
            record.video_tx = Some(video_tx);
            record.video_in_depth = Some(sink_depth);
            if self.call_video_callback.is_some()
                && let Some(pump) = self.spawn_video_task(&call_id, sink_rx)
            {
                record.tasks.push(pump);
            }
        }
        Ok(())
    }

    async fn retry_call_video_upgrade(
        &self,
        call_id: String,
    ) -> Result<(), crate::errors::BridgeError> {
        let handle = self.live_handle(&call_id)?;
        handle
            .re_request_video_upgrade()
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    fn call_video_diagnostics(
        &self,
        call_id: &str,
    ) -> Result<crate::result_types::CallVideoDiagnosticsResult, crate::errors::BridgeError> {
        let handle = self.live_handle(call_id)?;
        let Some((self_state, peer_state)) = handle.video_states() else {
            return Err(unknown_call());
        };
        Ok(crate::result_types::CallVideoDiagnosticsResult {
            self_state: self_state.code() as f64,
            peer_state: peer_state.code() as f64,
            upgrade_timeout_ms: VIDEO_UPGRADE_TIMEOUT.as_millis() as f64,
        })
    }

    async fn set_call_muted(
        &self,
        call_id: String,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let handle = {
            let mut records = self.call_records.borrow_mut();
            let Some(record) = records.get_mut(&call_id) else {
                return Err(unknown_call());
            };
            record.mic_muted = muted;
            if muted {
                // Stale audio queued before the mute would otherwise play
                // out after it — up to a second of it. The engine keeps
                // pulling newer packets past the gap.
                while record.mic_drain.try_recv().is_ok() {}
            }
            record.handle.clone()
        };
        handle
            .set_muted(muted)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    async fn preaccept_group_invite(
        &self,
        call_id: String,
    ) -> Result<(), crate::errors::BridgeError> {
        let offer = self.ringing_offer(&call_id)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .preaccept_group_invite(&offer)
            .await
            .map_err(group_control_error)
    }

    async fn accept_group_invite(&self, call_id: String) -> Result<(), crate::errors::BridgeError> {
        let offer = self.ringing_offer(&call_id)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .accept_group_invite(&offer)
            .await
            .map_err(group_control_error)
    }

    async fn dial_call(
        &self,
        peer: String,
        audio_format: JsValue,
    ) -> Result<String, crate::errors::BridgeError> {
        let peer_jid = parse_named_jid("peer", &peer)?;
        let format = call_audio_format(audio_format)?;
        // The dial generates its id inside `start`, so only the count is
        // known yet; a same-id collision it produces is displaced below.
        // The guard releases on every failure path, converting only when
        // the record below inserts.
        let slot = self.reserve_call_slot(None, "dialCall")?;
        let (mic_tx, mic_rx) = async_channel::bounded(MIC_CHANNEL_CAPACITY);
        let (speaker_tx, speaker_rx) = async_channel::bounded(SPEAKER_CHANNEL_CAPACITY);
        let mic_drain = mic_rx.clone();
        let handle = self
            .client
            .online()
            .await?
            .voip()
            .call(&peer_jid)
            .encoded_audio(format, mic_rx, speaker_tx)
            .start()
            .await
            .map_err(call_error_to_bridge)?;
        // Serialized with concurrent starts under the admission lock:
        // two same-id starters would otherwise interleave termination and
        // insertion so the second insert silently drops the first
        // replacement's record. The lock spans the tail only, never core
        // startup, so unrelated calls never wait on each other.
        let _admission = self.call_admission.lock().await;
        self.displace_call(handle.call_id()).await;
        let id = self.register_call(handle, mic_tx, mic_drain, speaker_rx);
        slot.commit();
        Ok(id)
    }

    async fn create_call_link(
        &self,
        media: JsValue,
    ) -> Result<crate::result_types::CallLinkResult, crate::errors::BridgeError> {
        let media = from_js_input::<crate::result_types::CallLinkMediaKind>("media", media)?;
        let media = match media {
            crate::result_types::CallLinkMediaKind::Audio => CallLinkMedia::Audio,
            crate::result_types::CallLinkMediaKind::Video => CallLinkMedia::Video,
        };
        let link = self
            .client
            .online()
            .await?
            .voip()
            .create_call_link(media)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        let url = link.url();
        Ok(crate::result_types::CallLinkResult {
            token: link.token,
            media: link.media.as_str().to_owned(),
            url,
        })
    }

    async fn preview_call_link(
        &self,
        token_or_url: String,
        media: JsValue,
    ) -> Result<crate::result_types::CallLinkPreviewResult, crate::errors::BridgeError> {
        if token_or_url.trim().is_empty() {
            return Err(crate::errors::invalid_arg(
                "tokenOrUrl",
                "must not be empty",
            ));
        }
        let media = from_js_input::<crate::result_types::CallLinkMediaKind>("media", media)?;
        let media = match media {
            crate::result_types::CallLinkMediaKind::Audio => CallLinkMedia::Audio,
            crate::result_types::CallLinkMediaKind::Video => CallLinkMedia::Video,
        };
        let preview = self
            .client
            .online()
            .await?
            .voip()
            .preview_call_link(&token_or_url, media)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        Ok(crate::result_types::CallLinkPreviewResult {
            token: preview.token,
            media: preview.media.as_str().to_owned(),
            creator: preview.creator.to_string(),
            creator_pn: preview.creator_pn.as_ref().map(ToString::to_string),
            waiting_room_enabled: preview.waiting_room_enabled,
            is_admin: preview.is_admin,
        })
    }

    async fn set_group_hand_raised(
        &self,
        call_id: String,
        call_creator: String,
        raised: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let call_creator = parse_named_jid("callCreator", &call_creator)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .set_hand_raised(&call_id, &call_creator, raised)
            .await
            .map_err(group_control_error)
    }

    async fn set_group_screen_share(
        &self,
        call_id: String,
        call_creator: String,
        state: JsValue,
        screen_share_id: Option<f64>,
    ) -> Result<(), crate::errors::BridgeError> {
        let call_creator = parse_named_jid("callCreator", &call_creator)?;
        let state = from_js_input::<crate::result_types::GroupScreenShareState>("state", state)?;
        let state = match state {
            crate::result_types::GroupScreenShareState::Started => ScreenShareState::Started,
            crate::result_types::GroupScreenShareState::Stopped => ScreenShareState::Stopped,
        };
        let screen_share_id = parse_optional_u32("screenShareId", screen_share_id)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .set_screen_share(&call_id, &call_creator, state, screen_share_id)
            .await
            .map_err(group_control_error)
    }

    async fn admit_waiting_user(
        &self,
        call_id: String,
        call_creator: String,
        user: String,
    ) -> Result<(), crate::errors::BridgeError> {
        let call_creator = parse_named_jid("callCreator", &call_creator)?;
        let user = parse_named_jid("user", &user)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .admit_waiting_user(&call_id, &call_creator, &user)
            .await
            .map_err(group_control_error)
    }

    async fn deny_waiting_user(
        &self,
        call_id: String,
        call_creator: String,
        user: String,
    ) -> Result<(), crate::errors::BridgeError> {
        let call_creator = parse_named_jid("callCreator", &call_creator)?;
        let user = parse_named_jid("user", &user)?;
        self.client
            .unwaited(Unwaited::ConnectionBound)
            .voip()
            .deny_waiting_user(&call_id, &call_creator, &user)
            .await
            .map_err(group_control_error)
    }
}

/// A counted slot reservation, released unless committed. Held across the
/// core startup awaits so a concurrent start observes it; committing
/// converts the count into the record `register_call` inserts. Every
/// failure path drops it, which is what keeps the bound exact under
/// concurrency rather than checked once and hoped.
struct SlotGuard {
    reserved: Option<std::rc::Rc<std::cell::Cell<u32>>>,
}

impl SlotGuard {
    fn commit(mut self) {
        if let Some(counter) = self.reserved.take() {
            counter.set(counter.get().saturating_sub(1));
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Some(counter) = self.reserved.take() {
            counter.set(counter.get().saturating_sub(1));
        }
    }
}

/// The unknown-id rejection every per-call method shares.
fn unknown_call() -> crate::errors::BridgeError {
    crate::errors::invalid_arg(
        "callId",
        "no live call for this call id (ended or never started)",
    )
}

/// Map an identifier-keyed group control across. Three shapes name the
/// caller's own doing: a non-offer or non-group offer handed to an invite
/// method, and a registry guard reporting the call gone — a stale id is
/// not evidence the bridge broke. Everything else walks the chain.
///
/// The literals track the core's messages; re-check them on pin bumps,
/// since a reword upstream silently returns those paths to `internal`.
fn group_control_error(error: CallError) -> crate::errors::BridgeError {
    match &error {
        CallError::NotAnOffer => crate::errors::invalid_arg("callId", error.to_string()),
        CallError::Media(message)
            if message == &"offer is not an active group invitation"
                || message == &"call is no longer active" =>
        {
            crate::errors::invalid_arg("callId", message.to_string())
        }
        _ => crate::errors::BridgeError::from(error),
    }
}

/// Parse an optional u32 argument, rejecting what cannot be one. The
/// bridge names the field; the core never sees a float, a negative, or
/// an overflow.
fn parse_optional_u32(
    field: &'static str,
    value: Option<f64>,
) -> Result<Option<u32>, crate::errors::BridgeError> {
    match value {
        None => Ok(None),
        Some(value) => {
            if !value.is_finite()
                || value.fract() != 0.0
                || value < 0.0
                || value > f64::from(u32::MAX)
            {
                return Err(crate::errors::invalid_arg(
                    field,
                    "must be an integer 0..4294967295",
                ));
            }
            Ok(Some(value as u32))
        }
    }
}

/// Parse the encoded-audio promise. Required, with no bridge default: the
/// core negotiates from this promise and supplies none of its own, so a
/// silent MLOW would turn an absent caller choice into a failed negotiation
/// against an Opus-only peer. Validation happens before the gate.
fn call_audio_format(value: JsValue) -> Result<AudioFormat, crate::errors::BridgeError> {
    let format = from_js_input::<crate::result_types::CallAudioFormat>("audioFormat", value)?;
    Ok(match format {
        crate::result_types::CallAudioFormat::Mlow => AudioFormat::MLOW_16KHZ_60MS,
        // The in-profile Opus escape on the MLOW clock, not native RFC 7587:
        // same timing as `mlow`, decodable under the MLOW profile.
        crate::result_types::CallAudioFormat::Opus => AudioFormat::OPUS_MLOW_16KHZ_60MS,
    })
}

/// The JS spelling of a core audio codec, written down here rather than
/// taken from `Debug`. The core enum is `#[non_exhaustive]`, so the wildcard
/// keeps a variant added upstream identifiable instead of folding it into a
/// neighbour's name.
fn call_audio_codec_str(codec: &AudioCodec) -> String {
    match codec {
        AudioCodec::Mlow => "mlow".into(),
        AudioCodec::Opus => "opus".into(),
        other => format!("{other:?}"),
    }
}

/// Map an accept/dial failure across. Two shapes name something the host
/// acts on: the negotiation pair (retry with the other `audioFormat`), and
/// a missing own identity, which is the not-logged-in condition by another
/// name — the host fixes it by pairing, so it reports `not-connected`,
/// never `internal`. Everything else walks the chain: setup/media/response
/// failures carry no caller action, and a peer that hung up mid-setup is
/// already reported through the terminate event, so mapping them would
/// invent precision the bridge does not have.
///
/// The identity arm tracks the core's literal message; re-check it on pin
/// bumps, since a reword upstream silently returns this path to `internal`.
fn call_error_to_bridge(error: CallError) -> crate::errors::BridgeError {
    match &error {
        CallError::AudioFormatNotOffered(rate) => crate::errors::invalid_arg(
            "audioFormat",
            format!("the peer offered no audio at {rate} Hz; retry with the other format"),
        ),
        CallError::EncodedAudioCodecNotNegotiated { selected, .. } => crate::errors::invalid_arg(
            "audioFormat",
            format!(
                "the peer speaks {}; retry with that format",
                call_audio_codec_str(selected)
            ),
        ),
        CallError::Media(message) if message == &"no own LID" => {
            crate::errors::BridgeError::NotConnected
        }
        _ => crate::errors::BridgeError::from(error),
    }
}

fn call_termination_to_result(outcome: &CallTermination) -> crate::result_types::CallEndResult {
    use crate::result_types::CallEndResult as R;
    match outcome {
        CallTermination::PeerNotified => R::PeerNotified,
        CallTermination::PartlyNotified {
            notified,
            unconfirmed,
        } => R::PartlyNotified {
            notified: *notified as f64,
            unconfirmed: *unconfirmed as f64,
        },
        // The core's rendering is diagnostic text, not a boundary contract —
        // and the local side is down either way, which the kind already says.
        CallTermination::LocalOnly(error) => R::LocalOnly {
            failure: error.to_string(),
        },
        CallTermination::AlreadyEnded => R::AlreadyEnded,
        // `#[non_exhaustive]` forces a wildcard: a termination added
        // upstream keeps its own rendering as the failure rather than taking
        // a neighbour's outcome.
        other => R::LocalOnly {
            failure: format!("{other:?}"),
        },
    }
}

fn call_media_stats_to_result(
    stats: &wacore::voip::CallMediaStats,
) -> crate::result_types::CallMediaStatsResult {
    crate::result_types::CallMediaStatsResult {
        rtp_received: stats.rtp_received as f64,
        rtp_payload_type_unexpected: stats.rtp_payload_type_unexpected as f64,
        srtp_unprotect_failed: stats.srtp_unprotect_failed as f64,
        sframe_decrypt_failed: stats.sframe_decrypt_failed as f64,
        audio_frames_decoded: stats.audio_frames_decoded as f64,
        audio_frames_delivered: stats.audio_frames_delivered as f64,
        audio_frames_concealed: stats.audio_frames_concealed as f64,
        mlow_off_point_dropped: stats.mlow_off_point_dropped as f64,
        mlow_inactive_or_sid: stats.mlow_inactive_or_sid as f64,
        foreign_frames_decoded: stats.foreign_frames_decoded as f64,
        audio_frames_without_decoder: stats.audio_frames_without_decoder as f64,
        outbound_frames_without_encoder: stats.outbound_frames_without_encoder as f64,
        playout_trimmed_samples: stats.playout_trimmed_samples as f64,
        inbound_pipe_dropped: stats.inbound_pipe_dropped as f64,
        audio_sink_dropped: stats.audio_sink_dropped as f64,
        video_sink_dropped: stats.video_sink_dropped as f64,
        peer_keyframe_requests: stats.peer_keyframe_requests as f64,
        relay_packet_unclassified: stats.relay_packet_unclassified as f64,
        forwarding_envelope_rejected: stats.forwarding_envelope_rejected as f64,
        codec_switches: f64::from(stats.codec_switches),
    }
}

/// One call-media event object: `{ callId, kind, ... }`, where absence stays
/// absent — a code the event does not carry is no key at all.
fn call_event_object(call_id: &str, kind: &str) -> Result<js_sys::Object, JsValue> {
    let event = js_sys::Object::new();
    js_sys::Reflect::set(&event, &"callId".into(), &call_id.into())?;
    js_sys::Reflect::set(&event, &"kind".into(), &kind.into())?;
    Ok(event)
}

/// Translate one engine event for the host. Only the encoded-audio 1:1
/// subset crosses: group, video, reaction and RTCP events belong to later
/// slices, and an undrained queue would otherwise evict what the host asked
/// for. Anything unmapped is dropped with a debug log, never collapsed into
/// a neighbour's name.
fn translate_call_event(call_id: &str, event: &CallEvent) -> Option<JsValue> {
    let num = |event: &js_sys::Object, key: &str, value: f64| {
        js_sys::Reflect::set(event, &key.into(), &value.into())
    };
    let text = |event: &js_sys::Object, key: &str, value: &str| {
        js_sys::Reflect::set(event, &key.into(), &value.into())
    };
    let translated = match event {
        CallEvent::RelayAllocated => call_event_object(call_id, "relay-allocated").ok()?,
        CallEvent::RelayAllocateFailed(code) => {
            let event = call_event_object(call_id, "relay-allocate-failed").ok()?;
            num(&event, "code", f64::from(*code)).ok()?;
            event
        }
        CallEvent::RelayAllocateTimedOut => {
            call_event_object(call_id, "relay-allocate-timed-out").ok()?
        }
        CallEvent::MediaSetupFailed(detail) => {
            let event = call_event_object(call_id, "media-setup-failed").ok()?;
            text(&event, "detail", detail).ok()?;
            event
        }
        CallEvent::AudioCodecSwitched { from, to, .. } => {
            let event = call_event_object(call_id, "audio-codec-switched").ok()?;
            text(&event, "from", &call_audio_codec_str(from)).ok()?;
            text(&event, "to", &call_audio_codec_str(to)).ok()?;
            event
        }
        CallEvent::AudioCodecSourceIsFixed {
            sending,
            peer_expects,
            ..
        } => {
            let event = call_event_object(call_id, "audio-codec-source-fixed").ok()?;
            text(&event, "sending", &call_audio_codec_str(sending)).ok()?;
            text(&event, "peerExpects", &call_audio_codec_str(peer_expects)).ok()?;
            event
        }
        _ => {
            log::debug!("Call event for {call_id} has no host shape in this slice; dropped");
            return None;
        }
    };
    Some(translated.into())
}

#[cfg(test)]
mod call_media_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::wacore::stanza::call::parse_call_stanza;
    use whatsapp_rust::wacore_binary::jid::Jid;
    use whatsapp_rust::wacore_binary::node::{Attrs, Node, NodeContent, NodeValue};

    fn jid(user: &str) -> Jid {
        format!("{user}@s.whatsapp.net")
            .parse()
            .expect("test JID parses")
    }

    fn attr_string(attrs: &mut Attrs, key: &str, value: &str) {
        attrs.push(
            std::borrow::Cow::Owned(key.to_owned()),
            NodeValue::String(value.into()),
        );
    }

    /// A `<call>` stanza carrying one action child, parsed the way the live
    /// path parses it — so the cache tests exercise the real shape, not a
    /// restatement of the fields the cache reads.
    fn offered_call(call_id: &str, audio: &[(&str, &str)]) -> IncomingCall {
        let mut call_attrs = Attrs::new();
        call_attrs.push(
            std::borrow::Cow::Borrowed("from"),
            NodeValue::Jid(jid("5511999999999")),
        );
        attr_string(&mut call_attrs, "id", "STANZA-1");
        attr_string(&mut call_attrs, "t", "1766847151");

        let mut offer_attrs = Attrs::new();
        attr_string(&mut offer_attrs, "call-id", call_id);
        offer_attrs.push(
            std::borrow::Cow::Borrowed("call-creator"),
            NodeValue::Jid(jid("5511888888888")),
        );
        let audio_children = audio
            .iter()
            .map(|(enc, rate)| {
                let mut audio_attrs = Attrs::new();
                attr_string(&mut audio_attrs, "enc", enc);
                attr_string(&mut audio_attrs, "rate", rate);
                Node::new("audio", audio_attrs, None)
            })
            .collect::<Vec<_>>();
        let offer = Node::new(
            "offer",
            offer_attrs,
            Some(NodeContent::Nodes(audio_children)),
        );
        let node = Node::new("call", call_attrs, Some(NodeContent::Nodes(vec![offer])));
        parse_call_stanza(&node.as_node_ref())
            .expect("the test offer parses")
            .expect("the test offer is a call")
    }

    fn terminated_call(call_id: &str) -> IncomingCall {
        let mut call_attrs = Attrs::new();
        call_attrs.push(
            std::borrow::Cow::Borrowed("from"),
            NodeValue::Jid(jid("5511999999999")),
        );
        attr_string(&mut call_attrs, "id", "STANZA-2");
        attr_string(&mut call_attrs, "t", "1766847152");

        let mut term_attrs = Attrs::new();
        attr_string(&mut term_attrs, "call-id", call_id);
        term_attrs.push(
            std::borrow::Cow::Borrowed("call-creator"),
            NodeValue::Jid(jid("5511888888888")),
        );
        let terminate = Node::new("terminate", term_attrs, None);
        let node = Node::new(
            "call",
            call_attrs,
            Some(NodeContent::Nodes(vec![terminate])),
        );
        parse_call_stanza(&node.as_node_ref())
            .expect("the test terminate parses")
            .expect("the test terminate is a call")
    }

    fn cache() -> Mutex<OfferCache> {
        Mutex::new(OfferCache::default())
    }

    /// One non-terminal update for a ringing call: ICE candidates trickle
    /// while the peer waits for an answer, and none of it ends anything.
    fn transport_update(call_id: &str) -> IncomingCall {
        let mut call_attrs = Attrs::new();
        call_attrs.push(
            std::borrow::Cow::Borrowed("from"),
            NodeValue::Jid(jid("5511999999999")),
        );
        attr_string(&mut call_attrs, "id", "STANZA-3");
        attr_string(&mut call_attrs, "t", "1766847153");

        let mut transport_attrs = Attrs::new();
        attr_string(&mut transport_attrs, "call-id", call_id);
        transport_attrs.push(
            std::borrow::Cow::Borrowed("call-creator"),
            NodeValue::Jid(jid("5511888888888")),
        );
        let transport = Node::new("transport", transport_attrs, None);
        let node = Node::new(
            "call",
            call_attrs,
            Some(NodeContent::Nodes(vec![transport])),
        );
        parse_call_stanza(&node.as_node_ref())
            .expect("the test transport parses")
            .expect("the test transport is a call")
    }

    /// A peer rejection for a ringing call.
    fn rejected_call(call_id: &str) -> IncomingCall {
        let mut call_attrs = Attrs::new();
        call_attrs.push(
            std::borrow::Cow::Borrowed("from"),
            NodeValue::Jid(jid("5511999999999")),
        );
        attr_string(&mut call_attrs, "id", "STANZA-4");
        attr_string(&mut call_attrs, "t", "1766847154");

        let mut reject_attrs = Attrs::new();
        attr_string(&mut reject_attrs, "call-id", call_id);
        reject_attrs.push(
            std::borrow::Cow::Borrowed("call-creator"),
            NodeValue::Jid(jid("5511888888888")),
        );
        let reject = Node::new("reject", reject_attrs, None);
        let node = Node::new("call", call_attrs, Some(NodeContent::Nodes(vec![reject])));
        parse_call_stanza(&node.as_node_ref())
            .expect("the test reject parses")
            .expect("the test reject is a call")
    }

    #[test]
    fn offers_are_retained_until_something_resolves_them() {
        let cache = cache();
        let offer = offered_call("CALL-1", &[("opus", "16000")]);
        assert_eq!(offer.action.call_id(), "CALL-1");
        note_call_event(&cache, &Event::IncomingCall(Box::new(offer)));
        assert!(cache.lock().unwrap().contains_key("CALL-1"));

        // The peer hung up: the same id arriving as a non-offer releases it.
        let terminate = terminated_call("CALL-1");
        note_call_event(&cache, &Event::IncomingCall(Box::new(terminate)));
        assert!(!cache.lock().unwrap().contains_key("CALL-1"));
    }

    #[test]
    fn resolving_sends_release_the_offer() {
        // What rejectCall and terminateCall run after a successful send:
        // answering afterwards must find nothing, and resolving twice
        // (a reject beside its own terminate event) must not error.
        let cache = cache();
        let offer = offered_call("CALL-9", &[("opus", "16000")]);
        note_call_event(&cache, &Event::IncomingCall(Box::new(offer)));
        assert!(cache.lock().unwrap().contains_key("CALL-9"));
        evict_offer(&cache, "CALL-9");
        assert!(!cache.lock().unwrap().contains_key("CALL-9"));
        evict_offer(&cache, "CALL-9");
    }

    #[test]
    fn only_terminal_updates_evict_the_offer() {
        for (tag, resolves) in [
            ("offer", false),
            ("offer_notice", false),
            ("preaccept", false),
            ("transport", false),
            ("relaylatency", false),
            ("video", false),
            ("accept", true),
            ("reject", true),
            ("terminate", true),
        ] {
            assert_eq!(resolves_offer(tag), resolves, "tag {tag}");
        }
    }

    #[test]
    fn mid_ringing_traffic_keeps_the_offer_answerable() {
        let cache = cache();
        let offer = offered_call("CALL-T", &[("opus", "16000")]);
        note_call_event(&cache, &Event::IncomingCall(Box::new(offer)));
        // ICE trickles while the peer waits: still ringing, still cached.
        let transport = transport_update("CALL-T");
        note_call_event(&cache, &Event::IncomingCall(Box::new(transport)));
        assert!(cache.lock().unwrap().contains_key("CALL-T"));
        // The peer hangs up: now it is over.
        let reject = rejected_call("CALL-T");
        note_call_event(&cache, &Event::IncomingCall(Box::new(reject)));
        assert!(!cache.lock().unwrap().contains_key("CALL-T"));
    }

    #[test]
    fn a_failed_start_restores_only_an_empty_slot() {
        let cache = cache();
        // Nothing cached: the taken offer goes back.
        let old = offered_call("CALL-R", &[("opus", "16000")]);
        restore_offer(&cache, "CALL-R".to_owned(), old);
        assert!(cache.lock().unwrap().contains_key("CALL-R"));

        // A re-offer arrived meanwhile: the stale restore must not win.
        let newer = offered_call("CALL-R", &[("pcmu", "8000")]);
        cache.lock().unwrap().insert("CALL-R".to_owned(), newer);
        let stale = offered_call("CALL-R", &[("opus", "16000")]);
        restore_offer(&cache, "CALL-R".to_owned(), stale);
        let cached = cache.lock().unwrap();
        let kept_newer = match &cached.get("CALL-R").expect("still cached").action {
            whatsapp_rust::wacore::types::call::CallAction::Offer { audio, .. } => {
                audio.iter().any(|codec| codec.enc == "pcmu")
            }
            _ => false,
        };
        assert!(kept_newer, "the newer offer must survive");
    }

    #[test]
    fn unrelated_calls_do_not_evict_each_other() {
        let cache = cache();
        for id in ["CALL-A", "CALL-B"] {
            let offer = offered_call(id, &[("opus", "16000")]);
            note_call_event(&cache, &Event::IncomingCall(Box::new(offer)));
        }
        let terminate = terminated_call("CALL-A");
        note_call_event(&cache, &Event::IncomingCall(Box::new(terminate)));
        let cache = cache.lock().unwrap();
        assert!(!cache.contains_key("CALL-A"));
        assert!(cache.contains_key("CALL-B"));
    }

    #[test]
    fn admission_refuses_growth_but_never_a_takeover() {
        assert!(admits_call(0, false));
        assert!(admits_call(ACTIVE_CALL_CAPACITY - 1, false));
        assert!(!admits_call(ACTIVE_CALL_CAPACITY, false));
        assert!(!admits_call(ACTIVE_CALL_CAPACITY + 1, false));
        // A repeated id displaces instead of growing, at any size.
        assert!(admits_call(ACTIVE_CALL_CAPACITY, true));
        assert!(admits_call(ACTIVE_CALL_CAPACITY + 1, true));
    }

    #[test]
    fn the_cache_is_bounded() {
        let cache = cache();
        for n in 0..OFFER_CACHE_CAPACITY + 4 {
            let offer = offered_call(&format!("CALL-{n}"), &[("opus", "16000")]);
            note_call_event(&cache, &Event::IncomingCall(Box::new(offer)));
        }
        assert!(cache.lock().unwrap().len() <= OFFER_CACHE_CAPACITY);
    }

    /// The negotiation arm names the field the host retries with. The twin
    /// arm beside it shares the match and the rationale; its error value is
    /// a struct variant the core's `#[non_exhaustive]` keeps unbuildable
    /// outside the core, so no test here can hold one.
    #[test]
    fn an_unoffered_format_names_the_format_field() {
        match call_error_to_bridge(CallError::AudioFormatNotOffered(8000)) {
            crate::errors::BridgeError::InvalidArgument { field, reason } => {
                assert_eq!(field, "audioFormat");
                assert!(reason.contains("8000"), "unexpected reason: {reason}");
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
    }

    #[test]
    fn codecs_keep_their_spelling() {
        assert_eq!(call_audio_codec_str(&AudioCodec::Mlow), "mlow");
        assert_eq!(call_audio_codec_str(&AudioCodec::Opus), "opus");
    }

    #[test]
    fn the_format_promise_is_explicit() {
        // No bridge default: the core negotiates from this promise and
        // supplies none, so absence rejects rather than silently promising
        // MLOW against an Opus-only peer.
        match call_audio_format(JsValue::UNDEFINED) {
            Err(crate::errors::BridgeError::InvalidArgument { field, .. }) => {
                assert_eq!(field, "audioFormat")
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
        assert!(
            matches!(
                call_audio_format(JsValue::from_str("mlow")),
                Ok(format) if format == AudioFormat::MLOW_16KHZ_60MS
            ),
            "mlow must promise MLOW"
        );
        assert!(
            matches!(
                call_audio_format(JsValue::from_str("opus")),
                Ok(format) if format == AudioFormat::OPUS_MLOW_16KHZ_60MS
            ),
            "opus must promise the in-profile escape"
        );
        match call_audio_format(JsValue::from_str("g729")) {
            Err(crate::errors::BridgeError::InvalidArgument { field, .. }) => {
                assert_eq!(field, "audioFormat")
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_identity_is_not_connected() {
        match call_error_to_bridge(CallError::Media("no own LID")) {
            crate::errors::BridgeError::NotConnected => {}
            other => panic!("expected not-connected, got {other:?}"),
        }
    }

    #[test]
    fn an_unusable_media_callback_rejects_construction() {
        // A present-but-unusable sink must fail at install time, not as
        // silently missing audio on the first live call.
        let receiver = js_sys::Object::new();
        js_sys::Reflect::set(&receiver, &"onCallAudio".into(), &JsValue::from_f64(42.0))
            .expect("the test object accepts a key");
        match media_callback(&receiver.into(), "onCallAudio") {
            Err(crate::errors::BridgeError::InvalidArgument { field, .. }) => {
                assert_eq!(field, "on_event.onCallAudio")
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
        // An object without the sink reads as absent, the normal
        // signaling-only host.
        let bare = js_sys::Object::new();
        assert!(
            media_callback(&bare.into(), "onCallAudio")
                .expect("a missing sink is absent")
                .is_none()
        );
    }

    #[test]
    fn the_ended_event_carries_its_counters() {
        let stats = call_media_stats_to_result(&wacore::voip::CallMediaStats::default());
        let stats_value =
            serde_wasm_bindgen::to_value(&stats).expect("the stats result serializes");
        let event = ended_event_object("CALL-1", Some(&stats_value)).expect("the event builds");
        for key in ["callId", "kind", "stats"] {
            assert!(
                js_sys::Reflect::has(&event, &key.into()).expect("the event is inspectable"),
                "ended event is missing {key}"
            );
        }
        let kind = js_sys::Reflect::get(&event, &"kind".into()).expect("the event carries a kind");
        assert_eq!(kind.as_string().as_deref(), Some("ended"));

        // Without counters the key stays absent rather than null.
        let bare = ended_event_object("CALL-1", None).expect("the event builds");
        assert!(
            !js_sys::Reflect::has(&bare, &"stats".into()).expect("the event is inspectable"),
            "an absent stats must not become a key"
        );
    }

    /// Engine events the host asked for cross with their fields; everything
    /// else is dropped rather than renamed.
    #[test]
    fn engine_events_cross_typed_or_not_at_all() {
        let allocated = translate_call_event("CALL-1", &CallEvent::RelayAllocated)
            .expect("relay-allocated crosses");
        let kind =
            js_sys::Reflect::get(&allocated, &"kind".into()).expect("the event carries a kind");
        assert_eq!(kind.as_string().as_deref(), Some("relay-allocated"));

        let failed = translate_call_event("CALL-1", &CallEvent::RelayAllocateFailed(486))
            .expect("relay-allocate-failed crosses");
        let code = js_sys::Reflect::get(&failed, &"code".into()).expect("the event carries a code");
        assert_eq!(code.as_f64(), Some(486.0));

        let setup = translate_call_event(
            "CALL-1",
            &CallEvent::MediaSetupFailed("no provider".to_string()),
        )
        .expect("media-setup-failed crosses");
        let detail =
            js_sys::Reflect::get(&setup, &"detail".into()).expect("the event carries a detail");
        assert_eq!(detail.as_string().as_deref(), Some("no provider"));
    }
}

#[cfg(test)]
mod call_group_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::wacore::types::call::VideoState;

    #[test]
    fn group_invite_answers_name_the_call() {
        match group_control_error(CallError::NotAnOffer) {
            crate::errors::BridgeError::InvalidArgument { field, .. } => {
                assert_eq!(field, "callId")
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
        match group_control_error(CallError::Media("offer is not an active group invitation")) {
            crate::errors::BridgeError::InvalidArgument { field, reason } => {
                assert_eq!(field, "callId");
                assert!(
                    reason.contains("group invitation"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
        // A stale id names the call, not the bridge: the registry guard
        // firing means the invitation died, not that anything broke.
        match group_control_error(CallError::Media("call is no longer active")) {
            crate::errors::BridgeError::InvalidArgument { field, reason } => {
                assert_eq!(field, "callId");
                assert!(
                    reason.contains("no longer active"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
    }

    #[test]
    fn share_ids_parse_strictly() {
        assert_eq!(
            parse_optional_u32("screenShareId", None).expect("absent"),
            None
        );
        assert_eq!(
            parse_optional_u32("screenShareId", Some(3.0)).expect("integer"),
            Some(3)
        );
        for bad in [f64::NAN, -1.0, 1.5, f64::from(u32::MAX) + 1.0] {
            match parse_optional_u32("screenShareId", Some(bad)) {
                Err(crate::errors::BridgeError::InvalidArgument { field, .. }) => {
                    assert_eq!(field, "screenShareId")
                }
                other => panic!("expected invalid-argument for {bad}, got {other:?}"),
            }
        }
    }

    /// The video event path leans on these two core predicates: upgrade
    /// requests route to the token holder, and the state crosses as the
    /// wire number the incoming-call event already carries.
    #[test]
    fn video_states_route_and_spell() {
        assert!(VideoState::UpgradeRequest.is_upgrade_request());
        assert!(VideoState::UpgradeRequestV2.is_upgrade_request());
        assert!(!VideoState::Enabled.is_upgrade_request());
        assert_eq!(VideoState::Enabled.code(), 1);
        assert_eq!(VideoState::Stopped.code(), 6);
        assert_eq!(VideoState::UpgradeRequestV2.code(), 11);
    }
}
