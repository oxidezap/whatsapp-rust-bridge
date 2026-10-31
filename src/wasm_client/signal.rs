//! Calls, Signal protocol and raw transport.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

impl WasmWhatsAppClient {
    fn execute_set_raw_node_forwarding(&self, enabled: bool) {
        let mut lease = self
            .raw_node_lease
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if enabled {
            if lease.is_none() {
                *lease = Some(
                    self.client
                        .unwaited(Unwaited::ThisSocket)
                        .acquire_raw_node_forwarding(),
                );
            }
        } else {
            lease.take();
        }
    }
}

include!("generated/signal.rs");
