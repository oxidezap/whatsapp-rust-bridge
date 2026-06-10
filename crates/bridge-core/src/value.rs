//! `BridgeValue` — engine-neutral, `Send` value tree used to carry serialized
//! events (and other dynamic payloads) out of the tokio worker thread to the
//! engine binding, which materializes it into a host JS value.
//!
//! It preserves two identities the WASM serializer guaranteed and that
//! `baileyrs` depends on (see `docs/napi-migration/analysis/04-events-serializer.md`):
//!   * `I64`/`U64` → protobufjs-style `{ low, high, unsigned }` Long object in JS
//!     (NEVER `BigInt` — `JSON.stringify(BigInt)` throws and Baileys logs events).
//!   * `Bytes` → `Uint8Array` in JS (NEVER base64 string / `number[]`).
//!
//! `serde_json::Value` cannot express either, which is why this exists.

/// JS `Number.MAX_SAFE_INTEGER`. Integers beyond this lose precision as `f64`;
/// the engine materializer emits them as decimal strings instead.
pub const JS_MAX_SAFE_INT: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeValue {
    Null,
    Bool(bool),
    /// A JS `number` (already known to be safe-range / float).
    F64(f64),
    /// 64-bit signed — materialized as a JS `number`, or a decimal `string`
    /// when out of safe-integer range (the snake/`proto::to_js_value` path).
    I64(i64),
    /// 64-bit unsigned — same number/string materialization as `I64`.
    U64(u64),
    /// protobufjs-style 64-bit — materialized as `{ low, high, unsigned }`
    /// (the camel/`message`/`history_sync` path; baileyrs `toNumber` reads it).
    Long(LongParts),
    Str(String),
    /// Raw bytes — materialized as `Uint8Array` in JS.
    Bytes(Vec<u8>),
    Array(Vec<BridgeValue>),
    /// Insertion-ordered object (key order is part of the contract for some
    /// consumers; preserve it). camelCase/snake_case is decided by the
    /// serializer that produced the tree, not here.
    Object(Vec<(String, BridgeValue)>),
}

/// protobufjs `Long` split: `{ low: i32, high: i32, unsigned: bool }`.
/// `low`/`high` are the two 32-bit halves as JS `number`s (signed int32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LongParts {
    pub low: i32,
    pub high: i32,
    pub unsigned: bool,
}

impl LongParts {
    pub fn from_i64(v: i64) -> Self {
        Self {
            low: v as i32,
            high: (v >> 32) as i32,
            unsigned: false,
        }
    }

    pub fn from_u64(v: u64) -> Self {
        Self {
            low: v as i32,
            high: (v >> 32) as i32,
            unsigned: true,
        }
    }

    /// A zero Long (used by protobufjs default-skipping).
    pub fn is_zero(&self) -> bool {
        self.low == 0 && self.high == 0
    }
}

impl BridgeValue {
    pub fn object(fields: impl IntoIterator<Item = (String, BridgeValue)>) -> Self {
        BridgeValue::Object(fields.into_iter().collect())
    }

    pub fn str(s: impl Into<String>) -> Self {
        BridgeValue::Str(s.into())
    }
}
