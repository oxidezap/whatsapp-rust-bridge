//! Pump tasks: core channels to plugin frames and back.
//!
//! The pumps own no policy. Each moves items between a core channel half
//! and one frame opcode, stopping when its channel closes, the session
//! ends, or the plugin link breaks. One frame in flight at a time bounds
//! the crossing: a slow plugin stalls the pump, and the full channel
//! behind it applies the backpressure the core documents.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use bytes::Bytes;
use futures::StreamExt;
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
    pub enable_media: bool,
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
    pub initial_muted: bool,
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
    failures: Sender<vc::MediaEvent>,
    task_handles: Arc<Mutex<Vec<whatsapp_rust::wacore::runtime::AbortHandle>>>,
) -> Vec<whatsapp_rust::wacore::runtime::AbortHandle> {
    let mut handles = Vec::new();
    let pump = Pump {
        handle,
        session,
        callbacks,
        runtime,
        alive,
        failures,
        muted: inputs.muted.clone(),
        initial_muted: inputs.initial_muted,
        task_handles,
    };

    handles.push(pump.spawn_command(pipe.rx, pipe.queued_bytes.clone()));

    match inputs.audio {
        AudioInputs::Pcm(mic) if inputs.enable_media && agreed & abi::Capabilities::PCM != 0 => {
            handles.push(pump.spawn_pcm(mic));
        }
        AudioInputs::Encoded(mic)
            if inputs.enable_media && agreed & abi::Capabilities::ENCODED_AUDIO != 0 =>
        {
            handles.push(pump.spawn_encoded(mic));
        }
        _ => log::warn!("voip microphone pump ungated by the handshake, not started"),
    }

    if agreed & abi::Capabilities::VIDEO != 0 {
        if inputs.enable_media {
            handles.push(pump.spawn_video(inputs.video_in, inputs.timed_in));
        }
        handles.push(pump.spawn_control(
            inputs.control,
            pipe.tx.clone(),
            pipe.queued_bytes.clone(),
        ));
    }
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
    failures: Sender<vc::MediaEvent>,
    muted: Arc<AtomicBool>,
    initial_muted: bool,
    task_handles: Arc<Mutex<Vec<whatsapp_rust::wacore::runtime::AbortHandle>>>,
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
                    .send(
                        abi::Opcode::Command,
                        envelope.bytes.as_bytes().to_vec(),
                        "command",
                    )
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
                if this.muted.load(Ordering::SeqCst) {
                    continue;
                }
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
                if this.muted.load(Ordering::SeqCst) {
                    continue;
                }
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
            // Both queues may exist simultaneously. Round-robin selection keeps
            // a ready producer from starving the other, and drains the surviving
            // queue when either sender closes. Dropping this task cancels both
            // pending receives and the single in-flight crossing.
            let untimed = untimed.map(|data| abi::VideoInputDto {
                data,
                timestamp: None,
                input_generation: None,
            });
            let timed = futures::stream::iter(timed)
                .flatten()
                .map(|unit| abi::VideoInputDto {
                    data: unit.data,
                    timestamp: Some(unit.timestamp),
                    input_generation: Some(unit.generation),
                });
            let inputs = futures::stream::select(untimed, timed);
            futures::pin_mut!(inputs);
            while this.running() {
                let Some(input) = inputs.next().await else {
                    break;
                };
                let body = abi::VideoIn {
                    session: this.session,
                    seq,
                    input,
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
                if commands
                    .send(CommandEnvelope {
                        bytes: abi::SecretBytes::new(bytes),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                queued.fetch_add(len, Ordering::Relaxed);
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
            let mut last = this.initial_muted;
            while this.running() {
                sleep(&*this.runtime, MUTE_POLL_INTERVAL).await;
                if !this.running() {
                    break;
                }
                let now = muted.load(Ordering::SeqCst);
                if now == last {
                    continue;
                }
                let bytes = abi::CommandRequest {
                    session: this.session,
                    command: abi::MediaCommand::AudioMute(u8::from(now)),
                }
                .encode();
                let len = bytes.len();
                if commands
                    .try_send(CommandEnvelope {
                        bytes: abi::SecretBytes::new(bytes),
                    })
                    .is_ok()
                {
                    queued.fetch_add(len, Ordering::Relaxed);
                    last = now;
                } else {
                    log::warn!("command mailbox full, mute update deferred");
                }
            }
        }))
    }

    /// One frame across with its acknowledgement. `false` ends the pump:
    /// the link broke, the plugin answered wrong, or the session ended.
    async fn send(&self, opcode: abi::Opcode, body: Vec<u8>, what: &str) -> bool {
        // Bound the whole operation, including host promises and BUSY retries.
        let success = whatsapp_rust::wacore::runtime::timeout(
            &*self.runtime,
            super::OPEN_CEILING,
            self.send_checked(opcode, body, what),
        )
        .await
        .unwrap_or(false);
        if !success && self.alive.swap(false, Ordering::SeqCst) {
            let _ = self.failures.force_send(vc::MediaEvent::MediaSetupFailed(
                "voip plugin link failed".to_owned(),
            ));
            self.failures.close();
            // Wake every blocked source/control pump, even without a core event consumer.
            for task in self.task_handles.lock().unwrap().drain(..) {
                task.abort();
            }
        }
        success
    }

    async fn send_checked(&self, opcode: abi::Opcode, body: Vec<u8>, what: &str) -> bool {
        if !self.running() {
            return false;
        }
        let request = abi::Frame::request(opcode, body);
        let encoded = abi::SecretBytes::new(request.encode());
        while self.running() {
            let raw = match self.callbacks.send(encoded.as_bytes()).await {
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
                Ok(resp)
                    if resp.major == abi::ABI_MAJOR
                        && resp.opcode == opcode
                        && resp.is_error()
                        && opcode == abi::Opcode::Command
                        && abi::ErrorBody::decode(&resp.payload)
                            .is_ok_and(|error| error.code == abi::AbiErrorCode::Busy) =>
                {
                    // BUSY can persist throughout engine setup. A real timer bounds
                    // retries and lets cancellation/deadline timers make progress.
                    sleep(&*self.runtime, Duration::from_millis(1)).await;
                }
                Ok(resp)
                    if resp.major == abi::ABI_MAJOR
                        && resp.opcode == opcode
                        && resp.is_response()
                        && !resp.is_error() =>
                {
                    return true;
                }
                Ok(resp)
                    if resp.major == abi::ABI_MAJOR
                        && resp.opcode == opcode
                        && resp.is_error()
                        && opcode == abi::Opcode::Command
                        && abi::ErrorBody::decode(&resp.payload).is_ok() =>
                {
                    // An engine refusal rejects this command, not the established link.
                    log::warn!(
                        "voip plugin refused command {what} (handle {})",
                        self.handle
                    );
                    return true;
                }
                Ok(resp) => {
                    log::warn!("voip plugin refused {what} (handle {})", self.handle);
                    let _ = resp;
                    return false;
                }
                Err(e) => {
                    log::warn!("voip plugin misframed {what} response: {e}");
                    return false;
                }
            }
        }
        false
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

#[cfg(test)]
mod failure_tests {
    use super::*;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn callbacks(refuse: bool) -> VoipBackendCallbacks {
        let send = wasm_bindgen::closure::Closure::wrap(Box::new(move |value: JsValue| {
            if !refuse {
                return js_sys::Promise::reject(&JsValue::from_str("fixture disconnected"));
            }
            let bytes = js_sys::Uint8Array::new(&value).to_vec();
            let request = abi::Frame::decode(&bytes).unwrap();
            let response = if request.opcode == abi::Opcode::Command {
                abi::Frame::fail(
                    &request,
                    abi::AbiErrorCode::NotSupported,
                    Some("fixture refused"),
                )
            } else {
                abi::Frame::respond(&request, Vec::new())
            };
            let response: JsValue = js_sys::Uint8Array::from(response.encode().as_slice()).into();
            js_sys::Promise::resolve(&response)
        })
            as Box<dyn FnMut(JsValue) -> js_sys::Promise>);
        let object = js_sys::Object::new();
        js_sys::Reflect::set(&object, &"sendFrame".into(), &send.into_js_value()).unwrap();
        js_sys::Reflect::set(
            &object,
            &"setPushHandler".into(),
            &js_sys::Function::new_no_args(""),
        )
        .unwrap();
        VoipBackendCallbacks::from_js(&object.into()).unwrap()
    }

    #[allow(clippy::arc_with_non_send_sync)] // Match the production session's shared task registry.
    fn pump(refuse: bool, failures: Sender<vc::MediaEvent>) -> Pump {
        Pump {
            handle: 1,
            session: abi::SessionId {
                handle: 1,
                generation: 1,
            },
            callbacks: callbacks(refuse),
            runtime: Arc::new(crate::runtime::WasmRuntime),
            alive: Arc::new(AtomicBool::new(true)),
            failures,
            muted: Arc::new(AtomicBool::new(false)),
            initial_muted: false,
            task_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[wasm_bindgen_test]
    async fn typed_command_refusal_preserves_a_healthy_media_link() {
        let (events, rx) = async_channel::bounded(1);
        let pump = pump(true, events);
        assert!(
            pump.send(abi::Opcode::Command, vec![], "fixture command")
                .await
        );
        assert!(pump.running());
        assert!(
            pump.send(abi::Opcode::PcmIn, vec![], "next media frame")
                .await
        );
        assert!(rx.is_empty());
    }

    #[wasm_bindgen_test]
    async fn failed_link_replaces_full_diagnostics_and_aborts_blocked_sources() {
        let (events, rx) = async_channel::bounded(1);
        events
            .try_send(vc::MediaEvent::MediaSetupFailed("old diagnostic".into()))
            .unwrap();
        let pump = pump(false, events);
        let (input, input_rx) = async_channel::bounded(1);
        let blocked = pump.spawn_encoded(input_rx);
        pump.task_handles.lock().unwrap().push(blocked);
        assert!(
            !pump
                .send(abi::Opcode::Command, vec![], "fixture failure")
                .await
        );
        assert!(
            matches!(rx.recv().await.unwrap(), vc::MediaEvent::MediaSetupFailed(detail) if detail == "voip plugin link failed")
        );
        assert!(pump.task_handles.lock().unwrap().is_empty());
        // Poll the cancelled task so its owned receiver is released.
        sleep(&*pump.runtime, Duration::from_millis(1)).await;
        assert!(input.is_closed());
    }
}
