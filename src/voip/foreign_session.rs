//! The session the registry stores per call when the plugin owns media.
//!
//! The session owns the public event stream from reservation, caches the
//! latest stats push for local reads, and forwards everything else across:
//! commands into the command mailbox the pump drains, media into the sink
//! halves the opening context carried, teardown into the locally stored
//! hook. A closure never crosses — `video_teardown` runs here.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use async_channel::{Receiver, Sender};
use async_trait::async_trait;
use futures::FutureExt;
use futures::channel::oneshot;
use voip_abi as abi;
use whatsapp_rust::voip_control as vc;

use super::{VoipBackendCallbacks, abi as conv, media_pump};

/// Event stream depth, mirroring the resident session's queue. The stream
/// carries signaling and media events in one order; bounding it is the
/// backpressure the `publish` contract names.
const EVENT_QUEUE_CAPACITY: usize = 64;
/// Command mailbox depth. `submit` answers full with `false`;
/// `submit_lossless` sheds the oldest entry first.
const COMMAND_QUEUE_CAPACITY: usize = 64;

/// One media session behind the foreign backend.
pub struct ForeignVoipSession {
    handle: u32,
    key: vc::MediaSessionKey,
    direction: vc::CallDirection,
    callbacks: VoipBackendCallbacks,
    runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
    agreed: u32,
    events_tx: Sender<vc::MediaEvent>,
    events_rx: Receiver<vc::MediaEvent>,
    stats: Mutex<vc::MediaStats>,
    format: Mutex<vc::MediaAudioFormat>,
    commands_tx: Sender<CommandEnvelope>,
    commands_rx: Receiver<CommandEnvelope>,
    queued_command_bytes: Arc<AtomicUsize>,
    pending_epoch: Mutex<Option<u32>>,
    engine_reserved: AtomicBool,
    opening: AtomicBool,
    pending_cancel: Mutex<Option<futures::future::Shared<oneshot::Receiver<()>>>>,
    opened: AtomicBool,
    initial_muted: AtomicBool,
    video_sink_dropped: AtomicU32,
    open_waker: Mutex<Option<oneshot::Sender<()>>>,
    closed: AtomicBool,
    remote_ended: AtomicBool,
    open_failure: Mutex<Option<String>>,
    alive: Arc<AtomicBool>,
    media: Mutex<Option<MediaSinks>>,
    video_teardown: Mutex<Option<whatsapp_rust::voip_control::VideoTeardownHook>>,
    pump_handles: Mutex<Vec<whatsapp_rust::wacore::runtime::AbortHandle>>,
    on_closed: Mutex<Option<vc::VideoTeardownHook>>,
}

/// One command queued for the wire, pre-encoded so the retained-bytes
/// accounting is exact.
pub(crate) struct CommandEnvelope {
    pub(crate) bytes: abi::SecretBytes,
}

/// The sink halves the opening context carried, stored before BEGIN_OPEN for the
/// push path. The source halves move into the pump tasks.
struct MediaSinks {
    pcm_sink: Option<Arc<dyn vc::AudioSink>>,
    encoded_sink: Option<Arc<dyn vc::EncodedAudioSink>>,
    video_sink: async_channel::Sender<vc::VideoFrame>,
}

impl ForeignVoipSession {
    /// Creates the reserved session. The event stream exists from here, so
    /// signaling published before media attaches reaches the same queue.
    pub fn new(
        handle: u32,
        key: vc::MediaSessionKey,
        direction: vc::CallDirection,
        callbacks: VoipBackendCallbacks,
        runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
        agreed: u32,
    ) -> Self {
        let (events_tx, events_rx) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
        let (commands_tx, commands_rx) = async_channel::bounded(COMMAND_QUEUE_CAPACITY);
        ForeignVoipSession {
            handle,
            key,
            direction,
            callbacks,
            runtime,
            agreed,
            events_tx,
            events_rx,
            stats: Mutex::new(vc::MediaStats::default()),
            format: Mutex::new(vc::MediaAudioFormat::MLOW_16KHZ_60MS),
            commands_tx,
            commands_rx,
            queued_command_bytes: Arc::new(AtomicUsize::new(0)),
            pending_epoch: Mutex::new(None),
            engine_reserved: AtomicBool::new(false),
            opening: AtomicBool::new(false),
            pending_cancel: Mutex::new(None),
            opened: AtomicBool::new(false),
            initial_muted: AtomicBool::new(false),
            video_sink_dropped: AtomicU32::new(0),
            open_waker: Mutex::new(None),
            closed: AtomicBool::new(false),
            remote_ended: AtomicBool::new(false),
            open_failure: Mutex::new(None),
            alive: Arc::new(AtomicBool::new(true)),
            media: Mutex::new(None),
            video_teardown: Mutex::new(None),
            pump_handles: Mutex::new(Vec::new()),
            on_closed: Mutex::new(None),
        }
    }

    pub(super) fn engine_reserved(&self) -> bool {
        self.engine_reserved.load(Ordering::SeqCst)
    }

    pub(super) fn mark_engine_reserved(&self) {
        self.engine_reserved.store(true, Ordering::SeqCst);
    }

    pub(super) async fn begin_open_attempt(&self) -> Result<(), vc::MediaSetupError> {
        let pending = self.pending_cancel.lock().unwrap().clone();
        if let Some(pending) = pending {
            let _ = pending.await;
        }
        if self.is_closed()
            || self.opened.load(Ordering::SeqCst)
            || self.opening.swap(true, Ordering::SeqCst)
        {
            return Err(vc::MediaSetupError::Backend(
                "session is closed or already opening/open".to_owned(),
            ));
        }
        *self.open_failure.lock().unwrap() = None;
        self.open_waker.lock().unwrap().take();
        Ok(())
    }

    pub(super) fn finish_open_attempt(&self) {
        self.opening.store(false, Ordering::SeqCst);
    }

    pub(super) fn track_cancel(&self) -> oneshot::Sender<()> {
        let (sender, receiver) = oneshot::channel();
        *self.pending_cancel.lock().unwrap() = Some(receiver.shared());
        sender
    }

    pub(super) fn set_on_closed(&self, hook: vc::VideoTeardownHook) {
        *self.on_closed.lock().unwrap() = Some(hook);
    }

    pub(super) fn prepare_teardown(&self, hook: Option<vc::VideoTeardownHook>) {
        if let Some(hook) = hook {
            *self.video_teardown.lock().unwrap() = Some(hook);
        }
    }

    /// The wire identity of this session.
    pub fn session_id(&self) -> abi::SessionId {
        abi::SessionId {
            handle: self.handle,
            generation: self.key.generation,
        }
    }

    /// True while `generation` is the generation this session was reserved
    /// for. Anything else is stale and never touches it.
    pub fn is_current(&self, generation: u64) -> bool {
        self.key.generation == generation
    }

    /// True once the session ended.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The direction this session was reserved for. The `RESERVE` frame
    /// carries the reserved direction rather than the spec's, so a key that
    /// changed hands between reserve and open cannot smuggle a new one in.
    pub fn direction(&self) -> vc::CallDirection {
        self.direction
    }

    /// Builds the context bits for the open params from the opening context.
    /// An enabled video plane sends and receives until a command says
    /// otherwise; the spec only says whether video is up.
    pub fn ctx_bits<'c>(
        &self,
        ctx: &'c vc::MediaOpenContext,
        enable_video: bool,
    ) -> conv::CtxBits<'c> {
        let muted = ctx.muted.load(Ordering::SeqCst);
        self.initial_muted.store(muted, Ordering::SeqCst);
        conv::CtxBits {
            muted,
            video_caps: if enable_video {
                voip_abi::VideoCaps::SEND | voip_abi::VideoCaps::RECV
            } else {
                0
            },
            initial_codec: ctx.initial_codec,
            peer_orientations: ctx.peer_video_orientations.clone(),
            group_epoch: ctx.group_epoch.as_ref().map(|(tx, epoch)| (*tx, epoch)),
        }
    }

    pub(super) fn prepare_sinks(&self, format: &vc::MediaAudioFormat, ctx: &vc::MediaOpenContext) {
        *self.format.lock().unwrap() = *format;
        let (pcm_sink, encoded_sink) = match &ctx.audio {
            vc::MediaAudioPorts::Pcm { sink, .. } => (Some(sink.clone()), None),
            vc::MediaAudioPorts::Encoded { sink, .. } => (None, Some(sink.clone())),
            _ => (None, None),
        };
        *self.media.lock().unwrap() = Some(MediaSinks {
            pcm_sink,
            encoded_sink,
            video_sink: ctx.video_channels.video_out.clone(),
        });
    }

    /// Starts source pumps after OPEN and clears the retained pre-attach epoch.
    /// Sinks and the teardown hook are already installed before BEGIN_OPEN.
    pub fn attach(&self, format: &vc::MediaAudioFormat, ctx: vc::MediaOpenContext) {
        if let Ok(mut slot) = self.format.lock() {
            *slot = *format;
        }
        // Stored before any early return below: the hook runs on every
        // teardown, and the queue drains whatever the microphone does.
        self.prepare_teardown(ctx.video_teardown);
        if let Ok(mut pending) = self.pending_epoch.lock() {
            *pending = None;
        }
        let audio = match ctx.audio {
            vc::MediaAudioPorts::Pcm { source, .. } => {
                media_pump::AudioInputs::Pcm(source.frames())
            }
            vc::MediaAudioPorts::Encoded { source, .. } => {
                media_pump::AudioInputs::Encoded(source.frames())
            }
            _ => {
                // A future audio port this bridge predates: the session
                // opens without a microphone rather than guessing its shape.
                log::warn!("voip audio port this bridge predates, no microphone pump");
                return;
            }
        };
        let inputs = media_pump::PumpInputs {
            audio,
            video_in: ctx.video_channels.video_in,
            timed_in: ctx.video_channels.timed_video_in,
            control: ctx.video_channels.control,
            muted: ctx.muted.clone(),
            initial_muted: self.initial_muted.load(Ordering::SeqCst),
        };
        let pipe = media_pump::CommandPipe {
            tx: self.commands_tx.clone(),
            rx: self.commands_rx.clone(),
            queued_bytes: self.queued_command_bytes.clone(),
        };
        let handles = media_pump::spawn_all(
            self.handle,
            self.session_id(),
            inputs,
            pipe,
            self.callbacks.clone(),
            self.runtime.clone(),
            self.agreed,
            self.alive.clone(),
            self.events_tx.clone(),
        );
        if let Ok(mut pump_handles) = self.pump_handles.lock() {
            pump_handles.extend(handles);
        }
    }

    /// Resolves when the `OPEN` notification arrives. A prior arrival
    /// resolves immediately; a close first fails the wait.
    pub async fn wait_opened(&self) -> Result<(), vc::MediaSetupError> {
        if let Some(failure) = self.open_failure.lock().ok().and_then(|s| s.clone()) {
            return Err(open_failure(&failure));
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(vc::MediaSetupError::Backend(
                "session ended before media came up".to_owned(),
            ));
        }
        if self.opened.load(Ordering::SeqCst) {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut waker = self.open_waker.lock().map_err(|_| {
                vc::MediaSetupError::Backend("session ended before media came up".to_owned())
            })?;
            if self.opened.load(Ordering::SeqCst) {
                return Ok(());
            }
            *waker = Some(tx);
        }
        rx.await.map_err(|_| {
            open_failure(
                &self
                    .open_failure
                    .lock()
                    .ok()
                    .and_then(|s| s.clone())
                    .unwrap_or_else(|| "session ended before media came up".to_owned()),
            )
        })
    }

    /// Completes a pending open wait. Idempotent: a duplicate `OPEN` is a
    /// plugin bug, not a second opening.
    fn on_opened(&self) {
        self.opened.store(true, Ordering::SeqCst);
        if let Ok(mut waker) = self.open_waker.lock()
            && let Some(tx) = waker.take()
        {
            let _ = tx.send(());
        }
    }

    /// Handles the media ending engine-side: raises `Closed` on the stream
    /// so a lingering handle's `recv` ends instead of parking. The channel
    /// itself closes on the control plane's `close`, which tears down.
    fn on_remote_ended(&self, reason: abi::CloseReason, detail: Option<&str>) {
        if self.remote_ended.swap(true, Ordering::SeqCst) {
            return;
        }
        // Terminal state must survive a full diagnostics queue.
        let _ = self
            .events_tx
            .force_send(vc::MediaEvent::Closed(conv::close_reason_from_abi(
                reason, detail,
            )));
        <Self as vc::VoipMediaSession>::close(self, conv::close_reason_from_abi(reason, detail));
    }

    /// Publishes into the session's stream. `false` is backpressure, per the
    /// trait contract — the caller decides whether to retry.
    fn publish_event(&self, event: vc::MediaEvent) -> bool {
        self.events_tx.try_send(event).is_ok()
    }

    /// Caches a pushed snapshot for local reads.
    fn cache_stats(&self, stats: &abi::StatsData) {
        if let Ok(mut slot) = self.stats.lock() {
            *slot = conv::stats_to_core(stats);
        }
    }

    /// Routes one validated push frame into the session.
    pub fn deliver_push(&self, frame: abi::Frame) {
        match frame.opcode {
            abi::Opcode::Open => match abi::OpenNotification::decode(&frame.payload) {
                Ok(notice) if notice.session == self.session_id() => self.on_opened(),
                _ => log::warn!("voip plugin OPEN for another session, dropped"),
            },
            abi::Opcode::Event => match abi::EventNotification::decode(&frame.payload) {
                Ok(notice) if notice.session == self.session_id() => self.on_event(notice.event),
                Ok(_) => log::warn!("stale voip plugin event, dropped"),
                Err(e) => log::warn!("voip plugin event misformed, dropped: {e}"),
            },
            abi::Opcode::Stats if !frame.is_response() => {
                match abi::StatsPush::decode(&frame.payload) {
                    Ok(push) if push.session == self.session_id() => self.cache_stats(&push.stats),
                    Ok(_) => log::warn!("stale voip plugin stats, dropped"),
                    Err(e) => log::warn!("voip plugin stats misformed, dropped: {e}"),
                }
            }
            abi::Opcode::MediaEnded => match abi::CloseRequest::decode(&frame.payload) {
                Ok(req) if req.session == self.session_id() => {
                    self.on_remote_ended(req.reason, req.detail.as_deref());
                }
                Ok(_) => log::warn!("stale voip plugin media-ended, dropped"),
                Err(e) => log::warn!("voip plugin media-ended misformed, dropped: {e}"),
            },
            abi::Opcode::PcmOut => match abi::MediaFrame::decode(&frame.payload) {
                Ok(frame) if frame.session == self.session_id() => self.on_pcm_out(&frame.data),
                Ok(_) => log::warn!("stale voip plugin PCM, dropped"),
                Err(e) => log::warn!("voip plugin PCM misformed, dropped: {e}"),
            },
            abi::Opcode::EncodedAudioOut => match abi::EncodedAudioOut::decode(&frame.payload) {
                Ok(out) if out.session == self.session_id() => self.on_encoded_out(&out.frame),
                Ok(_) => log::warn!("stale voip plugin encoded audio, dropped"),
                Err(e) => log::warn!("voip plugin encoded audio misformed, dropped: {e}"),
            },
            abi::Opcode::VideoOut => match abi::VideoOut::decode(&frame.payload) {
                Ok(out) if out.session == self.session_id() => self.on_video_out(out.frame),
                _ => log::warn!("voip plugin video with malformed or stale identity, dropped"),
            },
            _ => log::warn!(
                "voip plugin pushed {}, which flows core to voip; dropped",
                frame.opcode.name()
            ),
        }
    }

    /// Handles one engine-raised event. Closed follows the remote-ended
    /// path; other diagnostics publish without changing per-frame formats.
    fn on_event(&self, event: abi::AbiEvent) {
        if let abi::AbiEvent::MediaSetupFailed(detail) = &event {
            *self.open_failure.lock().unwrap() = Some(detail.clone());
            self.open_waker.lock().unwrap().take();
        }
        if let abi::AbiEvent::Closed(reason) = &event {
            self.on_remote_ended(*reason, None);
            return;
        }
        let format = self.format.lock().ok().map(|f| *f);
        let Some(format) = format else { return };
        match conv::event_to_call_event(&event, &format) {
            Some(mapped) => {
                if !self.publish_event(mapped) {
                    log::warn!("voip plugin event shed: session stream full");
                }
            }
            None => log::warn!(
                "voip plugin event unmappable ({}), dropped",
                event.kind_name()
            ),
        }
    }

    fn on_video_out(&self, frame: abi::VideoFrameDto) {
        if self.is_closed() || self.agreed & abi::Capabilities::VIDEO == 0 {
            return;
        }
        let sender = match frame.sender.as_deref().map(str::parse).transpose() {
            Ok(jid) => jid,
            Err(_) => return,
        };
        let device = match frame.device.as_deref().map(str::parse).transpose() {
            Ok(jid) => jid,
            Err(_) => return,
        };
        let sink = self
            .media
            .lock()
            .ok()
            .and_then(|media| media.as_ref().map(|media| media.video_sink.clone()));
        let Some(sink) = sink else { return };
        let frame = vc::VideoFrame::builder()
            .data(frame.data)
            .keyframe(frame.keyframe == 1)
            .orientation(frame.orientation)
            .maybe_sender(sender)
            .maybe_device(device)
            .maybe_pid(frame.pid)
            .timestamp(frame.timestamp)
            .generation(self.key.generation)
            .build();
        if matches!(
            sink.try_send(frame),
            Err(async_channel::TrySendError::Full(_))
        ) {
            let _ =
                self.video_sink_dropped
                    .try_update(Ordering::Relaxed, Ordering::Relaxed, |drops| {
                        Some(drops.saturating_add(1))
                    });
        }
    }

    /// Writes one PCM frame into the speaker sink. Loss-tolerant: a full
    /// sink sheds, like the trait documents.
    fn on_pcm_out(&self, data: &[u8]) {
        if !data.len().is_multiple_of(2) {
            log::warn!("voip plugin PCM has an odd byte tail, dropped");
            return;
        }
        let media = self
            .media
            .lock()
            .ok()
            .and_then(|m| m.as_ref()?.pcm_sink.clone());
        let Some(sink) = media else {
            log::warn!("voip plugin PCM with no speaker attached, dropped");
            return;
        };
        let samples: Vec<i16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        if sink.playout().try_send(samples).is_err() {
            log::debug!("speaker sink full, PCM frame shed");
        }
    }

    /// Writes one encoded packet into the encoded sink.
    fn on_encoded_out(&self, frame: &abi::EncodedFrameDto) {
        let media = self
            .media
            .lock()
            .ok()
            .and_then(|m| m.as_ref()?.encoded_sink.clone());
        let Some(sink) = media else {
            log::warn!("voip plugin encoded audio with no sink attached, dropped");
            return;
        };
        let format = self.format.lock().ok().map(|f| *f);
        let Some(format) = format else { return };
        if let Some(frame) = conv::encoded_frame(frame, &format) {
            let _ = sink.frames().try_send(frame);
        }
    }

    /// Enqueues one command for the pump. `false` is backpressure.
    fn enqueue(&self, command: abi::MediaCommand, lossless: bool) -> bool {
        if self.agreed & abi::Capabilities::VIDEO == 0 && command.kind_name().starts_with("VIDEO_")
        {
            return false;
        }
        let bytes = abi::CommandRequest {
            session: self.session_id(),
            command,
        }
        .encode();
        let len = bytes.len();
        let mut envelope = Some(CommandEnvelope {
            bytes: abi::SecretBytes::new(bytes),
        });
        // A full mailbox returns the envelope; shedding the oldest entry
        // (the resident mailbox policy) frees the slot it takes.
        for _ in 0..2 {
            let item = envelope.take().expect("envelope returned on full");
            match self.commands_tx.try_send(item) {
                Ok(()) => {
                    self.queued_command_bytes.fetch_add(len, Ordering::Relaxed);
                    return true;
                }
                Err(e) if lossless => {
                    envelope = Some(e.into_inner());
                    if let Ok(old) = self.commands_rx.try_recv() {
                        self.queued_command_bytes
                            .fetch_sub(old.bytes.len(), Ordering::Relaxed);
                    }
                }
                Err(_) => return false,
            }
        }
        false
    }

    /// Submits through the shared path, retaining group epochs for the
    /// pre-attach pairing the control plane reads back.
    fn submit_inner(&self, command: vc::MediaCommand, lossless: bool) -> bool {
        let epoch = match &command {
            vc::MediaCommand::ApplyGroupEpoch { transaction_id, .. } => Some(*transaction_id),
            _ => None,
        };
        let Some(mapped) = conv::command_to_abi(&command) else {
            return false;
        };
        if !self.enqueue(mapped, lossless) {
            return false;
        }
        if let Some(epoch) = epoch {
            *self.pending_epoch.lock().unwrap() = Some(epoch);
        }
        true
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl vc::VoipMediaSession for ForeignVoipSession {
    fn submit(&self, command: vc::MediaCommand) -> bool {
        self.submit_inner(command, false)
    }

    fn submit_lossless(&self, command: vc::MediaCommand) -> bool {
        self.submit_inner(command, true)
    }

    fn group_update_fits(&self, _update: &vc::GroupCallUpdate, _is_call_link: bool) -> bool {
        // The preflight cannot cross synchronously: `submit` is sync and
        // the engine answers over the wire. The delivery answer stays
        // authoritative — a refused delivery leaves the committed signaling
        // state unconsumed — so the preflight admits and the mailbox decides.
        true
    }

    fn pending_group_epoch(&self) -> Option<u32> {
        *self.pending_epoch.lock().ok()?
    }

    fn retained_bytes(&self) -> usize {
        self.queued_command_bytes.load(Ordering::Relaxed)
    }

    fn publish(&self, event: vc::MediaEvent) -> bool {
        self.publish_event(event)
    }

    fn stats(&self) -> vc::MediaStats {
        let mut stats = self.stats.lock().map(|s| *s).unwrap_or_default();
        stats.video_sink_dropped = stats
            .video_sink_dropped
            .saturating_add(self.video_sink_dropped.load(Ordering::Relaxed));
        stats
    }

    fn subscribe(&self) -> async_channel::Receiver<vc::MediaEvent> {
        self.events_rx.clone()
    }

    fn close(&self, reason: vc::MediaCloseReason) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let on_closed = self.on_closed.lock().unwrap().take();
        if let Some(hook) = on_closed {
            hook();
        }
        self.alive.store(false, Ordering::SeqCst);
        self.media.lock().unwrap().take();
        // An open parked on the `OPEN` notification ends here instead of
        // hanging to the ceiling: dropping the sender cancels the wait.
        if let Ok(mut waker) = self.open_waker.lock() {
            waker.take();
        }
        // The stream ends here so a lingering handle's `recv` ends instead
        // of parking — the same close-on-teardown the old entry queue had.
        self.events_tx.close();
        let teardown = self
            .video_teardown
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(hook) = teardown {
            hook();
        }
        if let Ok(mut handles) = self.pump_handles.lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
        self.commands_tx.close();
        while self.commands_rx.try_recv().is_ok() {}
        self.queued_command_bytes.store(0, Ordering::Relaxed);
        let (abi_reason, detail) = conv::close_reason_to_abi(&reason);
        let close = abi::CloseRequest {
            session: self.session_id(),
            reason: abi_reason,
            detail,
        };
        let bytes = abi::Frame::request(abi::Opcode::Close, close.encode()).encode();
        let callbacks = self.callbacks.clone();
        // Detached: dropping the handle aborts the task, which would kill
        // the send before its first poll. Fire-and-forget is the point —
        // the session is gone either way, and awaiting here would park
        // teardown on a plugin that may never answer.
        self.runtime
            .spawn(Box::pin(async move {
                let _ = callbacks.send(&bytes).await;
            }))
            .detach();
    }
}

fn open_failure(detail: &str) -> vc::MediaSetupError {
    if let Some(detail) = detail.strip_prefix("TRANSPORT: ") {
        vc::MediaSetupError::Connect(detail.to_owned())
    } else {
        vc::MediaSetupError::Backend(detail.to_owned())
    }
}

#[cfg(test)]
mod teardown_tests {
    use super::*;
    use vc::VoipMediaSession;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn closing_a_retained_session_releases_queued_secret_commands() {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(
            &object,
            &"sendFrame".into(),
            &js_sys::Function::new_with_args("frame", "return Promise.resolve(frame);"),
        )
        .unwrap();
        js_sys::Reflect::set(
            &object,
            &"setPushHandler".into(),
            &js_sys::Function::new_no_args(""),
        )
        .unwrap();
        let callbacks = VoipBackendCallbacks::from_js(&JsValue::from(object)).unwrap();
        let session = ForeignVoipSession::new(
            1,
            vc::MediaSessionKey::builder()
                .call_id("retained-secret-queue".to_owned())
                .generation(1)
                .build(),
            vc::CallDirection::Outgoing,
            callbacks,
            Arc::new(crate::runtime::WasmRuntime),
            u32::MAX,
        );
        for transaction_id in 1..=3 {
            assert!(session.enqueue(
                abi::MediaCommand::GroupApplyEpoch {
                    transaction_id,
                    epoch: abi::SecretBytes::new(vec![0x33; 4096]),
                },
                false
            ));
        }
        assert_eq!(session.commands_rx.len(), 3);
        assert!(session.retained_bytes() >= 3 * 4096);
        session.close(vc::MediaCloseReason::Local);
        assert!(session.commands_rx.is_closed());
        assert!(session.commands_rx.is_empty());
        assert_eq!(session.retained_bytes(), 0);
        assert!(!session.enqueue(abi::MediaCommand::AudioMute(1), false));
        session.close(vc::MediaCloseReason::Local);
    }
}
