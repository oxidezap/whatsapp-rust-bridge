//! Async runtime. On native we reuse whatsapp-rust's tokio-backed `Runtime`
//! impl — the custom single-threaded WASM executor (`src/runtime.rs` in the old
//! crate) is obsolete here. Time/crypto providers also fall through to wacore's
//! native defaults (`ChronoTimeProvider`/`StdMonotonicProvider`/`RustCryptoProvider`),
//! so there is nothing to install.

pub use whatsapp_rust::TokioRuntime;
