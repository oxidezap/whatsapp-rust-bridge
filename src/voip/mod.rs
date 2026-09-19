//! Foreign VoIP media backend: the control plane's media seam facing JS.
//!
//! When `createWhatsAppClient` receives `extensions.voipBackend`, the client
//! is built with [`ForeignVoipBackend`] instead of the resident engine. Every
//! media operation then crosses as OZVP frames through two JS callbacks —
//! `sendFrame` for requests, a registered push handler for engine-side
//! pushes — and the engine itself lives behind them (a second WASM the host
//! loads). Without the plugin nothing here is constructed: no second module
//! loads, compiles, or costs memory.
//!
//! The bridge transports. It encodes what the core decided, names what the
//! plugin answered, and maps the answer back into the core's own vocabulary
//! (`MediaSetupError`, `CallEvent`, `MediaStats`) without renaming a field
//! or inventing a default on the way.

pub mod abi;
mod foreign_backend;
mod foreign_session;
mod media_pump;
#[cfg(test)]
mod tests;

pub use foreign_backend::ForeignVoipBackend;
pub use foreign_session::ForeignVoipSession;

use std::sync::Arc;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::errors::{BridgeError, invalid_arg};

/// Capability bits offered at the handshake: everything the pumps below can
/// carry. `RELAY_RECONNECT` stays out: the relay lives engine-side here, so
/// the bridge must not promise reconnect help it never gives.
pub const ABI_CAPABILITIES: u32 = voip_abi::Capabilities::PCM
    | voip_abi::Capabilities::ENCODED_AUDIO
    | voip_abi::Capabilities::VIDEO
    | voip_abi::Capabilities::GROUP_CALLS
    | voip_abi::Capabilities::CALL_LINKS
    | voip_abi::Capabilities::STATS_PUSH;

/// How long `open` waits for the plugin to acknowledge `BEGIN_OPEN` and then
/// for media to come up. Mirrors the resident backend's relay dial ceiling:
/// same value, same `Connect` grammar on expiry.
pub const OPEN_CEILING: std::time::Duration = std::time::Duration::from_secs(20);

/// The JS side of the media seam, extracted once at construction.
///
/// ```ts
/// interface VoipBackendCallbacks {
///   sendFrame(frame: Uint8Array): Promise<Uint8Array>;
///   setPushHandler(handler: (frame: Uint8Array) => void): void;
/// }
/// ```
///
/// Every request gets exactly one response frame; notifications never do.
/// `setPushHandler` receives the bridge's inbound entry point, which the
/// plugin calls for `OPEN`, `EVENT`, `STATS` pushes, `MEDIA_ENDED`, and the
/// `_OUT` media frames.
#[derive(Clone)]
pub struct VoipBackendCallbacks {
    send_frame: js_sys::Function,
    set_push_handler: js_sys::Function,
}

impl VoipBackendCallbacks {
    /// Extracts and validates the two callbacks. Anything misshapen is the
    /// caller's `extensions` argument, not an internal failure.
    pub fn from_js(obj: &JsValue) -> Result<Self, BridgeError> {
        if obj.is_null() || obj.is_undefined() || !obj.is_object() {
            return Err(invalid_arg("extensions", "voipBackend must be an object"));
        }
        let get = |name: &str| {
            js_sys::Reflect::get(obj, &name.into())
                .map_err(|_| invalid_arg("extensions", "voipBackend is unreadable"))
                .and_then(|v| {
                    v.dyn_into::<js_sys::Function>().map_err(|_| {
                        invalid_arg(
                            "extensions",
                            format!("voipBackend.{name} must be a function"),
                        )
                    })
                })
        };
        Ok(VoipBackendCallbacks {
            send_frame: get("sendFrame")?,
            set_push_handler: get("setPushHandler")?,
        })
    }

    /// Hands the inbound entry point to the plugin. Called once, at install.
    pub fn register_push(&self, push: &JsValue) -> Result<(), BridgeError> {
        self.set_push_handler
            .call1(&JsValue::UNDEFINED, push)
            .map_err(|e| {
                invalid_arg(
                    "extensions",
                    format!("voipBackend.setPushHandler threw: {e:?}"),
                )
            })?;
        Ok(())
    }

    /// One request frame across, one response frame back.
    pub async fn send(&self, frame: &[u8]) -> Result<Vec<u8>, BackendCommsError> {
        let bytes = js_sys::Uint8Array::from(frame);
        let ret = self
            .send_frame
            .call1(&JsValue::UNDEFINED, &bytes.into())
            .map_err(|e| BackendCommsError(format!("sendFrame threw: {e:?}")))?;
        if !ret.is_instance_of::<js_sys::Promise>() {
            return Err(BackendCommsError(
                "sendFrame did not return a Promise".to_owned(),
            ));
        }
        let resolved: JsValue = JsFuture::from(js_sys::Promise::unchecked_from_js(ret))
            .await
            .map_err(|e| BackendCommsError(format!("sendFrame rejected: {e:?}")))?;
        let bytes = resolved
            .dyn_into::<js_sys::Uint8Array>()
            .map_err(|_| BackendCommsError("sendFrame resolved to a non-Uint8Array".to_owned()))?;
        Ok(crate::js_bytes::to_vec(&bytes))
    }
}

/// A failed crossing to the plugin: the link broke, not the call. The
/// backend maps these into the setup grammar at each call site.
#[derive(Debug)]
pub struct BackendCommsError(pub String);

/// The trailing `extensions` argument of `createWhatsAppClient`.
pub struct ClientExtensions {
    /// Present only when the host supplied a media plugin.
    pub voip_backend: Option<VoipBackendCallbacks>,
}

impl ClientExtensions {
    /// Parses the optional trailing argument. Absent is absent: no plugin,
    /// no second module, no behavior change.
    pub fn from_js(ext: Option<&JsValue>) -> Result<Self, BridgeError> {
        let Some(obj) = ext else {
            return Ok(ClientExtensions { voip_backend: None });
        };
        if obj.is_null() || obj.is_undefined() {
            return Ok(ClientExtensions { voip_backend: None });
        }
        if !obj.is_object() {
            return Err(invalid_arg("extensions", "extensions must be an object"));
        }
        let backend_val = js_sys::Reflect::get(obj, &"voipBackend".into())
            .map_err(|_| invalid_arg("extensions", "extensions is unreadable"))?;
        let voip_backend = if backend_val.is_null() || backend_val.is_undefined() {
            None
        } else {
            Some(VoipBackendCallbacks::from_js(&backend_val)?)
        };
        Ok(ClientExtensions { voip_backend })
    }
}

/// Version gate before the client exists: the plugin must speak this major,
/// and the behavior enabled is the capability intersection. A mismatch is
/// the caller's plugin version, reported on the argument that carried it.
pub async fn handshake(callbacks: &VoipBackendCallbacks) -> Result<u32, BridgeError> {
    let req = voip_abi::Frame::request(
        voip_abi::Opcode::Hello,
        voip_abi::HelloRequest {
            role: voip_abi::Role::Core,
            capabilities: ABI_CAPABILITIES,
        }
        .encode(),
    );
    let raw = callbacks
        .send(&req.encode())
        .await
        .map_err(|e| invalid_arg("extensions", format!("voipBackend handshake failed: {e:?}")))?;
    let resp = voip_abi::Frame::decode(&raw).map_err(|e| {
        invalid_arg(
            "extensions",
            format!("voipBackend handshake misframed: {e}"),
        )
    })?;
    if resp.major != voip_abi::ABI_MAJOR {
        return Err(invalid_arg(
            "extensions",
            format!(
                "voipBackend speaks ABI major {}, this bridge speaks {}",
                resp.major,
                voip_abi::ABI_MAJOR
            ),
        ));
    }
    if !resp.is_response() || resp.opcode != voip_abi::Opcode::Hello {
        return Err(invalid_arg(
            "extensions",
            format!(
                "voipBackend handshake answered {}, want HELLO",
                resp.opcode.name()
            ),
        ));
    }
    let hello = voip_abi::HelloResponse::decode(&resp.payload).map_err(|e| {
        invalid_arg(
            "extensions",
            format!("voipBackend handshake misformed: {e}"),
        )
    })?;
    Ok(hello.capabilities & ABI_CAPABILITIES)
}

/// Builds the backend, registers its push entry point, and installs it on
/// the client builder. Called during construction, never after.
pub fn install(
    builder: whatsapp_rust::client::ClientBuilder,
    callbacks: VoipBackendCallbacks,
    agreed: u32,
    runtime: Arc<dyn whatsapp_rust::wacore::runtime::Runtime>,
) -> Result<whatsapp_rust::client::ClientBuilder, BridgeError> {
    let backend = ForeignVoipBackend::new(callbacks.clone(), agreed, runtime);
    let push = backend.push_entry();
    callbacks.register_push(&push)?;
    Ok(builder.with_voip_media_backend(backend))
}
