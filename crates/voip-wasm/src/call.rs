//! One live call: the `CallChannels` mailboxes, the drive task, and the
//! fans between the OZVP frames and the engine.
//!
//! The core owns the symmetric half in `src/voip/`: its pumps turn JS media
//! sources into `PCM_IN`/`ENCODED_AUDIO_IN`/`VIDEO_IN` frames and its session
//! turns `PCM_OUT`/`ENCODED_AUDIO_OUT`/events back into sinks. This side owns
//! the engine those frames feed: each live session holds the channel halves
//! `run_call` was given, and this module routes inbound frames into them and
//! the drive loop's outputs back out as pushes.
//!
//! Media outputs shed on overflow; lifecycle events force-send like the
//! driver's own publisher. `Close` aborts the drive task, which drops the
//! transport `Arc` and closes the relay channel with it.

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use bytes::Bytes;
use voip_abi::{
    AbiEvent, CloseReason, CodecSource, EncodedFrameDto, MediaCommand, MediaFrame, OpenParams,
    SessionId, SilenceReason, StatsData, Urgency, VideoFrameDto, VideoInputDto,
};
use wacore::runtime::Runtime;
use wacore::voip::driver::{CallChannels, video_control_channel};
use wacore::voip::engine::{CallEngine, GroupEngineConfig};
use wacore::voip::transport::RelayEndpointParams;
use wacore::voip_control::{
    MediaAudioCodec, MediaAudioFormat, MediaCloseReason, MediaCodecDecisionSource,
    MediaEncodedFrame, MediaEvent, MediaKeyframeUrgency, MediaSetupError, MediaSilenceReason,
    media_stats,
};
use wacore_binary::Jid;

use crate::relay;
use crate::runtime::EngineRuntime;
#[cfg(target_arch = "wasm32")]
use crate::runtime::set_timeout_future;
#[cfg(target_arch = "wasm32")]
use crate::runtime::spawn_drive;
use crate::spec::{OpenCtx, spec_from_params};

/// Bounded mailboxes into one drive task, sized like the resident backend's.
const EVENT_QUEUE_CAPACITY: usize = 64;
const MEDIA_QUEUE_CAPACITY: usize = 64;

/// One running call behind a reserved handle.
pub struct LiveCall {
    id: SessionId,
    call_id: String,
    opus_mlow_escape: bool,
    mic_tx: async_channel::Sender<Vec<i16>>,
    mic_drain: async_channel::Receiver<Vec<i16>>,
    encoded_drain: async_channel::Receiver<Bytes>,
    relay_close: Option<wacore::voip_control::VideoTeardownHook>,
    encoded_tx: async_channel::Sender<Bytes>,
    video_tx: async_channel::Sender<Vec<u8>>,
    timed_video_tx: async_channel::Sender<wacore::voip_control::VideoInput>,
    video_ctl: wacore::voip_control::control::VideoControlSender,
    group_tx: Option<async_channel::Sender<wacore::voip_control::control::GroupControl>>,
    rekey_tx: Option<async_channel::Sender<wacore::voip_control::control::PeerAnswer>>,
    events: async_channel::Receiver<MediaEvent>,
    stats: Arc<media_stats::MediaStatsCell>,
    task: wacore::runtime::AbortHandle,
    fans: Vec<wacore::runtime::AbortHandle>,
    pushes: Arc<PushState>,
    muted: bool,
}

// Terminal delivery stops regular fans; explicit CLOSE also fences a terminal
// fan re-entered through a synchronous JS callback.
#[derive(Default)]
struct PushState(AtomicU8);
impl PushState {
    fn active(&self) -> bool {
        self.0.load(Ordering::Acquire) == 0
    }
    fn terminal(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    fn closed(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
    fn close(&self) -> bool {
        self.0.swap(2, Ordering::AcqRel) == 0
    }
}

/// The push callbacks `open_async` installs, one per push opcode.
/// `pub` because `open_async` in the crate root builds it.
pub struct PushSinks {
    pub event: Arc<dyn Fn(SessionId, AbiEvent) + Send + Sync>,
    pub stats: Arc<dyn Fn(SessionId, StatsData) + Send + Sync>,
    pub pcm: Arc<dyn Fn(SessionId, u32, Vec<u8>) + Send + Sync>,
    pub encoded: Arc<dyn Fn(SessionId, u32, EncodedFrameDto) + Send + Sync>,
    pub video: Arc<dyn Fn(SessionId, u32, VideoFrameDto) + Send + Sync>,
    pub ended: Arc<dyn Fn(SessionId, CloseReason, Option<String>) + Send + Sync>,
}

impl LiveCall {
    /// Builds the engine from decoded open params and starts the drive loop.
    /// The `OPEN` push goes out only after the task is spawned: the ack the
    /// core already received meant setup started, and this is it finishing.
    pub async fn open(
        session: SessionId,
        call_id: String,
        params: OpenParams,
        sinks: PushSinks,
    ) -> Result<Self, OpenError> {
        let generation = session.generation;
        let (spec, ctx) =
            spec_from_params(params, &call_id, generation).map_err(OpenError::Spec)?;
        let endpoint = RelayEndpointParams::from_spec(&spec).ok_or(OpenError::Endpoint)?;
        let opus_mlow_escape = spec.audio.format == MediaAudioFormat::OPUS_MLOW_16KHZ_60MS;
        let parts = wacore::voip_control::engine_bridge::into_engine_parts(spec)
            .map_err(OpenError::Spec)?;
        let mut rng_probe = [0u8; 12];
        getrandom::fill(&mut rng_probe).map_err(|_| OpenError::SecureRandomUnavailable)?;
        let mut engine = CallEngine::new(parts.config, Box::new(RandTxIds))
            .map_err(|e| OpenError::Engine(e.to_string()))?;
        if let Some(group) = parts.group {
            engine
                .configure_group(GroupEngineConfig {
                    call_creator: group.call_creator,
                    self_jid: group.self_jid,
                    initial_update: group.initial_update,
                    direct_peer: group
                        .direct_peer
                        .map(|peer| wacore::voip::engine::DirectPeer {
                            user_jid: peer.user_jid,
                            device_jid: peer.device_jid,
                            call_key: peer.call_key,
                        }),
                })
                .map_err(|e| OpenError::Engine(e.to_string()))?;
        }
        apply_open_ctx(&mut engine, &ctx).map_err(OpenError::Engine)?;

        let dialed = relay::dial(endpoint).await.map_err(OpenError::Relay)?;
        // The pre-attach peer rotations replay as orientation commands once
        // the drive loop owns the video mailbox: the peer itself first,
        // then each routed participant by JID.
        let (mic_tx, mic_rx) = async_channel::bounded(MEDIA_QUEUE_CAPACITY);
        let (spk_tx, spk_rx) = async_channel::bounded(MEDIA_QUEUE_CAPACITY);
        let (enc_tx, enc_rx) = async_channel::bounded::<Bytes>(MEDIA_QUEUE_CAPACITY);
        let (enc_out_tx, enc_out_rx) =
            async_channel::bounded::<MediaEncodedFrame>(MEDIA_QUEUE_CAPACITY);
        let (ev_tx, ev_rx) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
        let (vin_tx, vin_rx) = async_channel::bounded::<Vec<u8>>(MEDIA_QUEUE_CAPACITY);
        let (vout_tx, vout_rx) =
            async_channel::bounded::<wacore::voip_control::ports::VideoFrame>(MEDIA_QUEUE_CAPACITY);
        let (vctl_tx, vctl_rx) = video_control_channel();
        let (rekey_tx, rekey_rx) =
            async_channel::bounded::<wacore::voip_control::control::PeerAnswer>(1);
        let group_tx = parts_group_channel(&engine);
        // The core API requires Arc even on single-threaded WASM.
        #[allow(clippy::arc_with_non_send_sync)]
        let stats = Arc::new(media_stats::MediaStatsCell::default());
        let (timed_video_tx, timed_video_rx) = async_channel::bounded(MEDIA_QUEUE_CAPACITY);
        let mic_drain = mic_rx.clone();
        let encoded_drain = enc_rx.clone();
        let terminal_events = ev_tx.clone();
        let channels = CallChannels {
            mic: mic_rx,
            speaker: spk_tx,
            encoded_audio_in: enc_rx,
            encoded_audio_out: enc_out_tx,
            events: ev_tx,
            rekey: Some(rekey_rx),
            video_in: vin_rx,
            timed_video_in: Some(timed_video_rx),
            video_out: vout_tx,
            video_ctl: vctl_rx,
            group_ctl: group_tx.1,
            media_stats: stats.clone(),
        };
        let rt: Arc<dyn wacore::runtime::Runtime> = Arc::new(EngineRuntime);
        let drive = {
            let transport = dialed.transport.clone();
            let relay_events = dialed.relay_events.clone();
            async move {
                let reason =
                    wacore::voip::driver::run_call(rt, transport, relay_events, channels, engine)
                        .await;
                // The portable driver returns some terminal reasons without an
                // engine Closed event. Publish those through the same ordered fan.
                let _ = terminal_events.force_send(MediaEvent::Closed(reason));
            }
        };
        #[cfg(target_arch = "wasm32")]
        let task = spawn_drive(drive);
        #[cfg(not(target_arch = "wasm32"))]
        let task = {
            drop(drive);
            wacore::runtime::AbortHandle::noop()
        };
        let mut live = LiveCall {
            id: session,
            call_id,
            opus_mlow_escape,
            mic_tx,
            mic_drain,
            encoded_drain,
            relay_close: Some(dialed.close),
            encoded_tx: enc_tx,
            video_tx: vin_tx,
            timed_video_tx,
            video_ctl: vctl_tx,
            group_tx: group_tx.0,
            rekey_tx: Some(rekey_tx),
            events: ev_rx,
            stats,
            task,
            fans: Vec::new(),
            pushes: Arc::new(PushState::default()),
            muted: ctx.muted,
        };
        live.replay_orientations(&ctx.peer_orientations);
        live.fans = live.spawn_fans(spk_rx, enc_out_rx, vout_rx, sinks);
        Ok(live)
    }

    fn spawn_fans(
        &self,
        speaker: async_channel::Receiver<Vec<i16>>,
        encoded_out: async_channel::Receiver<MediaEncodedFrame>,
        video_out: async_channel::Receiver<wacore::voip_control::ports::VideoFrame>,
        sinks: PushSinks,
    ) -> Vec<wacore::runtime::AbortHandle> {
        let mut fans = vec![spawn_event_fan(
            self.id,
            self.events.clone(),
            self.stats.clone(),
            &sinks,
            self.pushes.clone(),
        )];
        let PushSinks {
            event: _,
            stats: push_stats,
            pcm: push_pcm,
            encoded: push_encoded,
            video: push_video,
            ended: _,
        } = sinks;
        fans.push(spawn_pcm_fan(
            self.id,
            speaker,
            push_pcm,
            self.pushes.clone(),
        ));
        let id = self.id;
        let pushes = self.pushes.clone();
        fans.push(EngineRuntime.spawn(Box::pin(async move {
            let mut seq = 0u32;
            while let Ok(frame) = encoded_out.recv().await {
                if !pushes.active() {
                    break;
                }
                if let Some(frame) = encoded_frame_to_dto(&frame) {
                    push_encoded(id, seq, frame);
                }
                seq = seq.wrapping_add(1);
            }
        })));
        let id = self.id;
        let pushes = self.pushes.clone();
        fans.push(EngineRuntime.spawn(Box::pin(async move {
            let mut seq = 0u32;
            while let Ok(frame) = video_out.recv().await {
                if !pushes.active() {
                    break;
                }
                push_video(id, seq, video_frame_to_dto(&frame));
                seq = seq.wrapping_add(1);
            }
        })));
        fans.push(spawn_stats_fan(
            self.id,
            self.stats.clone(),
            push_stats,
            self.pushes.clone(),
        ));
        fans
    }

    /// Feeds one inbound PCM frame to the engine's mic mailbox. Loss
    /// tolerant: a full mailbox sheds, like the core's own pump.
    pub fn pcm_in(&self, frame: &MediaFrame) {
        if self.muted {
            return;
        }
        let samples: Vec<i16> = frame
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let _ = self.mic_tx.try_send(samples);
    }

    /// Feeds one encoded packet to the engine's encoded mailbox.
    pub fn encoded_in(&self, data: &[u8]) {
        if self.muted {
            return;
        }
        if let Some(packet) = encoded_input_packet(data, self.opus_mlow_escape) {
            let _ = self.encoded_tx.try_send(packet);
        }
    }

    /// Feeds one outbound access unit to the engine's video mailbox: timed
    /// units carry their capture stamp, untimed ones pace freely.
    pub fn video_in(&self, input: &VideoInputDto) {
        match (input.timestamp, input.input_generation) {
            (Some(timestamp), Some(generation)) => {
                let _ = self.timed_video_tx.try_send(
                    wacore::voip_control::VideoInput::builder()
                        .data(input.data.clone())
                        .timestamp(timestamp)
                        .generation(generation)
                        .build(),
                );
            }
            _ => {
                let _ = self.video_tx.try_send(input.data.clone());
            }
        }
    }

    /// Replays the rotations the peer announced before media attached.
    /// A JID the core would not parse drops that entry rather than fails
    /// the open: one bad rotation must not cost the call.
    fn replay_orientations(&self, orientations: &[(Option<String>, u8)]) {
        use wacore::voip_control::control::VideoControl;
        for (participant, orientation) in orientations {
            match participant {
                None => {
                    self.video_ctl
                        .send(VideoControl::SetOrientation(*orientation));
                }
                Some(who) => match who.parse::<Jid>() {
                    Ok(jid) => {
                        self.video_ctl
                            .send(VideoControl::SetParticipantOrientation {
                                participant: jid,
                                orientation: *orientation,
                            });
                    }
                    Err(_) => continue,
                },
            }
        }
    }

    /// Applies one control intent to the live call. Group updates route to
    /// the group mailbox; video and rekey intents to theirs; mute flips the
    /// local gate the mic pump reads. `false` is backpressure or a call with
    /// no group plane for a group intent.
    pub fn command(&mut self, command: MediaCommand) -> bool {
        use wacore::voip_control::control::{
            GroupControl, GroupRawEpoch, PeerAnswer, VideoControl,
        };
        match command {
            MediaCommand::VideoEnable => self.video_ctl.send(VideoControl::Enable),
            MediaCommand::VideoEnableAwaitingAccept => {
                self.video_ctl.send(VideoControl::EnableAwaitingAccept)
            }
            MediaCommand::VideoDisable => self.video_ctl.send(VideoControl::Disable),
            MediaCommand::VideoDisableKeepLegacy => {
                self.video_ctl.send(VideoControl::DisableKeepLegacy)
            }
            MediaCommand::VideoDisableOutbound => {
                self.video_ctl.send(VideoControl::DisableOutbound)
            }
            MediaCommand::VideoRequireKeyframe => {
                self.video_ctl.send(VideoControl::RequireKeyframe)
            }
            MediaCommand::VideoRequestPeerKeyframe(urgency) => {
                self.video_ctl
                    .send(VideoControl::RequestPeerKeyframe(match urgency {
                        Urgency::Coalesced => MediaKeyframeUrgency::Coalesced,
                        Urgency::Immediate => MediaKeyframeUrgency::Immediate,
                    }))
            }
            MediaCommand::VideoSetOrientation {
                participant,
                orientation,
            } => match participant {
                Some(who) => match who.parse::<Jid>() {
                    Ok(jid) => self
                        .video_ctl
                        .send(VideoControl::SetParticipantOrientation {
                            participant: jid,
                            orientation,
                        }),
                    Err(_) => false,
                },
                None => self
                    .video_ctl
                    .send(VideoControl::SetOrientation(orientation)),
            },
            MediaCommand::VideoSetInputGeneration(generation) => self
                .video_ctl
                .send(VideoControl::SetInputGeneration(generation)),
            MediaCommand::VideoSetTimestampStride(stride) => self
                .video_ctl
                .send(VideoControl::SetTimestampStride(stride)),
            MediaCommand::RekeyRecv {
                answering_lid,
                audio_codec,
            } => {
                let Some(rekey) = self.rekey_tx.as_ref() else {
                    return false;
                };
                let codec = audio_codec.map(|c| match c {
                    voip_abi::AudioCodecWire::Mlow => MediaAudioCodec::Mlow,
                    voip_abi::AudioCodecWire::Opus => MediaAudioCodec::Opus,
                });
                rekey
                    .try_send(
                        PeerAnswer::builder()
                            .answering_lid(answering_lid)
                            .maybe_audio_codec(codec)
                            .build(),
                    )
                    .is_ok()
            }
            MediaCommand::GroupApplyUpdate(update) => {
                let Some(tx) = self.group_tx.as_ref() else {
                    return false;
                };
                match group_update_from_dto(&update) {
                    Ok(parsed) => tx.try_send(GroupControl::Update(Box::new(parsed))).is_ok(),
                    Err(_) => false,
                }
            }
            MediaCommand::GroupApplyTransition {
                update,
                transaction_id,
                epoch,
            } => {
                let Some(tx) = self.group_tx.as_ref() else {
                    return false;
                };
                match group_update_from_dto(&update) {
                    Ok(parsed) => tx
                        .try_send(GroupControl::Transition {
                            update: Box::new(parsed),
                            epoch: GroupRawEpoch::new(transaction_id, epoch.as_bytes().to_vec()),
                        })
                        .is_ok(),
                    Err(_) => false,
                }
            }
            MediaCommand::GroupApplyEpoch {
                transaction_id,
                epoch,
            } => {
                let Some(tx) = self.group_tx.as_ref() else {
                    return false;
                };
                tx.try_send(GroupControl::RawEpoch(GroupRawEpoch::new(
                    transaction_id,
                    epoch.as_bytes().to_vec(),
                )))
                .is_ok()
            }
            MediaCommand::GroupSendReaction(emoji) => {
                let Some(tx) = self.group_tx.as_ref() else {
                    return false;
                };
                tx.try_send(GroupControl::Reaction(emoji)).is_ok()
            }
            MediaCommand::AudioMute(flag) => {
                self.muted = flag != 0;
                if self.muted {
                    while self.mic_drain.try_recv().is_ok() {}
                    while self.encoded_drain.try_recv().is_ok() {}
                }
                true
            }
        }
    }

    /// Answers the admission probe: a live group plane admits, a 1:1 call
    /// fits no roster. Mirrors the core's always-admit preflight: the
    /// delivery answer stays authoritative.
    pub fn group_fits(&self, update: &voip_abi::GroupCallUpdateDto) -> (u8, u32) {
        if self.group_tx.is_some() {
            (1, 0)
        } else {
            let _ = update;
            (0, 0)
        }
    }

    /// Reads the latest published counters.
    pub fn stats(&self) -> StatsData {
        stats_to_wire(&self.stats.snapshot())
    }

    /// Ends the call: aborts the drive task, which drops the transport and
    /// closes the relay channel with it.
    pub fn close(mut self) -> (SessionId, String, Option<StatsData>) {
        let publish_final = self.pushes.close();
        for fan in self.fans.drain(..) {
            fan.abort();
        }
        let stats = publish_final.then(|| self.stats());
        (self.id, self.call_id.clone(), stats)
    }
}

impl Drop for LiveCall {
    fn drop(&mut self) {
        self.pushes.close();
        for fan in self.fans.drain(..) {
            fan.abort();
        }
        if let Some(close) = self.relay_close.take() {
            close();
        }
        self.task.abort();
    }
}

fn spawn_pcm_fan(
    id: SessionId,
    speaker: async_channel::Receiver<Vec<i16>>,
    push_pcm: Arc<dyn Fn(SessionId, u32, Vec<u8>) + Send + Sync>,
    pushes: Arc<PushState>,
) -> wacore::runtime::AbortHandle {
    EngineRuntime.spawn(Box::pin(async move {
        let mut seq = 0u32;
        while let Ok(samples) = speaker.recv().await {
            if !pushes.active() {
                break;
            }
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for sample in samples {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            push_pcm(id, seq, bytes);
            seq = seq.wrapping_add(1);
        }
    }))
}

fn spawn_event_fan(
    id: SessionId,
    events: async_channel::Receiver<MediaEvent>,
    stats: Arc<media_stats::MediaStatsCell>,
    sinks: &PushSinks,
    pushes: Arc<PushState>,
) -> wacore::runtime::AbortHandle {
    let push_event = sinks.event.clone();
    let push_stats = sinks.stats.clone();
    let push_ended = sinks.ended.clone();
    EngineRuntime.spawn(Box::pin(async move {
        while let Ok(event) = events.recv().await {
            if !pushes.active() {
                break;
            }
            if let MediaEvent::Closed(reason) = &event {
                if pushes.terminal() {
                    let (wire, detail) = close_reason_to_wire(reason);
                    push_stats(id, stats_to_wire(&stats.snapshot()));
                    if !pushes.closed() {
                        push_ended(id, wire, detail);
                    }
                }
                break;
            }
            if let Some(wire) = event_to_wire(&event) {
                push_event(id, wire);
            }
        }
    }))
}

/// The counters fan polls the cell the drive loop publishes into. Unlike the
/// channel fans, it has no sender that closes with the drive task: its abort
/// handle belongs to the live call so a completed call leaves no timer.
fn spawn_stats_fan(
    id: SessionId,
    stats: Arc<media_stats::MediaStatsCell>,
    push_stats: Arc<dyn Fn(SessionId, StatsData) + Send + Sync>,
    pushes: Arc<PushState>,
) -> wacore::runtime::AbortHandle {
    EngineRuntime.spawn(Box::pin(async move {
        let mut last = wacore::voip_control::MediaStats::default();
        loop {
            set_timeout_ms(1000).await;
            if !pushes.active() {
                break;
            }
            let now = stats.snapshot();
            if now != last {
                last = now;
                push_stats(id, stats_to_wire(&now));
            }
        }
    }))
}

/// One encoded packet out of the engine, re-exported for the push path.
pub use voip_abi::EncodedAudioOut;

/// Why an open never became a call. The `Debug` rendering is what rides
/// the `EVENT(MEDIA_SETUP_FAILED)` push, so every variant's payload stays
/// readable there rather than behind a dead-code lint. The wire code rides
/// beside it: transport stays `Connect` in the core's setup grammar,
/// everything else is setup.
#[derive(Debug)]
#[allow(dead_code)]
pub enum OpenError {
    Spec(MediaSetupError),
    Endpoint,
    SecureRandomUnavailable,
    Engine(String),
    Relay(String),
}

impl OpenError {
    pub fn detail(&self) -> String {
        match self {
            Self::Spec(error) => match error {
                MediaSetupError::BadCallKey => "call key must contain at least 32 bytes".to_owned(),
                MediaSetupError::BadEndpoint => "invalid relay endpoint".to_owned(),
                MediaSetupError::BadAudioFormat => {
                    "invalid audio timing or channel count".to_owned()
                }
                MediaSetupError::UnsupportedPcmAudio => "unsupported PCM audio format".to_owned(),
                MediaSetupError::MlowUnavailable => "MLOW codec is unavailable".to_owned(),
                MediaSetupError::Relay(detail) => format!("relay setup failed: {detail}"),
                MediaSetupError::Connect(detail) => format!("relay connection failed: {detail}"),
                MediaSetupError::Backend(detail) => format!("media backend failed: {detail}"),
                MediaSetupError::NoBackend => "no media backend installed".to_owned(),
                _ => "unsupported media setup failure".to_owned(),
            },
            Self::Endpoint => "no usable relay endpoint".to_owned(),
            Self::SecureRandomUnavailable => "secure random source unavailable".to_owned(),
            Self::Engine(_) => "engine configuration refused".to_owned(),
            Self::Relay(detail) => format!("relay connection failed: {detail}"),
        }
    }
    pub fn code(&self) -> voip_abi::AbiErrorCode {
        match self {
            OpenError::Spec(_) => voip_abi::AbiErrorCode::BadPayload,
            OpenError::Endpoint | OpenError::Relay(_) => voip_abi::AbiErrorCode::Transport,
            OpenError::Engine(_) | OpenError::SecureRandomUnavailable => {
                voip_abi::AbiErrorCode::Internal
            }
        }
    }
}

/// OS-RNG STUN transaction ids for live calls. The core's `SequentialTxIds`
/// is deterministic and test-only; real calls need unpredictable ids for
/// consent freshness, so this side reads its own RNG and nothing else.
#[derive(Default)]
struct RandTxIds;

impl wacore::voip::engine::TxIdSource for RandTxIds {
    fn next_tx_id(&mut self) -> [u8; 12] {
        let mut id = [0u8; 12];
        getrandom::fill(&mut id).expect("secure random source failed after setup");
        id
    }
}

fn apply_open_ctx(engine: &mut CallEngine, ctx: &OpenCtx) -> Result<(), String> {
    use wacore::voip::engine::CodecDecisionSource;
    if let Some((transaction_id, epoch)) = ctx.epoch.as_ref() {
        engine
            .apply_group_raw_epoch(*transaction_id, epoch.as_bytes())
            .map_err(|e| format!("pre-attach epoch refused: {e:?}"))?;
    }
    if let Some(codec) = ctx.initial_codec {
        engine
            .switch_audio_codec(codec, CodecDecisionSource::Negotiated)
            .map_err(|e| format!("initial codec refused: {e:?}"))?;
    }
    Ok(())
}

type GroupControlSender = async_channel::Sender<wacore::voip_control::control::GroupControl>;
type GroupControlReceiver = async_channel::Receiver<wacore::voip_control::control::GroupControl>;

fn parts_group_channel(
    engine: &CallEngine,
) -> (Option<GroupControlSender>, Option<GroupControlReceiver>) {
    if engine.is_group() {
        let (tx, rx) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    }
}

fn group_update_from_dto(
    dto: &voip_abi::GroupCallUpdateDto,
) -> Result<wacore::types::group_call::GroupCallUpdate, String> {
    crate::spec::update_from_dto(dto, &|raw| {
        raw.parse::<Jid>()
            .map_err(|_| format!("group JID refused: {raw}"))
    })
}

fn close_reason_to_wire(reason: &MediaCloseReason) -> (CloseReason, Option<String>) {
    match reason {
        MediaCloseReason::Local => (CloseReason::LocalHangup, None),
        MediaCloseReason::RelayDisconnected => (CloseReason::RelayDropped, None),
        MediaCloseReason::SendFailed(detail) => (CloseReason::Failed, Some(detail.clone())),
        MediaCloseReason::SetupFailed(detail) => (CloseReason::Failed, Some(detail.clone())),
        _ => (CloseReason::Failed, None),
    }
}

fn stats_to_wire(stats: &wacore::voip_control::MediaStats) -> StatsData {
    StatsData {
        rtp_received: stats.rtp_received,
        rtp_payload_type_unexpected: stats.rtp_payload_type_unexpected,
        srtp_unprotect_failed: stats.srtp_unprotect_failed,
        audio_frames_decoded: stats.audio_frames_decoded,
        audio_frames_delivered: stats.audio_frames_delivered,
        audio_frames_concealed: stats.audio_frames_concealed,
        sframe_decrypt_failed: stats.sframe_decrypt_failed,
        mlow_off_point_dropped: stats.mlow_off_point_dropped,
        mlow_inactive_or_sid: stats.mlow_inactive_or_sid,
        foreign_frames_decoded: stats.foreign_frames_decoded,
        audio_frames_without_decoder: stats.audio_frames_without_decoder,
        outbound_frames_without_encoder: stats.outbound_frames_without_encoder,
        playout_trimmed_samples: stats.playout_trimmed_samples,
        inbound_pipe_dropped: stats.inbound_pipe_dropped,
        audio_sink_dropped: stats.audio_sink_dropped,
        video_sink_dropped: stats.video_sink_dropped,
        peer_keyframe_requests: stats.peer_keyframe_requests,
        relay_packet_unclassified: stats.relay_packet_unclassified,
        forwarding_envelope_rejected: stats.forwarding_envelope_rejected,
        codec_switches: stats.codec_switches.into(),
    }
}

fn encoded_frame_to_dto(frame: &MediaEncodedFrame) -> Option<EncodedFrameDto> {
    let format = &frame.format;
    Some(EncodedFrameDto {
        format: Some(voip_abi::AudioFormatDto {
            codec: match format.codec {
                MediaAudioCodec::Mlow => voip_abi::AudioCodecWire::Mlow,
                MediaAudioCodec::Opus => voip_abi::AudioCodecWire::Opus,
                _ => return None,
            },
            rtp_profile: match format.rtp_profile {
                wacore::voip_control::MediaAudioRtpProfile::Mlow => voip_abi::RtpProfile::Mlow,
                wacore::voip_control::MediaAudioRtpProfile::StandardOpus => {
                    voip_abi::RtpProfile::StandardOpus
                }
                _ => return None,
            },
            signaling_rate: format.signaling_rate,
            sample_rate: format.sample_rate,
            channels: format.channels,
            samples_per_frame: format.samples_per_frame,
            rtp_clock_rate: format.rtp_clock_rate,
            rtp_timestamp_step: format.rtp_timestamp_step,
            rtp_payload_type: format.rtp_payload_type,
        }),
        codec: match frame.codec {
            MediaAudioCodec::Mlow => voip_abi::AudioCodecWire::Mlow,
            MediaAudioCodec::Opus => voip_abi::AudioCodecWire::Opus,
            _ => return None,
        },
        data: frame.data.to_vec(),
        payload_type: frame.payload_type,
        sequence_number: u32::from(frame.sequence_number),
        timestamp: frame.timestamp,
        marker: u8::from(frame.marker),
        sender: frame.sender.as_ref().map(ToString::to_string),
        device: frame.device.as_ref().map(ToString::to_string),
    })
}

fn video_frame_to_dto(frame: &wacore::voip_control::ports::VideoFrame) -> VideoFrameDto {
    VideoFrameDto {
        data: frame.data.clone(),
        keyframe: u8::from(frame.keyframe),
        orientation: frame.orientation,
        sender: frame.sender.as_ref().map(ToString::to_string),
        device: frame.device.as_ref().map(ToString::to_string),
        pid: frame.pid,
        timestamp: frame.timestamp,
    }
}

/// Projects one engine event into the wire vocabulary. `Closed` never
/// becomes an event here: the fan raises it through the ended path, on both
/// the event and close paths. `None` is a JID the core would not parse or a
/// signaling-born variant the engine never raises; the fan drops it rather
/// than invents it.
fn event_to_wire(event: &MediaEvent) -> Option<AbiEvent> {
    match event {
        MediaEvent::RelayAllocated => Some(AbiEvent::RelayAllocated),
        MediaEvent::ForeignAudio(data) => Some(AbiEvent::ForeignAudio(data.to_vec())),
        MediaEvent::ForeignGroupAudio(frame) => {
            Some(AbiEvent::ForeignGroupAudio(encoded_frame_to_dto(frame)?))
        }
        MediaEvent::AudioFormatMismatch {
            expected_rate,
            received_rates,
        } => Some(AbiEvent::AudioFormatMismatch {
            expected_rate: *expected_rate,
            received_rates: received_rates.clone(),
        }),
        MediaEvent::RelayAllocateFailed(code) => {
            Some(AbiEvent::RelayAllocateFailed(u32::from(*code)))
        }
        MediaEvent::RelayAllocateTimedOut => Some(AbiEvent::RelayAllocateTimedOut),
        MediaEvent::MediaSetupFailed(detail) => Some(AbiEvent::MediaSetupFailed(detail.clone())),
        MediaEvent::RelayReconnectTimedOut => Some(AbiEvent::RelayReconnectTimedOut),
        MediaEvent::VideoKeyframeNeeded => Some(AbiEvent::VideoKeyframeNeeded),
        MediaEvent::RtcpReceived {
            packet_types,
            sender_ssrc,
            referenced_ssrcs,
            reports_audio,
            reports_video,
            report_blocks,
            feedback,
        } => Some(AbiEvent::RtcpReceived {
            packet_types: packet_types.clone(),
            sender_ssrc: *sender_ssrc,
            referenced_ssrcs: referenced_ssrcs.clone(),
            reports_audio: u8::from(*reports_audio),
            reports_video: u8::from(*reports_video),
            report_blocks: report_blocks
                .iter()
                .map(
                    |b: &wacore::voip_control::MediaRtcpReportBlock| voip_abi::RtcpBlockDto {
                        ssrc: b.ssrc,
                        fraction_lost: b.fraction_lost,
                        cumulative_lost: b.cumulative_lost,
                        extended_highest_sequence: b.extended_highest_sequence,
                        jitter: b.jitter,
                        last_sender_report: b.last_sender_report,
                        delay_since_last_sender_report: b.delay_since_last_sender_report,
                        profile_extension: b.profile_extension.clone(),
                    },
                )
                .collect(),
            feedback: feedback
                .iter()
                .map(
                    |f: &wacore::voip_control::MediaRtcpFeedback| voip_abi::RtcpFeedbackDto {
                        packet_type: f.packet_type,
                        fmt: f.fmt,
                        sender_ssrc: f.sender_ssrc,
                        media_ssrc: f.media_ssrc,
                        fci: f.fci.clone(),
                    },
                )
                .collect(),
        }),
        MediaEvent::OutboundMediaDropped {
            video_access_units,
            packets,
        } => Some(AbiEvent::OutboundMediaDropped {
            video_access_units: *video_access_units,
            packets: *packets,
        }),
        MediaEvent::AudioSilent {
            silent_for_ms,
            rtp_received,
            frames_produced,
            dominant_reason,
        } => Some(AbiEvent::AudioSilent {
            silent_for_ms: *silent_for_ms,
            rtp_received: *rtp_received,
            frames_produced: *frames_produced,
            dominant_reason: match dominant_reason {
                MediaSilenceReason::NoDecoderForNegotiatedCodec => {
                    SilenceReason::NoDecoderForNegotiatedCodec
                }
                MediaSilenceReason::AuthenticationFailing => SilenceReason::AuthenticationFailing,
                MediaSilenceReason::UnexpectedPayloadType => SilenceReason::UnexpectedPayloadType,
                MediaSilenceReason::CodecRejectingFrames => SilenceReason::CodecRejectingFrames,
                MediaSilenceReason::CodecFlapping => SilenceReason::CodecFlapping,
                MediaSilenceReason::Unknown => SilenceReason::Unknown,
                _ => SilenceReason::Unknown,
            },
        }),
        MediaEvent::AudioCodecSwitched {
            from,
            to,
            source,
            packets_observed,
        } => Some(AbiEvent::AudioCodecSwitched {
            from: match from {
                MediaAudioCodec::Mlow => voip_abi::AudioCodecWire::Mlow,
                MediaAudioCodec::Opus => voip_abi::AudioCodecWire::Opus,
                _ => return None,
            },
            to: match to {
                MediaAudioCodec::Mlow => voip_abi::AudioCodecWire::Mlow,
                MediaAudioCodec::Opus => voip_abi::AudioCodecWire::Opus,
                _ => return None,
            },
            source: match source {
                MediaCodecDecisionSource::Negotiated => CodecSource::Negotiated,
                MediaCodecDecisionSource::Content => CodecSource::Content,
                _ => return None,
            },
            packets_observed: *packets_observed,
        }),
        MediaEvent::AudioCodecSourceIsFixed {
            sending,
            peer_expects,
            source,
        } => Some(AbiEvent::AudioCodecSourceIsFixed {
            sending: match sending {
                MediaAudioCodec::Mlow => voip_abi::AudioCodecWire::Mlow,
                MediaAudioCodec::Opus => voip_abi::AudioCodecWire::Opus,
                _ => return None,
            },
            peer_expects: match peer_expects {
                MediaAudioCodec::Mlow => voip_abi::AudioCodecWire::Mlow,
                MediaAudioCodec::Opus => voip_abi::AudioCodecWire::Opus,
                _ => return None,
            },
            source: match source {
                MediaCodecDecisionSource::Negotiated => CodecSource::Negotiated,
                MediaCodecDecisionSource::Content => CodecSource::Content,
                _ => return None,
            },
        }),
        MediaEvent::AudioReceptionStalled { silent_for_ms } => {
            Some(AbiEvent::AudioReceptionStalled {
                silent_for_ms: *silent_for_ms,
            })
        }
        // `Closed` never becomes an event here: the fan raises it through
        // the ended path instead. Signaling-born variants the engine never
        // raises (video state, group, reactions, ...) have no wire spelling
        // and drop here rather than invent one.
        MediaEvent::Closed(_)
        | MediaEvent::PeerVideoStateChanged { .. }
        | MediaEvent::GroupUpdated(_)
        | MediaEvent::WaitingRoomUpdated(_)
        | MediaEvent::WaitingRoomHeartbeatFailed
        | MediaEvent::GroupControlRejected { .. }
        | MediaEvent::GroupRekeyFailed
        | MediaEvent::HandRaised { .. }
        | MediaEvent::ScreenShareChanged { .. }
        | MediaEvent::Reaction { .. } => None,
        _ => None,
    }
}

/// Builds the push frame for one engine event.
pub fn event_push(session: SessionId, event: AbiEvent) -> Vec<u8> {
    use voip_abi::{ABI_MAJOR, ABI_MINOR, EventNotification, Frame, Opcode};
    Frame {
        major: ABI_MAJOR,
        minor: ABI_MINOR,
        opcode: Opcode::Event,
        flags: 0,
        payload: EventNotification { session, event }.encode(),
    }
    .encode()
}

/// Builds the push frame for one stats snapshot.
pub fn stats_push(session: SessionId, stats: StatsData) -> Vec<u8> {
    use voip_abi::{ABI_MAJOR, ABI_MINOR, Frame, Opcode, StatsPush};
    Frame {
        major: ABI_MAJOR,
        minor: ABI_MINOR,
        opcode: Opcode::Stats,
        flags: 0,
        payload: StatsPush { session, stats }.encode(),
    }
    .encode()
}

async fn set_timeout_ms(ms: u32) {
    #[cfg(target_arch = "wasm32")]
    {
        set_timeout_future(ms.min(i32::MAX as u32) as i32).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ms;
        futures::future::pending::<()>().await;
    }
}

// ---------------------------------------------------------------------------
// Proof the engine runs: a host-side test that builds a real `CallEngine`
// from a real `OpenParams` and drives `run_call` to its first STUN allocate.
// ---------------------------------------------------------------------------
//
// Why this is conclusive without a wasm runtime: `run_call` is executor-free
// — it takes an `Arc<dyn Runtime>` and a `RelayTransport`, both injected —
// so the same engine, the same driver, and the same spec mapping run here on
// a blocking executor with a fake relay. What differs on wasm32 is only the
// shell (the `EngineRuntime` sleep/spawn and the JS pipe), and those are
// thin adapters with no call logic. If the engine failed to build from these
// params, or the driver failed to emit its allocate, this test would fail.

// ---------------------------------------------------------------------------
// Proof the engine runs: a host-side test that builds a real `CallEngine`
// from a real `OpenParams` and drives its STUN allocate / binding answer.
// ---------------------------------------------------------------------------
//
// Why this is conclusive without a wasm runtime or a relay: the wasm path
// (`LiveCall::open`) does three things — map `OpenParams` into a spec
// (`spec_from_params`, pure), split the spec into engine parts
// (`into_engine_parts`), and hand the engine to `run_call` with the JS
// relay. This test exercises the first two plus the engine's own
// allocate/answer cycle directly: `CallEngine::new` from the mapped spec,
// `start`, one `poll_output` drained for the STUN allocate, the binding
// request fed back through `handle_input`, and the drain showing the
// binding success. No executor, no `Runtime` impl, no `RelayTransport`
// impl — the test never names a trait whose `Send` shape differs between
// host and wasm32, which is why it compiles on both. (An earlier revision
// implemented `Runtime`/`RelayTransport` for fakes and hit exactly that:
// the host traits demand `Send`, the wasm32 ones do not, so one impl cannot
// satisfy `clippy --all-targets` on both. The sans-IO engine needs neither.)
/// The engine, not core.wasm, owns the codec payload grammar. Invalid CELT
/// packets must not reach the engine under an MLOW RTP profile.
fn encoded_input_packet(data: &[u8], opus_mlow_escape: bool) -> Option<Bytes> {
    let mut packet = data.to_vec();
    if opus_mlow_escape && wacore::voip::packetize_opus_for_mlow(&mut packet).is_err() {
        return None;
    }
    Some(Bytes::from(packet))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn video_output_mapper_preserves_owned_access_unit_metadata() {
        let frame = wacore::voip_control::VideoFrame::builder()
            .data(vec![0, 0, 0, 1, 0x65, 0x80])
            .keyframe(true)
            .orientation(3)
            .sender("100000000000001@lid".parse().unwrap())
            .device("100000000000001:2@lid".parse().unwrap())
            .pid(0)
            .timestamp(u32::MAX)
            .generation(42)
            .build();
        let dto = video_frame_to_dto(&frame);
        assert_eq!(VideoFrameDto::decode(&dto.encode()).unwrap(), dto);
        assert_eq!(dto.data, frame.data);
        assert_eq!(dto.keyframe, 1);
        assert_eq!(dto.orientation, 3);
        assert_eq!(dto.sender, frame.sender.as_ref().map(ToString::to_string));
        assert_eq!(dto.device, frame.device.as_ref().map(ToString::to_string));
        assert_eq!(dto.pid, Some(0));
        assert_eq!(dto.timestamp, u32::MAX);
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn encoded_mapper_uses_the_frames_complete_format_after_codec_switch() {
        for format in [
            MediaAudioFormat::MLOW_16KHZ_60MS,
            MediaAudioFormat::OPUS_16KHZ_60MS,
            MediaAudioFormat::OPUS_MLOW_16KHZ_60MS,
            MediaAudioFormat::OPUS_RFC7587_48KHZ_60MS,
        ] {
            let frame = MediaEncodedFrame::builder()
                .format(format)
                .codec(format.codec)
                .data(Bytes::from_static(&[0x58, 1, 2]))
                .payload_type(format.rtp_payload_type)
                .sequence_number(u16::MAX)
                .timestamp(u32::MAX)
                .marker(true)
                .build();
            let dto = encoded_frame_to_dto(&frame).unwrap();
            let decoded = EncodedFrameDto::decode(&dto.encode()).unwrap();
            let restored = crate::spec::format_from_dto(decoded.format.as_ref().unwrap()).unwrap();
            assert_eq!(restored, format);
            assert_eq!(decoded.sequence_number, u32::from(u16::MAX));
            assert_eq!(decoded.timestamp, u32::MAX);
            assert_eq!(decoded.payload_type, frame.payload_type);
        }
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test(async)]
    async fn live_stats_fan_pushes_published_counters() {
        let (tx, rx) = futures::channel::oneshot::channel();
        let sender = Arc::new(std::sync::Mutex::new(Some(tx)));
        let push = Arc::new(move |_: SessionId, snapshot: StatsData| {
            if let Some(tx) = sender.lock().unwrap().take() {
                let _ = tx.send(snapshot);
            }
        });
        // The core stats fan takes Arc on both target shapes.
        #[allow(clippy::arc_with_non_send_sync)]
        let cell = Arc::new(media_stats::MediaStatsCell::default());
        let fan = spawn_stats_fan(
            SessionId {
                handle: 1,
                generation: 1,
            },
            cell.clone(),
            push,
            Arc::new(PushState::default()),
        );
        cell.publish(
            wacore::voip_control::MediaStats::builder()
                .rtp_received(7)
                .build(),
        );
        assert_eq!(rx.await.expect("stats pushed").rtp_received, 7);
        drop(fan);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn encoded_input_rewrites_only_opus_mlow() {
        let celt = [0xbb, 0x03, 1, 2, 3, 4, 5, 6];
        let escaped = encoded_input_packet(&celt, true).expect("CELT packet");
        assert_eq!(escaped[0], 0xdd);
        assert_eq!(&escaped[1..], &celt[1..]);
        assert_eq!(encoded_input_packet(&celt, false).unwrap().as_ref(), celt);
        assert_eq!(
            encoded_input_packet(&[0xbb, 0x03], true).unwrap().as_ref(),
            [0x90]
        );
        assert!(encoded_input_packet(&[0x08, 1, 2], true).is_none());
    }

    #[cfg(target_arch = "wasm32")]
    mod terminal_tests {
        use super::*;
        use std::sync::Mutex;
        use wasm_bindgen::JsCast;
        use wasm_bindgen_test::wasm_bindgen_test;

        #[derive(Debug, Clone, PartialEq, Eq)]
        enum Push {
            Stats(StatsData),
            Ended,
        }

        fn sinks(
            log: Arc<Mutex<Vec<Push>>>,
            ended: futures::channel::oneshot::Sender<()>,
        ) -> PushSinks {
            let stats_log = log.clone();
            let ended = Arc::new(Mutex::new(Some(ended)));
            PushSinks {
                event: Arc::new(|_, _| {}),
                stats: Arc::new(move |_, stats| stats_log.lock().unwrap().push(Push::Stats(stats))),
                pcm: Arc::new(|_, _, _| {}),
                encoded: Arc::new(|_, _, _| {}),
                video: Arc::new(|_, _, _| {}),
                ended: Arc::new(move |_, _, _| {
                    log.lock().unwrap().push(Push::Ended);
                    if let Some(sender) = ended.lock().unwrap().take() {
                        let _ = sender.send(());
                    }
                }),
            }
        }

        async fn wait_ended(ended: futures::channel::oneshot::Receiver<()>) {
            let timeout = set_timeout_ms(750);
            futures::pin_mut!(ended, timeout);
            assert!(
                matches!(
                    futures::future::select(ended, timeout).await,
                    futures::future::Either::Left((Ok(()), _))
                ),
                "terminal notification did not arrive before a periodic stats tick"
            );
        }

        #[wasm_bindgen_test(async)]
        async fn a_reentrant_close_stops_the_already_polling_pcm_fan() {
            let state = Arc::new(PushState::default());
            let callback_state = state.clone();
            let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let callbacks = count.clone();
            let (sender, receiver) = async_channel::bounded(2);
            sender.try_send(vec![1, 2]).unwrap();
            sender.try_send(vec![3, 4]).unwrap();
            let _fan = spawn_pcm_fan(
                SessionId {
                    handle: 80,
                    generation: 1,
                },
                receiver,
                Arc::new(move |_, _, _| {
                    callbacks.fetch_add(1, Ordering::Relaxed);
                    callback_state.close();
                }),
                state,
            );
            set_timeout_ms(0).await;
            assert_eq!(
                count.load(Ordering::Relaxed),
                1,
                "AbortHandle alone cannot stop a currently polling fan after synchronous CLOSE"
            );
        }

        #[wasm_bindgen_test(async)]
        async fn aborting_an_idle_fan_releases_its_callback_and_receiver() {
            let token = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let weak = Arc::downgrade(&token);
            let (sender, receiver) = async_channel::bounded(1);
            let fan = spawn_pcm_fan(
                SessionId {
                    handle: 81,
                    generation: 1,
                },
                receiver,
                Arc::new(move |_, _, _| {
                    token.fetch_add(1, Ordering::Relaxed);
                }),
                Arc::new(PushState::default()),
            );
            set_timeout_ms(0).await;
            drop(fan);
            set_timeout_ms(0).await;
            assert!(weak.upgrade().is_none());
            assert!(sender.is_closed());
        }

        #[wasm_bindgen_test(async)]
        #[allow(clippy::arc_with_non_send_sync)] // The core stats cell uses Arc on local WASM.
        async fn explicit_close_during_terminal_stats_suppresses_later_ended() {
            let state = Arc::new(PushState::default());
            let callback_state = state.clone();
            let ended = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let ended_count = ended.clone();
            let sinks = PushSinks {
                event: Arc::new(|_, _| {}),
                stats: Arc::new(move |_, _| {
                    callback_state.close();
                }),
                pcm: Arc::new(|_, _, _| {}),
                encoded: Arc::new(|_, _, _| {}),
                video: Arc::new(|_, _, _| {}),
                ended: Arc::new(move |_, _, _| {
                    ended_count.fetch_add(1, Ordering::Relaxed);
                }),
            };
            let (sender, receiver) = async_channel::bounded(1);
            let _fan = spawn_event_fan(
                SessionId {
                    handle: 82,
                    generation: 1,
                },
                receiver,
                Arc::new(media_stats::MediaStatsCell::default()),
                &sinks,
                state.clone(),
            );
            sender
                .try_send(MediaEvent::Closed(MediaCloseReason::Local))
                .unwrap();
            set_timeout_ms(0).await;
            assert!(state.closed());
            assert_eq!(
                ended.load(Ordering::Relaxed),
                0,
                "no callback may follow the reentrant CLOSE acknowledgement"
            );
        }

        #[wasm_bindgen_test(async)]
        async fn terminal_event_flushes_fresh_stats_before_ended() {
            for previous_tick in [false, true] {
                let id = SessionId {
                    handle: 71,
                    generation: 91,
                };
                #[allow(clippy::arc_with_non_send_sync)]
                let stats = Arc::new(media_stats::MediaStatsCell::default());
                let log = Arc::new(Mutex::new(Vec::new()));
                let (ended_tx, ended_rx) = futures::channel::oneshot::channel();
                let sinks = sinks(log.clone(), ended_tx);
                if previous_tick {
                    (sinks.stats)(
                        id,
                        StatsData {
                            rtp_received: 1,
                            ..StatsData::default()
                        },
                    );
                }
                stats.publish(
                    wacore::voip_control::MediaStats::builder()
                        .rtp_received(7)
                        .audio_frames_delivered(3)
                        .build(),
                );
                let (sender, events) = async_channel::bounded(1);
                let _fan = spawn_event_fan(
                    id,
                    events,
                    stats.clone(),
                    &sinks,
                    Arc::new(PushState::default()),
                );
                sender
                    .send(MediaEvent::Closed(MediaCloseReason::Local))
                    .await
                    .unwrap();
                wait_ended(ended_rx).await;
                let pushes = log.lock().unwrap().clone();
                assert_eq!(pushes.last(), Some(&Push::Ended));
                assert!(
                    matches!(&pushes[pushes.len()-2], Push::Stats(snapshot) if snapshot.rtp_received == 7 && snapshot.audio_frames_delivered == 3)
                );
                // The pushed snapshot owns its values beyond engine teardown.
                stats.publish(wacore::voip_control::MediaStats::default());
                assert_eq!(*log.lock().unwrap(), pushes);
            }
        }

        #[wasm_bindgen_test(async)]
        async fn relay_disconnect_reports_final_counters_without_an_engine_closed_event() {
            let transport = js_sys::Function::new_no_args("return {events:null,connect(_endpoint,events){this.events=events;return Promise.resolve({send(){},reconnect(){},close(){}});}};")
                .call0(&wasm_bindgen::JsValue::UNDEFINED).unwrap();
            relay::set_relay_transport(transport.clone()).unwrap();
            let id = SessionId {
                handle: 72,
                generation: 92,
            };
            let log = Arc::new(Mutex::new(Vec::new()));
            let (ended_tx, ended_rx) = futures::channel::oneshot::channel();
            let live = LiveCall::open(
                id,
                "terminal-stats".to_owned(),
                open_params(),
                sinks(log.clone(), ended_tx),
            )
            .await
            .unwrap();
            let events = js_sys::Reflect::get(&transport, &"events".into()).unwrap();
            let packet: js_sys::Function = js_sys::Reflect::get(&events, &"onPacket".into())
                .unwrap()
                .unchecked_into();
            let burst = u32::try_from(relay::RELAY_EVENT_CAPACITY * 2).unwrap();
            for _ in 0..burst {
                packet
                    .call1(&events, &js_sys::Uint8Array::from(&[0xff][..]))
                    .unwrap();
            }
            for _ in 0..100 {
                if live.stats().relay_packet_unclassified > 0 {
                    break;
                }
                set_timeout_ms(1).await;
            }
            assert!(
                live.stats().relay_packet_unclassified > 0,
                "the real driver counted the injected local packet"
            );
            let close: js_sys::Function = js_sys::Reflect::get(&events, &"onClose".into())
                .unwrap()
                .unchecked_into();
            close
                .call1(&events, &wasm_bindgen::JsValue::UNDEFINED)
                .unwrap();
            wait_ended(ended_rx).await;
            let pushes = log.lock().unwrap().clone();
            assert_eq!(pushes.last(), Some(&Push::Ended));
            assert!(
                matches!(pushes.as_slice(), [.., Push::Stats(snapshot), Push::Ended]
                if snapshot.relay_packet_unclassified > 0 && snapshot.inbound_pipe_dropped > 0
                    && snapshot.relay_packet_unclassified + snapshot.inbound_pipe_dropped == burst)
            );
            drop(live);
            assert_eq!(*log.lock().unwrap(), pushes);
        }
    }

    use wacore::voip::engine::{Input, Output, SequentialTxIds};

    pub(crate) fn open_params() -> OpenParams {
        use voip_abi::{AudioFormatDto, Direction};
        OpenParams {
            direction: Direction::Incoming,
            self_lid: "111111111111111:0@lid".into(),
            peer_lid: "222222222222222:0@lid".into(),
            ssrc: 0x5741_0001,
            audio_io: voip_abi::AudioIo::Pcm,
            audio_format: AudioFormatDto {
                codec: voip_abi::AudioCodecWire::Mlow,
                rtp_profile: voip_abi::RtpProfile::Mlow,
                signaling_rate: 16_000,
                sample_rate: 16_000,
                channels: 1,
                samples_per_frame: 960,
                rtp_clock_rate: 16_000,
                rtp_timestamp_step: 960,
                rtp_payload_type: 120,
            },
            relay_token: voip_abi::SecretBytes::new(vec![0xAB; 16]),
            auth_token: voip_abi::SecretBytes::new(vec![0xCD; 8]),
            call_key: voip_abi::SecretBytes::new((0u8..32).collect()),
            relay_host: "203.0.113.7".into(),
            relay_port: 3478,
            integrity_key: voip_abi::SecretBytes::new(b"relay-key".to_vec()),
            warp_mi_tag_len: 4,
            enable_media: 1,
            enable_video: 0,
            enable_sframe: 0,
            muted: 0,
            video: 0,
            initial_codec: None,
            peer_orientations: Vec::new(),
            group: None,
        }
    }

    fn drain_transmits(engine: &mut CallEngine) -> Vec<Bytes> {
        let mut out = Vec::new();
        loop {
            match engine.poll_output() {
                Output::Transmit(packet) => out.push(packet),
                Output::Timeout(_) => break,
                _ => {}
            }
        }
        out
    }

    #[test]
    fn engine_from_open_params_allocates_and_answers_binding() {
        // The spec mapping the wasm path uses: `OpenParams` back into the
        // neutral spec, then the split into engine parts. A refusal here is
        // the same refusal `BeginOpen` would push as `MEDIA_SETUP_FAILED`.
        let (spec, _ctx) =
            spec_from_params(open_params(), "CID", 7).expect("open params become a spec");
        let parts = wacore::voip_control::engine_bridge::into_engine_parts(spec)
            .expect("the spec splits into engine parts");
        assert_eq!(parts.key.call_id, "CID");
        assert_eq!(parts.key.generation, 7);
        let mut engine =
            CallEngine::new(parts.config, Box::new(SequentialTxIds::new())).expect("engine builds");

        // `start` queues the STUN allocate: the first drain must carry one.
        engine.start(0, 1_700_000_000_000);
        let first = drain_transmits(&mut engine);
        assert!(
            first
                .iter()
                .any(|b| wacore::voip::stun::stun_message_type(b)
                    == Some(wacore::voip::stun::MSG_ALLOCATE_REQUEST)),
            "start must emit the STUN allocate"
        );

        // A relay binding request fed back in is answered with a binding
        // success: the engine is driving the cycle, not just constructed.
        let binding = wacore::voip::stun::encode_stun_request(
            wacore::voip::stun::MSG_BINDING_REQUEST,
            &[9u8; 12],
            &[],
            None,
            false,
        );
        engine.handle_input(1, Input::RelayPacket(&binding));
        let second = drain_transmits(&mut engine);
        assert!(
            second
                .iter()
                .any(|b| wacore::voip::stun::stun_message_type(b)
                    == Some(wacore::voip::stun::MSG_BINDING_SUCCESS)),
            "a binding request must be answered with a binding success"
        );
    }
}
