//! Cross-boundary error type for the WASM bridge.
//!
//! `BridgeError` is the single shape that all errors take when they cross the
//! WASM→JS boundary. It is built by walking the `std::error::Error::source()`
//! chain of whatever the core returned and downcasting to the leaf types whose
//! structured fields (e.g. `IqError::ServerError { code, text }`) we want to
//! preserve in JS.
//!
//! On the JS side the result is a `js_sys::Error` named `WhatsAppError` with
//! the discriminant on `.kind` and per-variant fields set as own properties —
//! consumers narrow via `e.kind === 'server'` and read `e.serverCode`, etc.
//! See `From<BridgeError> for JsValue` and `to_js_error` for the conversion.

use core::error::Error;

use serde::Serialize;
use thiserror::Error;
use tsify::Tsify;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

use whatsapp_rust::client::ClientError;
use whatsapp_rust::features::{
    AppStateError, BlockingError, BusinessError, ChatStateError, CommunityError, ContactError,
    GroupError, MediaReuploadError, NewsletterError, PollError, PresenceError, ProfileError,
    RetryRequestError, SignalError, StanzaResponseError,
};
use whatsapp_rust::handshake::HandshakeError;
use whatsapp_rust::pair_code::{PairCodeError, PairError};
use whatsapp_rust::request::IqError;
use whatsapp_rust::socket::error::SocketError;
use whatsapp_rust::{
    CallError, ConnectError, MexError, SendError, SignalMaintenanceError, wacore, wacore_binary,
};

/// Public error shape that crosses the WASM→JS boundary.
///
/// Variants are intentionally flat — no `#[from]` on enum variants, no
/// `#[serde(flatten)]`. Translation from the core's typed errors happens in
/// `From` impls below by walking the source chain. This keeps the JS object
/// shape predictable and the codegen / `Tsify` output simple.
// `Tsify` is used here only for `.d.ts` generation — we deliberately do NOT
// add `#[tsify(into_wasm_abi)]` because that auto-generates a
// `From<BridgeError> for JsValue` that would produce a plain object instead
// of a real `js_sys::Error`. The custom `From` impl below builds an `Error`
// (so `instanceof Error` works in JS) and copies the serde-payload's fields
// onto it as own properties.
#[derive(Debug, Error, Serialize, Tsify)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BridgeError {
    /// The WhatsApp server returned a structured `<error code="..." text="..."/>`
    /// stanza in response to an IQ. `code`/`text` are the server-supplied
    /// fields; consumers usually branch on `code` (400/401/404/406/...).
    #[error("server error {server_code}: {server_text}")]
    Server {
        #[serde(rename = "serverCode")]
        server_code: u16,
        #[serde(rename = "serverText")]
        server_text: String,
    },

    /// The IQ request did not get a response within the timeout window.
    #[error("request timed out")]
    Timeout,

    /// No active socket / not authenticated. Caller must reconnect / log in
    /// before retrying.
    #[error("not connected")]
    NotConnected,

    /// Server pushed a `<stream:error>` / disconnect node mid-flight. The raw
    /// reason text (when available) is on `reason`.
    #[error("disconnected by server: {reason}")]
    Disconnected { reason: String },

    /// Caller passed something the API can statically reject — empty phone
    /// number, malformed JID, custom pair code outside the Crockford alphabet,
    /// etc. `field` names which input was rejected; `reason` describes why.
    #[error("invalid argument {field}: {reason}")]
    InvalidArgument { field: String, reason: String },

    /// The remote peer sent something that violates the protocol — e.g. an
    /// invalid public key during pairing, an unexpected stanza shape, missing
    /// required attributes. Different from `Server` in that the *server*
    /// (or peer) sent something we couldn't parse, vs. returning a typed
    /// error stanza.
    #[error("protocol violation: {reason}")]
    ProtocolViolation { reason: String },

    /// A cryptographic operation failed — key parse, DH agreement, AEAD
    /// encrypt/decrypt, HKDF expand. `operation` describes which step.
    #[error("crypto operation failed: {operation}")]
    Crypto { operation: String },

    /// Persistence layer failure — JS-backed callbacks errored, serde failed,
    /// SQLite was busy/locked, etc.
    #[error("storage operation failed: {operation}")]
    Storage { operation: String },

    /// Catch-all for cases not yet mapped. The full `Display` chain is in
    /// `message` so JS-side debugging is still possible.
    #[error("internal: {message}")]
    Internal { message: String },
}

impl BridgeError {
    /// Walk a borrowed `&dyn Error` chain looking for known leaf types whose
    /// structured fields we expose on the JS surface. The walk has two
    /// priority tiers:
    ///
    /// 1. **High-priority leaves** (return immediately): typed errors whose
    ///    discriminant is the most informative thing the JS side can get —
    ///    `IqError::ServerError { code, text }`, `PairCodeError` validation
    ///    variants, JS-backed storage callback errors. If a parent like
    ///    `IqError::Socket(_)` is purely a wrapper, `iq_to_bridge` returns
    ///    `None` and the loop continues down `source()` to find the real
    ///    leaf below.
    ///
    /// 2. **What the core says about the whole chain**: `ErrorChainExt` is
    ///    the core's own answer to "did this run out of time?" and "was the
    ///    transport gone?", across every type that can carry the fact. A
    ///    variant with no `source()` — `ClientError::NotConnected` — stops
    ///    the loop above dead, and asking closes that class for the types
    ///    the core teaches rather than one downcast at a time.
    ///
    /// 3. **Generic wrappers** (remembered as fallback): `StoreError` is
    ///    useful when nothing more specific is found below it, but a JS
    ///    callback or JSON-deserialize source nested inside it is even
    ///    more actionable. We capture the first wrapper hit and only
    ///    surface it if the deeper walk turns up nothing typed.
    ///
    /// Falls back to `Internal { message }` carrying the full chain
    /// `Display` if no tier matched.
    pub fn from_error_chain(err: &(dyn Error + 'static)) -> Self {
        // Scoped to this body: at module scope the trait's `as_dyn_error`
        // collides with the one thiserror's `#[source]` expansion resolves.
        use whatsapp_rust::ErrorChainExt;

        let mut storage_fallback: Option<BridgeError> = None;
        let mut cur: Option<&(dyn Error + 'static)> = Some(err);

        while let Some(c) = cur {
            // ── High-priority leaves: return on hit ──────────────────────
            if let Some(iq) = c.downcast_ref::<IqError>() {
                if let Some(b) = iq_to_bridge(iq) {
                    return b;
                }
                // wrapper variant — keep walking via .source()
            } else if let Some(iq) = c.downcast_ref::<wacore::request::IqError>() {
                if let Some(b) = wacore_iq_to_bridge(iq) {
                    return b;
                }
            } else if let Some(server) = c.downcast_ref::<wacore::request::ServerErrorCode>() {
                // The carrier the core uses to move a server rejection across a
                // crate boundary inside `anyhow`. Reachable since whatsapp-rust
                // #1100 exposed the head of an `anyhow` chain as `source()`.
                return BridgeError::Server {
                    server_code: server.code,
                    server_text: server.text.clone(),
                };
            } else if let Some(pc) = c.downcast_ref::<PairCodeError>() {
                return paircode_to_bridge(pc);
            } else if let Some(jc) = c.downcast_ref::<crate::js_backend::JsCallbackError>() {
                return BridgeError::Storage {
                    operation: jc.to_string(),
                };
            } else if let Some(js) = c.downcast_ref::<crate::js_backend::JsonStoreError>() {
                return BridgeError::Storage {
                    operation: js.to_string(),
                };
            } else if let Some(sock) = c.downcast_ref::<SocketError>() {
                if let Some(b) = socket_to_bridge(sock) {
                    return b;
                }
            } else if let Some(hs) = c.downcast_ref::<HandshakeError>() {
                if let Some(b) = handshake_to_bridge(hs) {
                    return b;
                }
            }
            // ── Generic wrapper: remember first hit, keep walking ────────
            else if storage_fallback.is_none()
                && let Some(se) = c.downcast_ref::<wacore::store::error::StoreError>()
            {
                storage_fallback = Some(BridgeError::Storage {
                    operation: se.to_string(),
                });
            }

            cur = c.source();
        }

        // ── Ask the core, rather than recognising discriminants ─────────
        // Runs after the walk so a leaf carrying more detail keeps it: an
        // `IqError::Disconnected` is transport-unavailable too, and reaches
        // `Disconnected { reason }` above instead of collapsing to here.
        if err.is_timeout() {
            return BridgeError::Timeout;
        }
        if err.is_transport_unavailable() {
            return BridgeError::NotConnected;
        }

        if let Some(b) = storage_fallback {
            return b;
        }
        BridgeError::Internal {
            message: display_chain(err),
        }
    }
}

/// The core spells "the caller's own request was rejected before it went out"
/// as a per-domain `InvalidRequest(String)`-shaped variant carrying no source,
/// and `ErrorChainExt` deliberately models no such question — the domains share
/// no representation of it, so it would be invented there rather than
/// recovered. That leaves each one to be named here, on the #29 precedent:
/// the core names no argument and the arguments differ per call, so the detail
/// is the reason and `field` stays the request.
fn invalid_request(detail: impl core::fmt::Display) -> BridgeError {
    invalid_arg("request", detail.to_string())
}

/// The two IQ types spell the same condition, so the wording lives once.
fn unexpected_response(got: Option<&str>) -> BridgeError {
    protocol_violation(match got {
        Some(kind) => format!("unexpected IQ response type: {kind}"),
        None => "IQ response carried no type attribute".into(),
    })
}

/// The stanza is the caller's own argument to `acknowledgeStanza`,
/// `rejectStanza` and `requestMessageRetry`, and the bridge cannot prove the
/// node it was handed is the one the wire delivered — so a missing attribute is
/// reported against that argument, the same as `UnsupportedStanzaClass` on the
/// same parameter.
fn missing_attribute(attr: &str) -> BridgeError {
    invalid_arg("stanza", format!("missing the '{attr}' attribute"))
}

/// Match a high-level `IqError` against variants whose fields are themselves
/// the user-actionable signal. Wrapper variants (`Socket`, `EncryptSend`,
/// `ClientState`, `EncodeError`, `ParseError`) return `None` so the chain
/// walker keeps descending into `.source()` — there may be a typed leaf
/// below worth surfacing.
fn iq_to_bridge(e: &IqError) -> Option<BridgeError> {
    Some(match e {
        // `..` ignores the `error_type`/`backoff` fields added in whatsapp-rust #747;
        // BridgeError::Server surfaces only code/text. Surface them here if ever needed.
        IqError::ServerError { code, text, .. } => BridgeError::Server {
            server_code: *code,
            server_text: text.clone(),
        },
        IqError::Timeout => BridgeError::Timeout,
        IqError::NotConnected => BridgeError::NotConnected,
        IqError::Disconnected(node) => BridgeError::Disconnected {
            reason: format!("{node:?}"),
        },
        IqError::UnexpectedResponseType { got } => unexpected_response(got.as_deref()),
        // `InternalChannelClosed` names the mechanism, not what the host should
        // do: the request pipeline is gone and a reconnect is what fixes it,
        // which is why the core files it under `is_transport_unavailable`. Left
        // to the chain walk's core question so that judgement is read, not
        // restated.
        IqError::InternalChannelClosed => return None,
        // Wrappers — let the walker drill into `.source()`.
        IqError::Socket(_)
        | IqError::EncryptSend(_)
        | IqError::ClientState(_)
        | IqError::EncodeError(_)
        | IqError::ParseError(_) => return None,
        // `IqError` is #[non_exhaustive] upstream (whatsapp-rust #735): an unknown
        // future variant has no actionable mapping here, so defer to the chain
        // walk — it falls back to `Internal` carrying the full Display chain.
        _ => return None,
    })
}

/// Neither of the two leaves below is an entry point — both are only ever
/// reached through `.source()`, under an `IqError` or a `ConnectError` — and
/// neither is covered by the core's transport answer, so their source-less
/// variants would otherwise end a walk on `Internal` mid-reconnect.
fn socket_to_bridge(e: &SocketError) -> Option<BridgeError> {
    Some(match e {
        SocketError::SocketClosed => BridgeError::NotConnected,
        SocketError::Cipher(_) => BridgeError::Crypto {
            operation: "noise cipher".into(),
        },
        _ => return None,
    })
}

fn handshake_to_bridge(e: &HandshakeError) -> Option<BridgeError> {
    Some(match e {
        HandshakeError::Disconnected | HandshakeError::StreamClosed => BridgeError::NotConnected,
        HandshakeError::UnexpectedEvent(detail) => BridgeError::ProtocolViolation {
            reason: format!("during handshake: {detail}"),
        },
        // `Timeout` is the core's own answer, asked at the end of the walk.
        _ => return None,
    })
}

fn wacore_iq_to_bridge(e: &wacore::request::IqError) -> Option<BridgeError> {
    Some(match e {
        wacore::request::IqError::ServerError { code, text, .. } => BridgeError::Server {
            server_code: *code,
            server_text: text.clone(),
        },
        wacore::request::IqError::Timeout => BridgeError::Timeout,
        wacore::request::IqError::NotConnected => BridgeError::NotConnected,
        wacore::request::IqError::Disconnected(node) => BridgeError::Disconnected {
            reason: format!("{node:?}"),
        },
        wacore::request::IqError::UnexpectedResponseType { got } => {
            unexpected_response(got.as_deref())
        }
        // Transport gone, same as its high-level twin above.
        wacore::request::IqError::InternalChannelClosed => return None,
        // `wacore::request::IqError` is #[non_exhaustive] upstream (whatsapp-rust
        // #735): defer unknown future variants to the chain walk (Internal fallback).
        _ => return None,
    })
}

fn paircode_to_bridge(e: &PairCodeError) -> BridgeError {
    use BridgeError::*;
    match e {
        PairCodeError::PhoneNumberRequired => InvalidArgument {
            field: "phoneNumber".into(),
            reason: "required".into(),
        },
        PairCodeError::PhoneNumberTooShort => InvalidArgument {
            field: "phoneNumber".into(),
            reason: "too short (min 7 digits)".into(),
        },
        PairCodeError::PhoneNumberNotInternational => InvalidArgument {
            field: "phoneNumber".into(),
            reason: "must not start with 0 (use international format)".into(),
        },
        PairCodeError::InvalidCustomCode => InvalidArgument {
            field: "customCode".into(),
            reason: "must be 8 chars from Crockford Base32 alphabet".into(),
        },
        PairCodeError::InvalidWrappedData { expected, got } => ProtocolViolation {
            reason: format!("wrapped data: expected {expected} bytes, got {got}"),
        },
        PairCodeError::InvalidPrimaryEphemeralKey(_) => Crypto {
            operation: "parse primary ephemeral key".into(),
        },
        PairCodeError::InvalidPrimaryIdentityKey(_) => Crypto {
            operation: "parse primary identity key".into(),
        },
        PairCodeError::EphemeralKeyAgreement(_) => Crypto {
            operation: "ephemeral DH agreement".into(),
        },
        PairCodeError::IdentityKeyAgreement(_) => Crypto {
            operation: "identity DH agreement".into(),
        },
        PairCodeError::AdvSecretKeyDerivation => Crypto {
            operation: "HKDF expand for adv_secret".into(),
        },
        PairCodeError::BundleKeyDerivation => Crypto {
            operation: "HKDF expand for bundle encryption key".into(),
        },
        PairCodeError::BundleAead(_) => Crypto {
            operation: "AES-GCM encrypt key bundle".into(),
        },
        PairCodeError::NotWaiting => ProtocolViolation {
            reason: "not in waiting state for pair-code notification".into(),
        },
        PairCodeError::MissingPairingRef => ProtocolViolation {
            reason: "server response missing pairing ref".into(),
        },
        // `#[non_exhaustive]` upstream (whatsapp-rust #1100). A variant added
        // later carries no classification we can honestly claim, so it keeps
        // its own text instead of being filed under a kind by guesswork.
        _ => Internal {
            message: e.to_string(),
        },
    }
}

/// Render the full `Display` chain of an error, separated by `: `, collapsing
/// neighbours that render the same text. Since whatsapp-rust #1100 a wrapping
/// variant renders exactly what it wraps, so joining every node would repeat
/// the same sentence once per layer.
fn display_chain(err: &(dyn Error + 'static)) -> String {
    use core::fmt::Write;
    const SEPARATOR: &str = ": ";

    let mut out = String::new();
    let _ = write!(out, "{err}");
    // Each node is rendered straight into the buffer and compared in place
    // against the previous one, so a duplicate costs a truncate instead of a
    // per-node String.
    let mut previous = 0..out.len();
    let mut cur = err.source();
    while let Some(c) = cur {
        let appended_at = out.len();
        let _ = write!(out, "{SEPARATOR}{c}");
        let segment = appended_at + SEPARATOR.len()..out.len();
        if out[segment.clone()] == out[previous.clone()] {
            out.truncate(appended_at);
        } else {
            previous = segment;
        }
        cur = c.source();
    }
    out
}

// ---------------------------------------------------------------------------
// From impls — typed entry points that walk the source chain.
//
// We can't blanket `impl<E: Error + 'static> From<E> for BridgeError` because
// it conflicts with the blanket `From<T> for T`. So we list the entry points
// the wasm boundary actually returns. New error types added at call sites
// only need their own `From` here — the chain walking generalizes.

impl From<PairError> for BridgeError {
    fn from(e: PairError) -> Self {
        // `PairError` does not expose its inner `PairCodeError` via `source()`,
        // so the generic chain walk in `from_error_chain` would miss it and fall
        // back to `Internal`. Route the validation variant directly so callers
        // get the actionable `InvalidArgument`/`ProtocolViolation`/… mapping.
        if let PairError::PairCode(pc) = &e {
            return paircode_to_bridge(pc);
        }
        Self::from_error_chain(&e)
    }
}

impl From<IqError> for BridgeError {
    fn from(e: IqError) -> Self {
        Self::from_error_chain(&e)
    }
}

impl From<PairCodeError> for BridgeError {
    fn from(e: PairCodeError) -> Self {
        paircode_to_bridge(&e)
    }
}

/// Bulk of the core's high-level methods return `anyhow::Error`. Walk the
/// chain — `anyhow::Error` derefs to `dyn Error + Send + Sync + 'static`
/// so the same downcast machinery works.
impl From<anyhow::Error> for BridgeError {
    fn from(e: anyhow::Error) -> Self {
        Self::from_error_chain(e.as_ref())
    }
}

/// Lower-level wacore error wrappers we hit at the boundary occasionally
/// (e.g. `parse_jid`, IQ encoding, binary parsing). Mapped to specific
/// variants where the discriminant is useful; otherwise falls through to
/// `Internal` via the chain walk.
impl From<wacore_binary::jid::JidError> for BridgeError {
    fn from(e: wacore_binary::jid::JidError) -> Self {
        BridgeError::InvalidArgument {
            field: "jid".into(),
            reason: e.to_string(),
        }
    }
}

/// `serde_wasm_bindgen` errors: deserialization of JS arguments failed.
/// Always a caller bug — wrong shape supplied. `InvalidArgument` is the
/// right kind so consumers can surface "you passed bad input".
impl From<serde_wasm_bindgen::Error> for BridgeError {
    fn from(e: serde_wasm_bindgen::Error) -> Self {
        BridgeError::InvalidArgument {
            field: "input".into(),
            reason: e.to_string(),
        }
    }
}

/// Catch-all for raw `JsValue` errors crossing back from JS callbacks
/// (transport, http, store) — those don't have Rust-side type info, only
/// whatever `Display`/`Debug` representation JS gave us.
#[cfg(target_arch = "wasm32")]
impl From<JsValue> for BridgeError {
    fn from(e: JsValue) -> Self {
        let message = e.as_string().unwrap_or_else(|| format!("{e:?}"));
        BridgeError::Internal { message }
    }
}

/// One entry per core error type: the source-less variants of that type, and
/// the kind each has to reach. A variant carrying no `source()` stops the walk
/// dead, so anything not listed here that the walk cannot classify lands on
/// `Internal` — which tells the host the bridge broke.
///
/// Everything else goes to `from_error_chain`: it finds the typed leaf, then
/// asks the core whether the transport was gone or time ran out.
macro_rules! classify {
    ($( $error:ty { $( $variant:pat => $kind:expr ),* $(,)? } )+) => {
        $(
            impl From<$error> for BridgeError {
                fn from(e: $error) -> Self {
                    $( if let $variant = &e { return $kind; } )*
                    Self::from_error_chain(&e)
                }
            }
        )+
    };
}

classify! {
    // Every variant carries a source; the walk and the core's answers cover it.
    ChatStateError {}

    // `Timeout` needs no arm: the core reports it, including the handshake
    // timeout nested under `Handshake` that this list could not reach. The two
    // below are the caller reaching for a client that cannot connect — already
    // up, or shut down — so they name the operation the way `connect()` already
    // does. `NotActivated` is not among them: construction never completing is
    // an invariant no argument of ours reaches, so it keeps `internal`.
    ConnectError {
        ConnectError::AlreadyConnected => invalid_arg("connect", "client is already connected"),
        ConnectError::Shutdown => invalid_arg("connect", "client has been shut down; build a new one"),
    }

    // `CorruptKey` is the one variant a retry cannot fix. A drain commit that
    // failed is the store refusing the batch; a drain cut short by shutdown
    // leaves nothing for the host to fix except a live client.
    SignalMaintenanceError {
        SignalMaintenanceError::CorruptKey(detail) => protocol_violation(detail.clone()),
        SignalMaintenanceError::DrainCommitFailed => BridgeError::Storage {
            operation: "commit inbound drain batch".into(),
        },
        SignalMaintenanceError::DrainShuttingDown => BridgeError::NotConnected,
    }

    // `NotConnected` needs no arm — the core calls it transport unavailable and
    // the walk asks. `NotLoggedIn` sits outside that answer (the socket may be
    // up), but `not-connected` covers it by contract.
    ClientError {
        ClientError::NotLoggedIn => BridgeError::NotConnected,
    }

    // `sendPresence` is the only call that reaches this, and its own argument
    // is fine — so the field names the operation, and the reason names the call
    // that repairs the state.
    PresenceError {
        PresenceError::PushNameEmpty =>
            invalid_arg("sendPresence", "push name is not set; call setPushName first"),
    }

    // `DescriptionConflict` is the core's typed form of `<error code="409"
    // text="conflict"/>` on a description update, mapped back to the server
    // error it stands for.
    GroupError {
        GroupError::DescriptionConflict => BridgeError::Server {
            server_code: 409,
            server_text: "conflict".into(),
        },
        GroupError::InvalidRequest(detail) => invalid_request(detail),
    }

    AppStateError {
        AppStateError::InvalidRequest(detail) => invalid_request(detail),
    }

    // `InvalidUpdate` is the caller's own delta failing the core's pre-flight.
    // It does carry a source, but `BusinessProfileUpdateError` is a leaf the
    // walk does not know, so without this arm it lands on `Internal` too. A
    // malformed response is the server's doing, not the caller's.
    BusinessError {
        BusinessError::InvalidUpdate(detail) => invalid_arg("update", detail.to_string()),
        BusinessError::MalformedResponse { operation, detail } =>
            protocol_violation(format!("malformed {operation} response: {detail}")),
    }

    CallError {
        CallError::EmptyCallId => invalid_arg("callId", "required"),
    }

    SendError {
        SendError::NotLoggedIn => BridgeError::NotConnected,
        SendError::InvalidRequest(detail) => invalid_request(detail),
    }

    BlockingError {
        BlockingError::InvalidJid(detail) => invalid_arg("jid", detail),
    }

    CommunityError {
        CommunityError::InvalidRequest(detail) => invalid_request(detail),
    }

    ContactError {
        ContactError::InvalidJid(detail) => invalid_arg("jid", detail),
    }

    // `Timeout` here is the media retry *notification* never arriving, which
    // the core models on this type rather than as an `IqError`, so its own
    // timeout answer does not report it.
    MediaReuploadError {
        MediaReuploadError::NotLoggedIn => BridgeError::NotConnected,
        MediaReuploadError::Timeout => BridgeError::Timeout,
        MediaReuploadError::InvalidRequest(detail) => invalid_request(detail),
    }

    NewsletterError {
        NewsletterError::InvalidRequest(detail) => invalid_request(detail),
    }

    PollError {
        PollError::NotLoggedIn => BridgeError::NotConnected,
        // The core rejects `options`, `selectableCount` or `correctIndex`
        // through this one variant and names which only in its text, so it
        // joins the rest of the unnameable rejections rather than inventing a
        // `poll` argument no method takes.
        PollError::InvalidPoll(detail) => invalid_request(detail),
    }

    ProfileError {
        ProfileError::InvalidArgument(detail) => invalid_request(detail),
    }

    // The two stanza surfaces split the same three ways: a missing attribute is
    // the *received* stanza being unusable, so it is the wire's fault; an
    // unsupported class is the caller handing over the wrong stanza; and a
    // missing local identity means the device has no own-JID yet, which
    // `not-connected` already covers.
    RetryRequestError {
        RetryRequestError::MissingAttribute(attr) => missing_attribute(attr),
        RetryRequestError::UnsupportedStanzaClass =>
            invalid_arg("stanza", "retry requests require a message stanza"),
        RetryRequestError::MissingLocalIdentity => BridgeError::NotConnected,
    }

    StanzaResponseError {
        StanzaResponseError::MissingAttribute(attr) => missing_attribute(attr),
        StanzaResponseError::UnsupportedStanzaClass =>
            invalid_arg("stanza", "the stanza class does not support this response"),
        StanzaResponseError::MissingLocalIdentity => BridgeError::NotConnected,
    }

    // `Unsupported` is one variant over two conditions — an `msgType` that
    // belongs to another method, and an encrypt-path invariant — and only the
    // detail string tells them apart. It gets no arm here: the two enter
    // through different bridge methods, so `signalDecryptMessage` names the
    // argument itself and everything else keeps `internal`.
    SignalError {
        SignalError::InvalidInput(detail) => invalid_request(detail),
    }

    // `PayloadParsing` is the server's own answer failing to parse.
    //
    // `ExtensionError` is left on the `Internal` fallback on purpose: it is a
    // GraphQL-layer rejection whose `code` comes from a different space than the
    // IQ `code` attribute — the core refuses to merge the two in
    // `ErrorChainExt::server_rejection`, and putting it on `serverCode` here
    // would undo that. No current kind stands for it.
    MexError {
        MexError::PayloadParsing(detail) => protocol_violation(format!("MEX payload: {detail}")),
    }
}

// Deliberate non-impl: `From<serde_json::Error>`. The right `kind` depends on
// where the JSON came from — caller-supplied input is `InvalidArgument`,
// remote/CDN responses are `ProtocolViolation`, internal serialization is
// `Internal`. A blanket impl would mis-classify CDN parse failures as caller
// bugs (see whatsapp-rust-bridge POC review). Use `BridgeError::invalid_arg`
// / `protocol_violation` / `internal` helpers (or build the variant inline)
// at each call site instead.

/// Slice → fixed-size-array conversion failures (e.g. media keys, hashes).
/// Indicates malformed/wrong-sized binary input — caller's responsibility.
impl From<core::array::TryFromSliceError> for BridgeError {
    fn from(e: core::array::TryFromSliceError) -> Self {
        BridgeError::InvalidArgument {
            field: "binary".into(),
            reason: e.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// JsValue conversion: build a real `js_sys::Error` (so `instanceof Error`
// works in JS) and copy the serde payload's fields onto it as own properties.
// JS-side, consumers see:
//   const e = ...;
//   e instanceof Error            // true
//   e.name === 'WhatsAppError'    // true
//   e.kind === 'server'           // narrows via discriminant
//   e.serverCode, e.serverText    // (camelCased by serde rename_all)

#[cfg(target_arch = "wasm32")]
impl From<BridgeError> for JsValue {
    fn from(e: BridgeError) -> JsValue {
        to_js_error(&e)
    }
}

/// Build an `Internal` variant from any `Display`able message. Convenience
/// wrapper used by call sites that previously returned `JsError::new(...)`
/// for ad-hoc messages with no typed source (e.g. argument validation that
/// the bridge does itself, before reaching the core).
pub fn internal<S: Into<String>>(message: S) -> BridgeError {
    BridgeError::Internal {
        message: message.into(),
    }
}

/// Build an `InvalidArgument` variant. Use at boundary sites where the
/// bridge rejects caller-supplied input *before* it reaches the core
/// (malformed JSON arguments, missing fields the JS layer should have
/// supplied).
pub fn invalid_arg<F: Into<String>, R: Into<String>>(field: F, reason: R) -> BridgeError {
    BridgeError::InvalidArgument {
        field: field.into(),
        reason: reason.into(),
    }
}

/// Build a `ProtocolViolation` variant. Use when a *remote* peer (server,
/// CDN, paired device) sent something we couldn't parse — distinct from
/// `Server` (which is a typed `<error>` stanza from WhatsApp) and from
/// `InvalidArgument` (which is the *caller's* fault, not the wire's).
pub fn protocol_violation<R: Into<String>>(reason: R) -> BridgeError {
    BridgeError::ProtocolViolation {
        reason: reason.into(),
    }
}

/// Construct a JS `Error` carrying the `BridgeError` payload. Takes `&` so
/// `Display` (via `e.to_string()`) and `serde::Serialize` (via
/// `serde_wasm_bindgen::to_value`) can both run without consuming.
#[cfg(target_arch = "wasm32")]
pub fn to_js_error(e: &BridgeError) -> JsValue {
    use js_sys::{Error as JsError, Object, Reflect};

    let err = JsError::new(&e.to_string());
    err.set_name("WhatsAppError");

    if let Ok(payload) = serde_wasm_bindgen::to_value(e)
        && let Some(obj) = payload.dyn_ref::<Object>()
    {
        let keys = Object::keys(obj);
        for key in keys.iter() {
            if let Ok(value) = Reflect::get(obj, &key) {
                let _ = Reflect::set(&err, &key, &value);
            }
        }
    }

    err.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Alias `#[test]` -> wasm_bindgen_test so these run on wasm32 via
    // `wasm-pack test --node` (the crate has no native test target).
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// The rejection stanza `IqError::ServerError` carries since whatsapp-rust
    /// #1257. The mapping reads only `code`/`text`, so any well-formed `<iq>`
    /// stands in for the one the server would have sent.
    fn rejection_stanza() -> whatsapp_rust::RejectionStanza {
        use whatsapp_rust::wacore_binary::node::{Attrs, Node, OwnedNodeRef};

        let packed =
            whatsapp_rust::wacore_binary::marshal::marshal(&Node::new("iq", Attrs::new(), None))
                .expect("the stanza marshals");
        let node_bytes = whatsapp_rust::wacore_binary::util::unpack(&packed)
            .expect("marshal output unpacks")
            .into_owned();
        std::sync::Arc::new(OwnedNodeRef::new(node_bytes).expect("the node bytes decode")).into()
    }

    #[test]
    fn iq_server_error_extracts_code_and_text() {
        let iq = IqError::ServerError {
            code: 400,
            text: "bad-request".into(),
            error_type: None,
            backoff: None,
            response: rejection_stanza(),
        };
        let be: BridgeError = iq.into();
        match be {
            BridgeError::Server {
                server_code,
                server_text,
            } => {
                assert_eq!(server_code, 400);
                assert_eq!(server_text, "bad-request");
            }
            other => panic!("expected Server, got {other:?}"),
        }
    }

    #[test]
    fn pair_error_request_failed_walks_to_iq_server_error() {
        let iq = IqError::ServerError {
            code: 400,
            text: "bad-request".into(),
            error_type: None,
            backoff: None,
            response: rejection_stanza(),
        };
        let pe: PairError = PairError::RequestFailed(iq);
        let be: BridgeError = pe.into();
        match be {
            BridgeError::Server {
                server_code,
                server_text,
            } => {
                assert_eq!(server_code, 400);
                assert_eq!(server_text, "bad-request");
            }
            other => panic!("expected Server via chain walk, got {other:?}"),
        }
    }

    #[test]
    fn group_description_conflict_keeps_its_server_code() {
        let be: BridgeError = whatsapp_rust::features::GroupError::DescriptionConflict.into();
        match be {
            BridgeError::Server {
                server_code,
                server_text,
            } => {
                assert_eq!(server_code, 409);
                assert_eq!(server_text, "conflict");
            }
            other => panic!("expected Server, got {other:?}"),
        }
    }

    fn rejected(code: u16) -> IqError {
        IqError::ServerError {
            code,
            text: "forbidden".into(),
            error_type: None,
            backoff: None,
            response: rejection_stanza(),
        }
    }

    #[test]
    fn other_group_errors_still_walk_the_source_chain() {
        let be: BridgeError = whatsapp_rust::features::GroupError::Iq(rejected(403)).into();
        match be {
            BridgeError::Server { server_code, .. } => assert_eq!(server_code, 403),
            other => panic!("expected Server via chain walk, got {other:?}"),
        }
    }

    /// Since whatsapp-rust #1100 the chain is lossless for every domain, not
    /// just the one whose symptom was reported. A consumer that branches on
    /// `kind === 'server'` gets the same answer wherever the call came from.
    #[test]
    fn every_domain_surfaces_its_server_code() {
        // Every domain the bridge actually returns; `TcTokenError` is absent
        // because no boundary method exposes it.
        use whatsapp_rust::features::{
            BlockingError, CommunityError, ContactError, GroupError, NewsletterError, ProfileError,
        };

        let domains: Vec<BridgeError> = vec![
            GroupError::Iq(rejected(403)).into(),
            CommunityError::Iq(rejected(403)).into(),
            ContactError::Iq(rejected(403)).into(),
            ProfileError::Iq(rejected(403)).into(),
            BlockingError::Iq(rejected(403)).into(),
            NewsletterError::Iq(rejected(403)).into(),
            // Nested two domains deep.
            CommunityError::Group(GroupError::Iq(rejected(403))).into(),
        ];

        for be in domains {
            match be {
                BridgeError::Server { server_code, .. } => assert_eq!(server_code, 403),
                other => panic!("expected Server, got {other:?}"),
            }
        }
    }

    /// The core moves a rejection across a crate boundary as `ServerErrorCode`
    /// inside `anyhow`. That head only became reachable once the chain stopped
    /// erasing it, so classifying it is what keeps it out of `internal`.
    #[test]
    fn server_error_code_carried_in_anyhow_is_a_server_error() {
        let carrier = wacore::request::ServerErrorCode {
            code: 409,
            text: "conflict".into(),
            error_type: None,
            backoff: None,
        };
        let be: BridgeError =
            whatsapp_rust::features::GroupError::Internal(anyhow::Error::new(carrier)).into();
        match be {
            BridgeError::Server {
                server_code,
                server_text,
            } => {
                assert_eq!(server_code, 409);
                assert_eq!(server_text, "conflict");
            }
            other => panic!("expected Server, got {other:?}"),
        }
    }

    /// A wrapping variant renders exactly what it wraps, so a naive join would
    /// repeat the same sentence once per layer.
    #[test]
    fn display_chain_collapses_repeated_neighbours() {
        use whatsapp_rust::features::{CommunityError, GroupError};

        let err = CommunityError::Group(GroupError::Iq(rejected(403)));
        let rendered = display_chain(&err);
        let inner = rejected(403).to_string();

        assert_eq!(rendered, inner, "three identical layers collapse into one");
        assert_eq!(rendered.matches("received a server error").count(), 1);
    }

    /// Distinct texts still accumulate: collapsing must not swallow a layer
    /// that adds information.
    ///
    /// The outer layer is declared here rather than taken from the core. The
    /// core renders a wrapping variant as what it wraps, so every wrapper it
    /// offers is a collapse case, and pinning this to whichever one still
    /// carried its own sentence made the test fail when that sentence was
    /// dropped — a rendering change in an unrelated crate, not a regression in
    /// this function.
    #[test]
    fn display_chain_keeps_distinct_layers() {
        #[derive(Debug, thiserror::Error)]
        #[error("outer layer")]
        struct Outer(#[source] IqError);

        let err = Outer(rejected(400));
        let rendered = display_chain(&err);

        assert!(rendered.starts_with("outer layer"));
        assert!(rendered.contains("received a server error"));
    }

    #[test]
    fn pair_error_paircode_validation_walks_to_invalid_argument() {
        let pc = PairCodeError::PhoneNumberTooShort;
        let pe: PairError = PairError::PairCode(pc);
        let be: BridgeError = pe.into();
        match be {
            BridgeError::InvalidArgument { field, .. } => {
                assert_eq!(field, "phoneNumber");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn iq_timeout_maps_to_timeout_kind() {
        let be: BridgeError = IqError::Timeout.into();
        assert!(matches!(be, BridgeError::Timeout));
    }

    #[test]
    fn iq_not_connected_maps_to_not_connected_kind() {
        let be: BridgeError = IqError::NotConnected.into();
        assert!(matches!(be, BridgeError::NotConnected));
    }

    /// The reported failure. A send that lands in a disconnect window fails as
    /// `ClientError::NotConnected` instead of `IqError::NotConnected`, and the
    /// core files both under `is_transport_unavailable`. Sits next to the
    /// `IqError` test above: same condition, and until this passes the two
    /// answer differently.
    #[test]
    fn client_not_connected_maps_to_not_connected_kind() {
        let be: BridgeError = whatsapp_rust::client::ClientError::NotConnected.into();
        assert!(matches!(be, BridgeError::NotConnected), "got {be:?}");
    }

    /// Classifying a source-less variant must not shadow the walk: an error
    /// that wraps a typed leaf still has to reach the leaf.
    #[test]
    fn client_error_wrapping_a_typed_leaf_still_reaches_it() {
        let be: BridgeError = ClientError::Iq(rejected(403)).into();
        match be {
            BridgeError::Server { server_code, .. } => assert_eq!(server_code, 403),
            other => panic!("expected Server via chain walk, got {other:?}"),
        }

        // Two hops, through a variant that is itself transport-unavailable-shaped.
        let be: BridgeError = ClientError::Iq(IqError::ClientState(Box::new(ClientError::Iq(
            rejected(401),
        ))))
        .into();
        match be {
            BridgeError::Server { server_code, .. } => assert_eq!(server_code, 401),
            other => panic!("expected Server via chain walk, got {other:?}"),
        }
    }

    /// The fallback has to survive the sweep: an error nothing classifies is
    /// still `internal`, so "transient" does not become the default answer.
    #[test]
    fn an_error_no_one_classifies_still_lands_in_internal() {
        #[derive(Debug, thiserror::Error)]
        #[error("a failure this bridge has never seen")]
        struct Foreign;

        let be = BridgeError::from_error_chain(&Foreign);
        match be {
            BridgeError::Internal { message } => {
                assert_eq!(message, "a failure this bridge has never seen");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    /// The payload as JS reads it, so the sweep below checks the contract
    /// rather than a restatement of the arms under test.
    fn payload_of(e: &BridgeError) -> serde_json::Value {
        serde_json::to_value(e).expect("BridgeError serializes")
    }

    /// Every source-less variant of every core error the bridge maps, with the
    /// kind it has to reach.
    ///
    /// A variant carrying no `source()` gives the chain walk nothing to find,
    /// so it lands in `internal` unless something classifies it — the defect
    /// this table exists to catch, and the reason a variant the core adds
    /// later belongs here. `internal` is an answer only for a failure the host
    /// genuinely cannot act on, and each such row says why.
    #[test]
    fn source_less_variants_reach_an_actionable_kind() {
        let cases: Vec<(&str, BridgeError, &str)> = vec![
            // The core calls all three transport-gone.
            (
                "ClientError::NotConnected",
                ClientError::NotConnected.into(),
                "not-connected",
            ),
            (
                "ClientError::NotLoggedIn",
                ClientError::NotLoggedIn.into(),
                "not-connected",
            ),
            (
                "IqError::InternalChannelClosed",
                IqError::InternalChannelClosed.into(),
                "not-connected",
            ),
            (
                "wacore::IqError::InternalChannelClosed",
                BridgeError::from_error_chain(&wacore::request::IqError::InternalChannelClosed),
                "not-connected",
            ),
            (
                "SocketError::SocketClosed",
                BridgeError::from_error_chain(&IqError::Socket(SocketError::SocketClosed)),
                "not-connected",
            ),
            (
                "HandshakeError::Disconnected",
                ConnectError::Handshake(HandshakeError::Disconnected).into(),
                "not-connected",
            ),
            (
                "HandshakeError::StreamClosed",
                ConnectError::Handshake(HandshakeError::StreamClosed).into(),
                "not-connected",
            ),
            (
                "HandshakeError::Timeout",
                ConnectError::Handshake(HandshakeError::Timeout).into(),
                "timeout",
            ),
            (
                "HandshakeError::UnexpectedEvent",
                ConnectError::Handshake(HandshakeError::UnexpectedEvent("stream end".into()))
                    .into(),
                "protocol-violation",
            ),
            (
                "ConnectError::AlreadyConnected",
                ConnectError::AlreadyConnected.into(),
                "invalid-argument:connect",
            ),
            (
                // Construction never completing is an invariant no argument
                // of ours reaches; `internal` is what that is.
                "ConnectError::NotActivated",
                ConnectError::NotActivated.into(),
                "internal",
            ),
            (
                "ConnectError::Shutdown",
                ConnectError::Shutdown.into(),
                "invalid-argument:connect",
            ),
            (
                "ConnectError::Timeout",
                ConnectError::Timeout {
                    stage: whatsapp_rust::client::ConnectStage::Socket,
                    timeout: core::time::Duration::from_secs(20),
                }
                .into(),
                "timeout",
            ),
            (
                "SignalMaintenanceError::CorruptKey",
                SignalMaintenanceError::CorruptKey("bad length".into()).into(),
                "protocol-violation",
            ),
            (
                "SignalMaintenanceError::DrainCommitFailed",
                SignalMaintenanceError::DrainCommitFailed.into(),
                "storage",
            ),
            (
                "SignalMaintenanceError::DrainShuttingDown",
                SignalMaintenanceError::DrainShuttingDown.into(),
                "not-connected",
            ),
            (
                "IqError::UnexpectedResponseType",
                IqError::UnexpectedResponseType {
                    got: Some("get".into()),
                }
                .into(),
                "protocol-violation",
            ),
            (
                "wacore::IqError::UnexpectedResponseType",
                BridgeError::from_error_chain(&wacore::request::IqError::UnexpectedResponseType {
                    got: Some("get".into()),
                }),
                "protocol-violation",
            ),
            // Our own request-id bookkeeping collided: the host supplies no id
            // and can do nothing about it. `internal` is the honest answer.
            (
                "IqError::DuplicateRequestId",
                IqError::DuplicateRequestId("42".into()).into(),
                "internal",
            ),
            (
                "PresenceError::PushNameEmpty",
                PresenceError::PushNameEmpty.into(),
                "invalid-argument:sendPresence",
            ),
            (
                "GroupError::InvalidRequest",
                GroupError::InvalidRequest("no participants".into()).into(),
                "invalid-argument:request",
            ),
            (
                "GroupError::DescriptionConflict",
                GroupError::DescriptionConflict.into(),
                "server",
            ),
            (
                "AppStateError::InvalidRequest",
                AppStateError::InvalidRequest("mute timestamp is in the past".into()).into(),
                "invalid-argument:request",
            ),
            (
                "BusinessError::InvalidUpdate",
                BusinessError::InvalidUpdate(
                    whatsapp_rust::BusinessProfileUpdateError::TooManyWebsites { count: 9 },
                )
                .into(),
                "invalid-argument:update",
            ),
            (
                "BusinessError::MalformedResponse",
                BusinessError::MalformedResponse {
                    operation: "profile",
                    detail: "missing hours".into(),
                }
                .into(),
                "protocol-violation",
            ),
            (
                "CallError::EmptyCallId",
                CallError::EmptyCallId.into(),
                "invalid-argument:callId",
            ),
            (
                "SendError::NotLoggedIn",
                SendError::NotLoggedIn.into(),
                "not-connected",
            ),
            (
                "SendError::InvalidRequest",
                SendError::InvalidRequest("empty recipient list".into()).into(),
                "invalid-argument:request",
            ),
            (
                "SendError::NoRecipientDevice",
                SendError::NoRecipientDevice(
                    whatsapp_rust::wacore::send::NoRecipientDeviceError::Unresolved,
                )
                .into(),
                "no-recipient-device",
            ),
            (
                "BlockingError::InvalidJid",
                BlockingError::InvalidJid("g.us".into()).into(),
                "invalid-argument:jid",
            ),
            (
                "CommunityError::InvalidRequest",
                CommunityError::InvalidRequest("not a community".into()).into(),
                "invalid-argument:request",
            ),
            (
                "ContactError::InvalidJid",
                ContactError::InvalidJid("broadcast".into()).into(),
                "invalid-argument:jid",
            ),
            (
                "MediaReuploadError::NotLoggedIn",
                MediaReuploadError::NotLoggedIn.into(),
                "not-connected",
            ),
            (
                "MediaReuploadError::InvalidRequest",
                MediaReuploadError::InvalidRequest("no media key".into()).into(),
                "invalid-argument:request",
            ),
            (
                "MediaReuploadError::Timeout",
                MediaReuploadError::Timeout.into(),
                "timeout",
            ),
            (
                "NewsletterError::InvalidRequest",
                NewsletterError::InvalidRequest("not a newsletter jid".into()).into(),
                "invalid-argument:request",
            ),
            (
                "PollError::InvalidPoll",
                PollError::InvalidPoll("fewer than two options".into()).into(),
                "invalid-argument:request",
            ),
            (
                "PollError::NotLoggedIn",
                PollError::NotLoggedIn.into(),
                "not-connected",
            ),
            (
                "ProfileError::InvalidArgument",
                ProfileError::InvalidArgument("picture is not JPEG".into()).into(),
                "invalid-argument:request",
            ),
            (
                "RetryRequestError::UnsupportedStanzaClass",
                RetryRequestError::UnsupportedStanzaClass.into(),
                "invalid-argument:stanza",
            ),
            (
                "RetryRequestError::MissingAttribute",
                RetryRequestError::MissingAttribute("from").into(),
                "invalid-argument:stanza",
            ),
            (
                "RetryRequestError::MissingLocalIdentity",
                RetryRequestError::MissingLocalIdentity.into(),
                "not-connected",
            ),
            (
                // Only `signalDecryptMessage` can name the argument behind
                // this one, and it does so itself; reached any other way it is
                // an invariant the caller cannot act on.
                "SignalError::Unsupported",
                SignalError::Unsupported("unexpected ciphertext variant".into()).into(),
                "internal",
            ),
            (
                "SignalError::InvalidInput",
                SignalError::InvalidInput("key is 31 bytes".into()).into(),
                "invalid-argument:request",
            ),
            (
                "StanzaResponseError::MissingAttribute",
                StanzaResponseError::MissingAttribute("id").into(),
                "invalid-argument:stanza",
            ),
            (
                "StanzaResponseError::MissingLocalIdentity",
                StanzaResponseError::MissingLocalIdentity.into(),
                "not-connected",
            ),
            (
                "StanzaResponseError::UnsupportedStanzaClass",
                StanzaResponseError::UnsupportedStanzaClass.into(),
                "invalid-argument:stanza",
            ),
            (
                "MexError::PayloadParsing",
                MexError::PayloadParsing("missing data".into()).into(),
                "protocol-violation",
            ),
            // A GraphQL-layer rejection. `code` lives in a different space from
            // the IQ `code` attribute — the core refuses to merge the two, and
            // so does `server` here — and no kind stands for it. Left where it
            // is until the surface grows one.
            (
                "MexError::ExtensionError",
                MexError::ExtensionError {
                    code: 1675247,
                    message: "not authorized".into(),
                }
                .into(),
                "internal",
            ),
        ];

        let mut wrong = Vec::new();
        for (name, bridge, expected) in cases {
            // An expectation pins the `field` too where the kind carries one:
            // "invalid-argument:stanza". `field` is as much of the contract as
            // the kind — the same argument has to answer with the same name
            // wherever it can be wrong.
            let (want_kind, want_field) = match expected.split_once(':') {
                Some((kind, field)) => (kind, Some(field)),
                None => (expected, None),
            };
            let payload = payload_of(&bridge);
            let got_kind = payload["kind"].as_str().unwrap_or("<not a string>");
            if got_kind != want_kind {
                wrong.push(format!(
                    "  {name}: expected {want_kind:?}, got {got_kind:?}"
                ));
                continue;
            }
            if let Some(want_field) = want_field {
                let got_field = payload["field"].as_str().unwrap_or("<absent>");
                if got_field != want_field {
                    wrong.push(format!(
                        "  {name}: expected field {want_field:?}, got {got_field:?}"
                    ));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "{} source-less variant(s) misclassified:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }

    #[test]
    fn unknown_iq_variant_lands_in_internal_with_full_chain() {
        // EncodeError carries an anyhow source; we want display_chain to walk it.
        let inner = anyhow::anyhow!("encoder blew up");
        let iq = IqError::EncodeError(inner);
        let be: BridgeError = iq.into();
        match be {
            BridgeError::Internal { message } => {
                assert!(message.contains("failed to encode"));
                assert!(message.contains("encoder blew up"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn contextualized_storage_error_keeps_the_callback_reason() {
        let store_error = wacore::store::error::StoreError::Database(Box::new(
            crate::js_backend::JsCallbackError {
                context: "setMany",
                message: "session projection failed".into(),
            },
        ));
        let error = anyhow::Error::new(store_error).context("Failed to flush signal cache");
        let bridge: BridgeError = error.into();

        match bridge {
            BridgeError::Storage { operation } => {
                assert_eq!(operation, "JS setMany: session projection failed");
            }
            other => panic!("expected Storage, got {other:?}"),
        }
    }
}
