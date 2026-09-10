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
use whatsapp_rust::voip::{CallHandle, CallTermination};
use whatsapp_rust::wacore::types::call::IncomingCall;
use whatsapp_rust::wacore::types::events::Event;
use whatsapp_rust::wacore::voip::{AudioCodec, AudioFormat, CallEvent};
use whatsapp_rust::{CallError, wacore};

/// Offers that rang and have not resolved yet, by call id. Inserted from the
/// event handler, consumed by `acceptCall`, evicted by anything that ends the
/// ringing. Bounded: concurrent ringing offers past this are absurd, and an
/// unbounded map would let a peer-sized trickle pin memory.
pub(super) type OfferCache = HashMap<String, IncomingCall>;

const OFFER_CACHE_CAPACITY: usize = 32;
/// Live calls by call id. A backstop, not a concurrency limit: the core owns
/// call policy, and a same-id replacement supersedes rather than coexists.
const ACTIVE_CALL_CAPACITY: usize = 32;
/// Mic packets queued while the engine is busy. Voice cadence is one packet
/// per 20-60 ms, so this holds about a second; past it the bridge sheds
/// newest-first and says so, which is the loss-tolerant contract end to end.
const MIC_CHANNEL_CAPACITY: usize = 16;
/// Decoded packets queued for the host callback. The facade already sheds
/// into a full sink, so this only smooths callback jitter.
const SPEAKER_CHANNEL_CAPACITY: usize = 32;
/// Ended calls whose final counters stay readable. The core documents
/// post-end stats as the point of `media_stats`; evicting the record must
/// not take them with it.
const PAST_STATS_CAPACITY: usize = 8;

/// One live call: its handle, its mic queue, and the tasks pumping it.
pub(super) struct CallRecord {
    pub(super) handle: CallHandle,
    pub(super) mic_tx: async_channel::Sender<Bytes>,
    pub(super) tasks: Vec<wacore::runtime::AbortHandle>,
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
            } else {
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

/// Read one optional host callback off the callbacks object. Absent is the
/// normal case for a host that only signals; a present-but-unusable value is
/// ignored the same way, since these callbacks only ever fire into live
/// calls the host asked for.
pub(super) fn optional_callback(
    receiver: &JsValue,
    method: &'static str,
) -> Option<js_sys::Function> {
    js_sys::Reflect::get(receiver, &method.into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()
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
    #[wasm_bindgen(js_name = acceptCall)]
    pub async fn accept_call(
        &self,
        call_id: &str,
        #[wasm_bindgen(unchecked_param_type = "CallAudioFormat | null | undefined")]
        audio_format: Option<JsValue>,
    ) -> Result<String, crate::errors::BridgeError> {
        let format = call_audio_format(audio_format)?;
        let offer = self
            .call_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(call_id)
            .cloned()
            .ok_or_else(|| {
                crate::errors::invalid_arg(
                    "callId",
                    "no live incoming offer for this call id (answered, missed, or never rang)",
                )
            })?;
        let (mic_tx, mic_rx) = async_channel::bounded(MIC_CHANNEL_CAPACITY);
        let (speaker_tx, speaker_rx) = async_channel::bounded(SPEAKER_CHANNEL_CAPACITY);
        let handle = self
            .client
            .online()
            .await?
            .voip()
            .accept(&offer)
            .encoded_audio(format, mic_rx, speaker_tx)
            .start()
            .await
            .map_err(call_error_to_bridge)?;
        // The engine owns the offer now; a re-answer would double-answer.
        self.call_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(handle.call_id());
        Ok(self.register_call(handle, mic_tx, speaker_rx))
    }

    /// Dial a peer with encoded audio, and return the new call id.
    ///
    /// The handle is dormant until the server acks the offer with a relay;
    /// mic packets pushed before then queue bounded and shed oldest-first
    /// once live, so a host can start its capture at dial time.
    #[wasm_bindgen(js_name = dialCall)]
    pub async fn dial_call(
        &self,
        peer: &str,
        #[wasm_bindgen(unchecked_param_type = "CallAudioFormat | null | undefined")]
        audio_format: Option<JsValue>,
    ) -> Result<String, crate::errors::BridgeError> {
        let peer_jid = parse_named_jid("peer", peer)?;
        let format = call_audio_format(audio_format)?;
        let (mic_tx, mic_rx) = async_channel::bounded(MIC_CHANNEL_CAPACITY);
        let (speaker_tx, speaker_rx) = async_channel::bounded(SPEAKER_CHANNEL_CAPACITY);
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
        Ok(self.register_call(handle, mic_tx, speaker_rx))
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
            return Err(crate::errors::invalid_arg(
                "callId",
                "no live call for this call id (ended or never started)",
            ));
        };
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
    /// was told. The local side is down whatever comes back.
    #[wasm_bindgen(js_name = endCall)]
    pub async fn end_call(
        &self,
        call_id: &str,
    ) -> Result<Ts<crate::result_types::CallEndResult>, crate::errors::BridgeError> {
        // Before the gate: ending a call that is already gone is an answer,
        // not something worth parking behind a reconnect.
        if !self.call_records.borrow().contains_key(call_id) && !self.past_call_stats_has(call_id) {
            return Err(crate::errors::invalid_arg(
                "callId",
                "no live call for this call id (ended or never started)",
            ));
        }
        if self.past_call_stats_has(call_id) {
            return to_ts(crate::result_types::CallEndResult::AlreadyEnded);
        }
        // Held at the gate like accept: nothing has happened while parked,
        // so withdrawing and re-issuing is not a repeat.
        self.client.online().await?;
        let handle = self
            .call_records
            .borrow()
            .get(call_id)
            .map(|record| record.handle.clone());
        // The end watcher may have finished the call while the gate was
        // held; that already emitted `ended`, so report it, don't repeat it.
        let Some(handle) = handle else {
            return to_ts(crate::result_types::CallEndResult::AlreadyEnded);
        };
        let outcome = handle.terminate().await;
        self.finish_call(call_id);
        to_ts(call_termination_to_result(&outcome))
    }

    /// Mute or unmute the mic on a live call.
    #[wasm_bindgen(js_name = setCallMuted)]
    pub async fn set_call_muted(
        &self,
        call_id: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let handle = self
            .call_records
            .borrow()
            .get(call_id)
            .map(|record| record.handle.clone())
            .ok_or_else(|| {
                crate::errors::invalid_arg(
                    "callId",
                    "no live call for this call id (ended or never started)",
                )
            })?;
        // The mute itself rides the handle, which already knows the
        // answering device; the record lookup above only gates on liveness,
        // and the gate only waits out a reconnect in flight.
        self.client.online().await?;
        handle
            .set_muted(muted)
            .await
            .map_err(crate::errors::BridgeError::from)
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
        Err(crate::errors::invalid_arg(
            "callId",
            "no live call for this call id (ended or never started)",
        ))
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

impl WasmWhatsAppClient {
    /// Store a started call, pump it, and return its id.
    fn register_call(
        &self,
        handle: CallHandle,
        mic_tx: async_channel::Sender<Bytes>,
        speaker_rx: async_channel::Receiver<wacore::voip::EncodedAudioFrame>,
    ) -> String {
        let call_id = handle.call_id().to_owned();
        {
            let mut records = self.call_records.borrow_mut();
            if records.len() >= ACTIVE_CALL_CAPACITY && !records.contains_key(&call_id) {
                // Same backstop reasoning as the offer cache: the core owns
                // call policy, and unbounded host-side retention is worse
                // than dropping the oldest handle (whose call runs on).
                if let Some(evicted) = records.keys().next().cloned() {
                    log::warn!("Active call map full; released handle for {evicted}");
                    records.remove(&evicted);
                }
            }
            records.insert(
                call_id.clone(),
                CallRecord {
                    handle: handle.clone(),
                    mic_tx,
                    tasks: Vec::new(),
                },
            );
        }
        let tasks = vec![
            self.spawn_speaker_task(&call_id, speaker_rx),
            self.spawn_call_event_task(&call_id, handle.clone()),
            self.spawn_call_end_task(&call_id, handle),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        // The end watcher aborts its siblings when the call finishes; every
        // task here ends with the call either way, so nothing here outlives
        // `finish_call` except the watcher itself, which ends with it.
        if let Some(record) = self.call_records.borrow_mut().get_mut(&call_id) {
            record.tasks = tasks;
        }
        call_id
    }

    /// Forward decoded packets to the host audio callback. Only spawned when
    /// the host registered one; without it the facade sheds into the full
    /// sink channel, which is the same loss-tolerant answer with no task.
    fn spawn_speaker_task(
        &self,
        call_id: &str,
        speaker_rx: async_channel::Receiver<wacore::voip::EncodedAudioFrame>,
    ) -> Option<wacore::runtime::AbortHandle> {
        let callback = self.call_audio_callback.clone()?;
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            while let Ok(frame) = speaker_rx.recv().await {
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
                if callback.call1(&JsValue::NULL, &packet).is_err() {
                    log::error!("Call audio callback threw; stopping the pump for {call_id}");
                    break;
                }
            }
        })))
    }

    /// Forward the encoded-audio-relevant call events to the host event
    /// callback. Always spawned: the queue is bounded with eviction, so an
    /// undrained call would silently lose the events the host asked for.
    fn spawn_call_event_task(
        &self,
        call_id: &str,
        handle: CallHandle,
    ) -> Option<wacore::runtime::AbortHandle> {
        let events = handle.events();
        let callback = self.call_event_callback.clone();
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            while let Ok(event) = events.recv().await {
                let Some(callback) = callback.as_ref() else {
                    continue;
                };
                let Some(js_event) = translate_call_event(&call_id, &event) else {
                    continue;
                };
                if callback.call1(&JsValue::NULL, &js_event).is_err() {
                    log::error!("Call event callback threw; stopping forwarding for {call_id}");
                    break;
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
        handle: CallHandle,
    ) -> Option<wacore::runtime::AbortHandle> {
        let records = self.call_records.clone();
        let past = self.past_call_stats.clone();
        let offers = self.call_offers.clone();
        let callback = self.call_event_callback.clone();
        let call_id = call_id.to_owned();
        Some(self.runtime.spawn(Box::pin(async move {
            handle.wait_ended().await;
            finish_call(&records, &past, &offers, callback.as_ref(), &call_id);
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
    fn finish_call(&self, call_id: &str) {
        finish_call(
            &self.call_records,
            &self.past_call_stats,
            &self.call_offers,
            self.call_event_callback.as_ref(),
            call_id,
        );
    }
}

/// Release a call: abort its pumps, drop its offer, keep its final
/// counters, and tell the host it ended. Idempotent — only the remover
/// emits, so a racing `endCall` and end watcher cannot double-report.
fn finish_call(
    records: &RefCell<HashMap<String, CallRecord>>,
    past: &Mutex<VecDeque<(String, crate::result_types::CallMediaStatsResult)>>,
    offers: &Mutex<OfferCache>,
    callback: Option<&js_sys::Function>,
    call_id: &str,
) {
    let Some(record) = records.borrow_mut().remove(call_id) else {
        return;
    };
    for task in &record.tasks {
        task.abort();
    }
    offers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(call_id);
    let stats = call_media_stats_to_result(&record.handle.media_stats());
    {
        let mut past = past.lock().unwrap_or_else(|e| e.into_inner());
        past.push_back((call_id.to_owned(), stats));
        while past.len() > PAST_STATS_CAPACITY {
            past.pop_front();
        }
    }
    if let Some(callback) = callback {
        match call_event_object(call_id, "ended") {
            Ok(event) => {
                if callback.call1(&JsValue::NULL, &event.into()).is_err() {
                    log::error!("Call event callback threw on ended for {call_id}");
                }
            }
            Err(e) => log::error!("Ended event object rejected its fields: {e:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Boundary shapes
// ---------------------------------------------------------------------------

/// Parse the encoded-audio promise, defaulting to MLOW. Validation happens
/// before the gate: a misspelled format is the caller's own doing and should
/// not sit out a reconnect to be told so.
fn call_audio_format(value: Option<JsValue>) -> Result<AudioFormat, crate::errors::BridgeError> {
    let format = match value {
        Some(value) if !value.is_null() && !value.is_undefined() => {
            from_js_input::<crate::result_types::CallAudioFormat>("audioFormat", value)?
        }
        _ => crate::result_types::CallAudioFormat::Mlow,
    };
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

/// Map an accept/dial failure across. Only the negotiation pair names a
/// field: the encoded-audio promise disagrees with what the peer speaks, so
/// the host answers by retrying with the other `audioFormat`. Everything
/// else walks the chain — setup/media/response failures carry no caller
/// action, and a peer that hung up mid-setup is already reported through the
/// terminate event, so mapping them would invent precision the bridge does
/// not have.
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
    fn the_format_promise_defaults_to_mlow() {
        assert!(
            matches!(
                call_audio_format(None),
                Ok(format) if format == AudioFormat::MLOW_16KHZ_60MS
            ),
            "absent format must promise MLOW"
        );
        assert!(
            matches!(
                call_audio_format(Some(JsValue::UNDEFINED)),
                Ok(format) if format == AudioFormat::MLOW_16KHZ_60MS
            ),
            "undefined format must promise MLOW"
        );
        assert!(
            matches!(
                call_audio_format(Some(JsValue::from_str("opus"))),
                Ok(format) if format == AudioFormat::OPUS_MLOW_16KHZ_60MS
            ),
            "opus must promise the in-profile escape"
        );
        match call_audio_format(Some(JsValue::from_str("g729"))) {
            Err(crate::errors::BridgeError::InvalidArgument { field, .. }) => {
                assert_eq!(field, "audioFormat")
            }
            other => panic!("expected invalid-argument, got {other:?}"),
        }
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
