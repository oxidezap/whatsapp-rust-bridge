//! The session the registry stores per call when the plugin owns media.
//!
//! The session owns the public event stream from reservation, caches the
//! latest stats push for local reads, and forwards everything else across:
//! commands into the command mailbox the pump drains, media into the sink
//! halves the opening context carried, teardown into the locally stored
//! hook. A closure never crosses — `video_teardown` runs here.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_channel::{Receiver, Sender};
use async_trait::async_trait;
use bytes::Bytes;
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
    opened: AtomicBool,
    open_waker: Mutex<Option<oneshot::Sender<()>>>,
    closed: AtomicBool,
    alive: Arc<AtomicBool>,
    media: Mutex<Option<MediaSinks>>,
    video_teardown: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pump_handles: Mutex<Vec<whatsapp_rust::wacore::runtime::AbortHandle>>,
}

/// One command queued for the wire, pre-encoded so the retained-bytes
/// accounting is exact.
pub(crate) struct CommandEnvelope {
    pub(crate) bytes: Vec<u8>,
}

/// The sink halves the opening context carried, stored at attach for the
/// push path. The source halves move into the pump tasks.
struct MediaSinks {
    pcm_sink: Option<Arc<dyn vc::AudioSink>>,
    encoded_sink: Option<Arc<dyn vc::EncodedAudioSink>>,
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
            opened: AtomicBool::new(false),
            open_waker: Mutex::new(None),
            closed: AtomicBool::new(false),
            alive: Arc::new(AtomicBool::new(true)),
            media: Mutex::new(None),
            video_teardown: Mutex::new(None),
            pump_handles: Mutex::new(Vec::new()),
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
        conv::CtxBits {
            muted: ctx.muted.load(Ordering::SeqCst),
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

    /// Wires the opening context: stores the sinks, the teardown hook, and
    /// the negotiated format; spawns the pumps over the sources; clears the
    /// retained pre-attach epoch now that the queue drains onto the wire.
    pub fn attach(&self, format: &vc::MediaAudioFormat, ctx: vc::MediaOpenContext) {
        if let Ok(mut slot) = self.format.lock() {
            *slot = *format;
        }
        // Stored before any early return below: the hook runs on every
        // teardown, and the queue drains whatever the microphone does.
        if let Ok(mut teardown) = self.video_teardown.lock() {
            *teardown = ctx.video_teardown;
        }
        if let Ok(mut pending) = self.pending_epoch.lock() {
            *pending = None;
        }
        let (audio, pcm_sink, encoded_sink) = match ctx.audio {
            vc::MediaAudioPorts::Pcm { source, sink } => (
                media_pump::AudioInputs::Pcm(source.frames()),
                Some(sink),
                None,
            ),
            vc::MediaAudioPorts::Encoded { source, sink } => (
                media_pump::AudioInputs::Encoded(source.frames()),
                None,
                Some(sink),
            ),
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
        );
        if let Ok(mut pump_handles) = self.pump_handles.lock() {
            pump_handles.extend(handles);
        }
        if let Ok(mut media) = self.media.lock() {
            *media = Some(MediaSinks {
                pcm_sink,
                encoded_sink,
            });
        }
    }

    /// Resolves when the `OPEN` notification arrives. A prior arrival
    /// resolves immediately; a close first fails the wait.
    pub async fn wait_opened(&self) -> Result<(), vc::MediaSetupError> {
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
            vc::MediaSetupError::Backend("session ended before media came up".to_owned())
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
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.publish_event(vc::MediaEvent::Closed(conv::close_reason_from_abi(
            reason, detail,
        )));
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

    /// Adopts the codec a switch event announced, so the next encoded frame
    /// fills the format the engine now speaks.
    fn adopt_codec(&self, codec: vc::MediaAudioCodec) {
        if let Ok(mut format) = self.format.lock() {
            format.codec = codec;
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
            abi::Opcode::VideoOut => {
                // No neutral `VideoFrame` constructor exists for an external
                // backend to fill the control plane's sink with; inbound
                // video waits on one upstream. The drop is loud on purpose.
                log::warn!("voip plugin video frame dropped: no neutral VideoFrame constructor");
            }
            _ => log::warn!(
                "voip plugin pushed {}, which flows core to voip; dropped",
                frame.opcode.name()
            ),
        }
    }

    /// Handles one engine-raised event: codec switches refresh the format
    /// cache, `Closed` follows the remote-ended path, the rest publishes.
    fn on_event(&self, event: abi::AbiEvent) {
        if let abi::AbiEvent::Closed(reason) = &event {
            self.on_remote_ended(*reason, None);
            return;
        }
        let format = self.format.lock().ok().map(|f| *f);
        let Some(format) = format else { return };
        match conv::event_to_call_event(&event, &format) {
            Some(mapped @ vc::MediaEvent::AudioCodecSwitched { to, .. }) => {
                self.adopt_codec(to);
                if !self.publish_event(mapped) {
                    log::warn!("voip plugin event shed: session stream full");
                }
            }
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

    /// Writes one PCM frame into the speaker sink. Loss-tolerant: a full
    /// sink sheds, like the trait documents.
    fn on_pcm_out(&self, data: &[u8]) {
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
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        if samples.len() * 2 != data.len() {
            log::warn!("voip plugin PCM has an odd byte tail, truncated");
        }
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
        let codec = conv::unmap_audio_codec(frame.codec);
        let sender = match frame.sender.as_deref().map(str::parse).transpose() {
            Ok(sender) => sender,
            Err(_) => {
                log::warn!("voip plugin encoded audio with bad sender, dropped");
                return;
            }
        };
        let device = match frame.device.as_deref().map(str::parse).transpose() {
            Ok(device) => device,
            Err(_) => {
                log::warn!("voip plugin encoded audio with bad device, dropped");
                return;
            }
        };
        let _ = sink.frames().try_send(
            vc::MediaEncodedFrame::builder()
                .format(format)
                .codec(codec)
                .data(Bytes::from(frame.data.clone()))
                .payload_type(frame.payload_type)
                .sequence_number(frame.sequence_number as u16)
                .timestamp(frame.timestamp)
                .marker(frame.marker == 1)
                .maybe_sender(sender)
                .maybe_device(device)
                .build(),
        );
    }

    /// Enqueues one command for the pump. `false` is backpressure.
    fn enqueue(&self, command: abi::MediaCommand, lossless: bool) -> bool {
        let bytes = abi::CommandRequest {
            session: self.session_id(),
            command,
        }
        .encode();
        let len = bytes.len();
        let mut envelope = Some(CommandEnvelope { bytes });
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
        if let vc::MediaCommand::ApplyGroupEpoch { transaction_id, .. } = &command
            && let Ok(mut pending) = self.pending_epoch.lock()
        {
            *pending = Some(*transaction_id);
        }
        let Some(mapped) = conv::command_to_abi(&command) else {
            log::warn!("voip command this bridge predates, refused");
            return false;
        };
        self.enqueue(mapped, lossless)
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
        self.stats.lock().map(|s| *s).unwrap_or_default()
    }

    fn subscribe(&self) -> async_channel::Receiver<vc::MediaEvent> {
        self.events_rx.clone()
    }

    fn close(&self, reason: vc::MediaCloseReason) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.alive.store(false, Ordering::SeqCst);
        // An open parked on the `OPEN` notification ends here instead of
        // hanging to the ceiling: dropping the sender cancels the wait.
        if let Ok(mut waker) = self.open_waker.lock() {
            waker.take();
        }
        // The stream ends here so a lingering handle's `recv` ends instead
        // of parking — the same close-on-teardown the old entry queue had.
        self.events_tx.close();
        if let Ok(mut teardown) = self.video_teardown.lock()
            && let Some(hook) = teardown.take()
        {
            hook();
        }
        if let Ok(mut handles) = self.pump_handles.lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
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
