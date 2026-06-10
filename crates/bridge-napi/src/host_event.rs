//! `HostEventSink` over the JS `onEvent(event)` callback. Fire-and-forget,
//! non-blocking, ordered (single TSFN libuv FIFO). The `BridgeValue` is
//! materialized to JS (`JsBridgeValue: ToNapiValue`) on the JS thread.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

use bridge_core::host::HostEventSink;
use bridge_core::value::BridgeValue;

use crate::value_js::JsBridgeValue;

pub type EventTsfn = ThreadsafeFunction<JsBridgeValue, (), JsBridgeValue, Status, false>;

pub struct NapiEventSink {
    pub tsfn: Arc<EventTsfn>,
}

impl HostEventSink for NapiEventSink {
    fn emit(&self, event: BridgeValue) {
        self.tsfn.call(
            JsBridgeValue(event),
            ThreadsafeFunctionCallMode::NonBlocking,
        );
    }
}
