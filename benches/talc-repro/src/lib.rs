//! Two talc bugs that broke this bridge in production, as runnable tests.
//!
//! Both live here rather than in the bridge's own suite because showing a fix
//! means running the same bodies against two versions of talc, and the bridge
//! pins one. Switch arms with `cargo update -p talc --precise 5.0.3`.
//!
//! Run: `wasm-pack test --node benches/talc-repro`

#![cfg(all(target_family = "wasm", not(target_feature = "atomics")))]

pub mod repro;
pub mod stress;
pub mod upstream;
