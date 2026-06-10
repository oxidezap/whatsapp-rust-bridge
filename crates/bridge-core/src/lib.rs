//! Engine-agnostic core for the WhatsApp bridge.
//!
//! Holds ALL business logic behind `Host*` abstraction traits so the engine
//! binding (napi today, wasm in the future) is a thin shell. Nothing here may
//! depend on `napi` or `wasm-bindgen`.
//!
//! See `docs/napi-migration/DESIGN.md`.

pub mod adapters;
pub mod backend;
pub mod cache;
pub mod client;
pub mod client_profile;
pub mod device_props;
pub mod errors;
pub mod events;
pub mod helpers;
pub mod host;
pub mod m_groups;
pub mod m_identity;
pub mod m_media;
pub mod m_messaging;
pub mod m_newsletter_signal_media;
pub mod m_profile;
pub mod methods;
pub mod result_types;
pub mod runtime;
pub mod serializer;
pub mod value;

pub use host::{
    HostCacheStore, HostEventSink, HostHttp, HostStore, HostStoreCapabilities, HostTransport,
    HostTransportFactory, HostTransportSink,
};
pub use runtime::TokioRuntime;
pub use value::BridgeValue;
