//! Pump tasks: core channels to plugin frames and back.
//!
//! The pumps own no policy. Each moves items between a core channel half
//! and one frame opcode, stopping when its channel closes, the session
//! ends, or the plugin link breaks. One frame in flight at a time bounds
//! the crossing: a slow plugin stalls the pump, and the full channel
//! behind it applies the backpressure the core documents.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use bytes::Bytes;
use voip_abi as abi;
use whatsapp_rust::voip_control as vc;

use super::VoipBackendCallbacks;
use super::foreign_session::CommandEnvelope;

/// Which microphone the session opened with.
pub enum AudioInputs {
    /// 16-bit PCM frames from the core's converter input.
    Pcm(Receiver<Vec<i16>>),
    /// Complete codec payloads from the encoded input.
    Encoded(Receiver<Bytes>),
}

/// Everything one attach fans out into pumps.
pub struct PumpInputs {
    /// The microphone side.
    pub audio: AudioInputs,
    /// Untimed outbound access units.
    pub video_in: Receiver<Vec<u8>>,
    /// Timed outbound access units, when the handle attached one.
    pub timed_in: Option<Receiver<vc::VideoInput>>,
    /// Drive-loop video commands.
    pub control: vc::VideoControlReceiver,
    /// Shared microphone mute flag.
    pub muted: Arc<AtomicBool>,
}

/// The command mailbox halves plus its byte accounting, shared between the
/// session (submit path) and the pumps (control and mute paths).
pub struct CommandPipe {
    /// Enqueue end, shared by submit and the pumps.
    pub tx: Sender<CommandEnvelope>,
    /// Drain end, owned by the command pump.
    pub rx: Receiver<CommandEnvelope>,
    /// Encoded bytes currently queued, for `retained_bytes`.
    pub queued_bytes: Arc<AtomicUsize>,
}

/// How often the mute watcher polls the shared flag. Mute has no wake
/// mechanism across the boundary; 250 ms bounds the skew without waking
/// the event loop for anything else.
const MUTE_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Spawns the pumps for one attach: the command drain always, the
/// microphone and video sources when the handshake agreed them, plus the
/// video-control and mute followers. Returns the handles `close` aborts.
#[allow(clippy::too_many_arguments)]
pub fn spawn_all(
    handle: u32,
    session: abi::SessionId,
    inputs: PumpInputs,
    pipe: CommandPipe,
    callbacks: VoipBackendCallbacks,
    runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
    agreed: u32,
    alive: Arc<AtomicBool>,
) -> Vec<whatsapp_rust::wacore::runtime::AbortHandle> {
    let mut handles = Vec::new();
    let pump = Pump {
        handle,
        session,
        callbacks,
        runtime,
        alive,
    };

    handles.push(pump.spawn_command(pipe.rx, pipe.queued_bytes.clone()));

    match inputs.audio {
        AudioInputs::Pcm(mic) if agreed & abi::Capabilities::PCM != 0 => {
            handles.push(pump.spawn_pcm(mic));
        }
        AudioInputs::Encoded(mic) if agreed & abi::Capabilities::ENCODED_AUDIO != 0 => {
            handles.push(pump.spawn_encoded(mic));
        }
        _ => log::warn!("voip microphone pump ungated by the handshake, not started"),
    }

    if agreed & abi::Capabilities::VIDEO != 0 {
        handles.push(pump.spawn_video(inputs.video_in, inputs.timed_in));
    }
    handles.push(pump.spawn_control(inputs.control, pipe.tx.clone(), pipe.queued_bytes.clone()));
    handles.push(pump.spawn_mute_watcher(inputs.muted, pipe.tx, pipe.queued_bytes));

    handles
}

/// One session's pumps, sharing everything the tasks need.
#[derive(Clone)]
struct Pump {
    handle: u32,
    session: abi::SessionId,
    callbacks: VoipBackendCallbacks,
    runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
    alive: Arc<AtomicBool>,
}

impl Pump {
    fn running(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn spawn_command(
        &self,
        commands: Receiver<CommandEnvelope>,
        queued: Arc<AtomicUsize>,
    ) -> whatsapp_rust::wacore::runtime::AbortHandle {
        let this = self.clone();
        self.runtime.spawn(Box::pin(async move {
            while this.running() {
                let envelope = match commands.recv().await {
                    Ok(envelope) => envelope,
                    Err(_) => break,
                };
                let len = envelope.bytes.len();
                if !this
                    .send(abi::Opcode::Command, envelope.bytes, "command")
                    .await
                {
                    break;
                }
                queued.fetch_sub(len, Ordering::Relaxed);
            }
        }))
    }

    fn spawn_pcm(&self, mic: Receiver<Vec<i16>>) -> whatsapp_rust::wacore::runtime::AbortHandle {
        let this = self.clone();
        self.runtime.spawn(Box::pin(async move {
            let mut seq = 0u32;
            while this.running() {
                let samples = match mic.recv().await {
                    Ok(samples) => samples,
                    Err(_) => break,
                };
                let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                let body = abi::MediaFrame {
                    session: this.session,
                    seq,
                    data,
                }
                .encode();
                seq = seq.wrapping_add(1);
                if !this.send(abi::Opcode::PcmIn, body, "pcm frame").await {
                    break;
                }
            }
        }))
    }

    fn spawn_encoded(&self, mic: Receiver<Bytes>) -> whatsapp_rust::wacore::runtime::AbortHandle {
        let this = self.clone();
        self.runtime.spawn(Box::pin(async move {
            let mut seq = 0u32;
            while this.running() {
                let data = match mic.recv().await {
                    Ok(data) => data,
                    Err(_) => break,
                };
                let body = abi::MediaFrame {
                    session: this.session,
                    seq,
                    data: data.to_vec(),
                }
                .encode();
                seq = seq.wrapping_add(1);
                if !this
                    .send(abi::Opcode::EncodedAudioIn, body, "encoded frame")
                    .await
                {
                    break;
                }
            }
        }))
    }

    fn spawn_video(
        &self,
        untimed: Receiver<Vec<u8>>,
        timed: Option<Receiver<vc::VideoInput>>,
    ) -> whatsapp_rust::wacore::runtime::AbortHandle {
        let this = self.clone();
        self.runtime.spawn(Box::pin(async move {
            let mut seq = 0u32;
            // A timed source carries capture timestamps and the source
            // generation the input queue filters on; the untimed feed has
            // neither, and `None` tells the engine to pace and accept
            // freely.
            if let Some(timed) = timed {
                while this.running() {
                    let unit = match timed.recv().await {
                        Ok(unit) => unit,
                        Err(_) => break,
                    };
                    let body = abi::VideoIn {
                        session: this.session,
                        seq,
                        input: abi::VideoInputDto {
                            data: unit.data,
                            timestamp: Some(unit.timestamp),
                            input_generation: Some(unit.generation),
                        },
                    }
                    .encode();
                    seq = seq.wrapping_add(1);
                    if !this.send(abi::Opcode::VideoIn, body, "video unit").await {
                        break;
                    }
                }
                return;
            }
            while this.running() {
                let data = match untimed.recv().await {
                    Ok(data) => data,
                    Err(_) => break,
                };
                let body = abi::VideoIn {
                    session: this.session,
                    seq,
                    input: abi::VideoInputDto {
                        data,
                        timestamp: None,
                        input_generation: None,
                    },
                }
                .encode();
                seq = seq.wrapping_add(1);
                if !this.send(abi::Opcode::VideoIn, body, "video unit").await {
                    break;
                }
            }
        }))
    }

    fn spawn_control(
        &self,
        control: vc::VideoControlReceiver,
        commands: Sender<CommandEnvelope>,
        queued: Arc<AtomicUsize>,
    ) -> whatsapp_rust::wacore::runtime::AbortHandle {
        let this = self.clone();
        self.runtime.spawn(Box::pin(async move {
            while this.running() {
                let item = match control.recv().await {
                    Ok(item) => item,
                    Err(_) => break,
                };
                let Some(mapped) = super::abi::video_control_to_abi(&item) else {
                    log::warn!("voip video control this bridge predates, dropped");
                    continue;
                };
                let bytes = abi::CommandRequest {
                    session: this.session,
                    command: mapped,
                }
                .encode();
                let len = bytes.len();
                if commands.try_send(CommandEnvelope { bytes }).is_ok() {
                    queued.fetch_add(len, Ordering::Relaxed);
                } else {
                    log::warn!("command mailbox full, video control shed");
                }
            }
        }))
    }

    fn spawn_mute_watcher(
        &self,
        muted: Arc<AtomicBool>,
        commands: Sender<CommandEnvelope>,
        queued: Arc<AtomicUsize>,
    ) -> whatsapp_rust::wacore::runtime::AbortHandle {
        let this = self.clone();
        self.runtime.spawn(Box::pin(async move {
            // The open params carried the initial flag; only changes cross.
            let mut last = muted.load(Ordering::SeqCst);
            while this.running() {
                sleep(&*this.runtime, MUTE_POLL_INTERVAL).await;
                if !this.running() {
                    break;
                }
                let now = muted.load(Ordering::SeqCst);
                if now == last {
                    continue;
                }
                last = now;
                let bytes = abi::CommandRequest {
                    session: this.session,
                    command: abi::MediaCommand::AudioMute(u8::from(now)),
                }
                .encode();
                let len = bytes.len();
                if commands.try_send(CommandEnvelope { bytes }).is_ok() {
                    queued.fetch_add(len, Ordering::Relaxed);
                } else {
                    log::warn!("command mailbox full, mute update shed");
                }
            }
        }))
    }

    /// One frame across with its acknowledgement. `false` ends the pump:
    /// the link broke, the plugin answered wrong, or the session ended.
    async fn send(&self, opcode: abi::Opcode, body: Vec<u8>, what: &str) -> bool {
        if !self.running() {
            return false;
        }
        let raw = match self
            .callbacks
            .send(&abi::Frame::request(opcode, body).encode())
            .await
        {
            Ok(raw) => raw,
            Err(e) => {
                log::warn!(
                    "voip plugin link failed during {what} (handle {}): {}",
                    self.handle,
                    e.0
                );
                return false;
            }
        };
        match abi::Frame::decode(&raw) {
            Ok(resp) if resp.is_response() => true,
            Ok(resp) => {
                log::warn!("voip plugin refused {what} (handle {})", self.handle);
                let _ = resp;
                false
            }
            Err(e) => {
                log::warn!("voip plugin misframed {what} response: {e}");
                false
            }
        }
    }
}

/// Sleeps on the runtime clock: a timeout around a never-ready future.
async fn sleep(runtime: &dyn whatsapp_rust::wacore::runtime::Runtime, duration: Duration) {
    let _ = whatsapp_rust::wacore::runtime::timeout(
        runtime,
        duration,
        futures::future::pending::<()>(),
    )
    .await;
}
