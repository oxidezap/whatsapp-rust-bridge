//! wacore trait adapters: bridge engine-agnostic `Host*` traits to the concrete
//! `wacore` traits the `whatsapp_rust::Client` consumes. All the engine-neutral
//! glue (bounded transport channel + overflow policy, etc.) lives here.

pub mod http;
pub mod transport;

pub use http::CoreHttpClient;
pub use transport::{ChannelSink, CoreTransport, CoreTransportFactory};
