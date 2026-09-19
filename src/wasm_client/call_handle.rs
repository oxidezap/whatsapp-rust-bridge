//! `WasmCallHandle` — an exported handle for one active call.
//!
//! Returned by `acceptCall(offerHandle, ...)` and `dialCall(...)`. Every
//! method maps directly to the underlying `CallHandle` from whatsapp-rust or
//! to the bridge's own video-pump infrastructure (via `CallMedia`).
//!
//! The handle is independent of the client once constructed: `terminate`,
//! `hangupLocal`, `waitEnded`, `setMuted`, and `mediaStats` all work until
//! the call ends without holding a client borrow. Video methods need
//! `CallMedia` which holds `Rc<RefCell<...>>` — valid on WASM's single thread.

use wasm_bindgen::prelude::*;

#[cfg(feature = "client-calls-audio")]
use super::calls_audio::CallMedia;
#[cfg(feature = "client-calls-audio")]
use whatsapp_rust::voip::CallHandle;

/// An active call handle returned by `acceptCall(offerHandle, ...)` or `dialCall(...)`.
///
/// Safe to hold after `free()`ing the client wrapper: the underlying engine
/// task and the call's event channel both outlive the client. Video methods
/// need the bridge's pump infrastructure and will fail gracefully if the
/// client has been freed.
#[cfg(feature = "client-calls-audio")]
#[wasm_bindgen]
pub struct WasmCallHandle {
    /// The whatsapp-rust call handle — owns the engine task reference.
    handle: CallHandle,
    /// The bridge's media infrastructure — for video plane management and
    /// mute-flag mirroring. Cloned from the client at construction time.
    media: CallMedia,
    /// The call-id, cached so JS getters don't need to cross into Rust.
    call_id: String,
    /// The registry generation for ABA protection. Monotonically increasing
    /// per call-id; used in generation-guarded operations.
    generation: u64,
}

#[cfg(feature = "client-calls-audio")]
// SAFETY: WASM is single-threaded; the Rc<RefCell<...>> inside CallMedia is
// never shared across threads. The wasm_send_sync! macro declares this.
crate::wasm_send_sync!(WasmCallHandle);

#[cfg(feature = "client-calls-audio")]
impl WasmCallHandle {
    /// Construct from a live call handle and the client's media infrastructure.
    pub(super) fn new(handle: CallHandle, generation: u64, media: CallMedia) -> Self {
        let call_id = handle.call_id().to_owned();
        Self {
            handle,
            media,
            call_id,
            generation,
        }
    }
}

#[cfg(feature = "client-calls-audio")]
#[wasm_bindgen]
impl WasmCallHandle {
    /// The call-id this handle controls.
    #[wasm_bindgen(getter, js_name = callId)]
    pub fn call_id_js(&self) -> String {
        self.call_id.clone()
    }

    /// The registry generation for this call. Opaque to the host — useful for
    /// distinguishing a replacement call that has the same `callId` (glare/retry).
    /// Values beyond 2^53 would lose precision as a JS double, but that requires
    /// 9 × 10^15 calls per session.
    #[wasm_bindgen(getter)]
    pub fn generation(&self) -> f64 {
        self.generation as f64
    }

    /// Terminate the call: send `<terminate>` to every peer address and tear
    /// down the engine. Returns a typed outcome describing what was reached.
    #[wasm_bindgen(js_name = terminate, unchecked_return_type = "Promise<CallEndResult>")]
    pub fn terminate_js(&self) -> js_sys::Promise {
        let handle = self.handle.clone();
        let call_id = self.call_id.clone();
        let generation = self.generation;
        let media = self.media.clone();
        super::promise_value(async move {
            let outcome = handle.terminate().await;
            media.finish_call_by_generation(&call_id, generation);
            let result = super::calls_audio::call_termination_to_result(&outcome);
            super::to_ts(result)
        })
    }

    /// Hang up locally without sending a `<terminate>` stanza. The engine
    /// tears down immediately.
    #[wasm_bindgen(js_name = hangupLocal, unchecked_return_type = "Promise<void>")]
    pub fn hangup_local_js(&self) -> js_sys::Promise {
        let handle = self.handle.clone();
        super::promise_void(async move {
            handle.hangup_local().await;
            Ok(())
        })
    }

    /// Resolves once the call is fully ended.
    #[wasm_bindgen(js_name = waitEnded, unchecked_return_type = "Promise<void>")]
    pub fn wait_ended_js(&self) -> js_sys::Promise {
        let handle = self.handle.clone();
        super::promise_void(async move {
            handle.wait_ended().await;
            Ok(())
        })
    }

    /// Set the microphone mute state. Sends a `<mute v2>` stanza so the peer
    /// updates its own UI and mirrors the flag locally for immediate pump silence.
    #[wasm_bindgen(js_name = setMuted, unchecked_return_type = "Promise<void>")]
    pub fn set_muted_js(&self, muted: bool) -> js_sys::Promise {
        let handle = self.handle.clone();
        let media = self.media.clone();
        let call_id = self.call_id.clone();
        super::promise_void(async move {
            // Mirror locally first so outbound audio sheds before the stanza
            // lands. A failed stanza is still a local decision — the pump stops.
            media.set_mic_muted_flag(&call_id, muted);
            handle
                .set_muted(muted)
                .await
                .map_err(|e| super::calls_audio::call_error_to_bridge(e, "setMuted"))?;
            Ok(())
        })
    }

    /// Current media counters for this call. All-zero until the media plane
    /// attaches; readable after the call ends.
    #[wasm_bindgen(js_name = mediaStats, unchecked_return_type = "CallMediaStatsResult")]
    pub fn media_stats_js(&self) -> Result<JsValue, JsValue> {
        let raw = self.handle.media_stats();
        let result = super::calls_audio::stats_to_result(&raw);
        crate::proto::to_js_value(&result)
    }

    /// Start outbound video. The video callback (`onCallVideo`) registered at
    /// client construction time is used for inbound frames.
    #[wasm_bindgen(js_name = startVideo, unchecked_return_type = "Promise<void>")]
    pub fn start_video_js(&self) -> js_sys::Promise {
        let handle = self.handle.clone();
        let call_id = self.call_id.clone();
        let generation = self.generation;
        let media = self.media.clone();
        super::promise_void(async move {
            media
                .attach_video_plane_external(&call_id, generation, "startVideo", move |rx, tx| {
                    let handle = handle.clone();
                    async move { handle.start_video(rx, tx).await }
                })
                .await
        })
    }

    /// Accept an incoming video upgrade request. The pending upgrade token must
    /// have been stored from the `video-upgrade-requested` call event.
    #[wasm_bindgen(js_name = acceptVideo, unchecked_return_type = "Promise<void>")]
    pub fn accept_video_js(&self) -> js_sys::Promise {
        let call_id = self.call_id.clone();
        let generation = self.generation;
        let media = self.media.clone();
        super::promise_void(async move {
            media
                .accept_call_video_by_generation(&call_id, generation)
                .await
        })
    }

    /// Stop outbound video.
    #[wasm_bindgen(js_name = stopVideo, unchecked_return_type = "Promise<void>")]
    pub fn stop_video_js(&self) -> js_sys::Promise {
        let call_id = self.call_id.clone();
        let media = self.media.clone();
        super::promise_void(async move { media.stop_call_video(call_id).await })
    }

    /// Reject an incoming video upgrade request. Clears the pending upgrade
    /// token so future `acceptVideo` calls will report no pending request.
    #[wasm_bindgen(js_name = rejectVideo, unchecked_return_type = "Promise<void>")]
    pub fn reject_video_js(&self) -> js_sys::Promise {
        let media = self.media.clone();
        let call_id = self.call_id.clone();
        super::promise_void(async move {
            media.clear_pending_video_upgrade(&call_id);
            Ok(())
        })
    }
}
