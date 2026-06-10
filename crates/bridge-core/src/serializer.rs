//! Engine-agnostic serde `Serializer` that produces a [`BridgeValue`] tree.
//!
//! This is the host-neutral port of the old `src/camel_serializer.rs` (which
//! had TWO impls — a `JsValue` one and a `serde_json::Value` twin) plus the
//! `src/proto.rs` `to_js_value` snake-case path. The napi (or future wasm)
//! binding turns the resulting [`BridgeValue`] into a host JS value, preserving
//! the identities the old WASM serializer guaranteed.
//!
//! The 64-bit representation split is encoded in [`BridgeValue`] itself (see
//! `value.rs`), and the two modes pick different variants:
//!   * Camel (`message`/`history_sync`) 64-bit → [`BridgeValue::Long`] →
//!     protobufjs-style `{ low, high, unsigned }` object (NEVER `BigInt` —
//!     `JSON.stringify(BigInt)` throws and consumers log events). Ports the old
//!     `i64_to_long`/`u64_to_long`; the 32-bit low/high split via [`LongParts`]
//!     is pure, exact arithmetic, so it is fine to do here (value.rs expects it).
//!   * Snake (`proto::to_js_value`) 64-bit → [`BridgeValue::I64`]/[`BridgeValue::U64`]
//!     → materialized as a JS `number`, or a decimal `string` when out of
//!     `Number.MAX_SAFE_INTEGER` range (the binding decides — matching
//!     `proto::to_js_value`'s in-range-number / out-of-range-string fallback).
//!     We carry the raw value; we do NOT stringify here.
//!   * `Bytes` → `Uint8Array` (NEVER base64 string / `number[]`).
//!
//! ## Two modes, one serializer
//!
//! * [`to_bridge_camel`] — `snake_case` → `camelCase` field rename + protobufjs
//!   default-skipping. For proto / message / `history_sync` payloads. Ports the
//!   old `CamelSerializer`/`to_js_value_camel` (and its JSON twin
//!   `JsonCamelSerializer`/`to_json_value_camel`, which had identical rename +
//!   skip logic — they only differed in output type, now unified to
//!   `BridgeValue`).
//! * [`to_bridge_snake`] — keys verbatim, NO skipping. For the `serialize {}`
//!   event variants + `info`. Ports `src/proto.rs::to_js_value`
//!   (serde_wasm_bindgen with `serialize_maps_as_objects(true)` and
//!   `serialize_large_number_types_as_bigints(false)`), which renamed nothing
//!   and skipped nothing. NOTE: the old JSON twin *did* skip — snake mode must
//!   NOT copy that; its reference is `to_js_value`, not the JSON twin.
//!
//! ## Bytes handling differs by mode
//!
//! `serialize_bytes` → [`BridgeValue::Bytes`] in BOTH modes (camel emitted a
//! `Uint8Array`; serde_wasm_bindgen's default — snake's reference — also emits a
//! `Uint8Array`). The *all-`u8` sequence detector* (a `Vec<u8>` serialized as a
//! seq of `u8` via serde's default `Serialize`) collapses to `Bytes` ONLY in
//! camel mode — that detector lived solely in `CamelSerializer`/
//! `JsonCamelSerializer`. In snake mode, serde_wasm_bindgen leaves such a seq as
//! a plain number array, so we do too.

use crate::value::{BridgeValue, LongParts};
use serde::ser::{self, Serialize};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Serialization error. Ports the old `camel_serializer::Error` /
/// `proto::to_js_value`'s string error — a thin wrapper around a message.
#[derive(Debug)]
pub struct BridgeSerError(String);

impl BridgeSerError {
    pub fn into_message(self) -> String {
        self.0
    }
}

impl std::fmt::Display for BridgeSerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BridgeSerError {}

impl ser::Error for BridgeSerError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        BridgeSerError(msg.to_string())
    }
}

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

/// Controls field-key casing, protobufjs default-skipping, and the all-`u8`
/// sequence → `Bytes` collapse. Threaded through every sub-serializer so a
/// nested struct under a map/seq keeps the same behavior.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `snake_case` → `camelCase` + skip proto defaults + all-`u8` seq → Bytes.
    Camel,
    /// Keys verbatim, skip nothing, all-`u8` seq stays a number array.
    Snake,
}

impl Mode {
    /// Transform a struct field key for this mode. Camel renames via
    /// [`to_camel_case`]; snake keeps it verbatim.
    fn key(self, key: &str) -> String {
        match self {
            Mode::Camel => to_camel_case(key),
            Mode::Snake => key.to_owned(),
        }
    }

    /// Whether to omit a field whose value is a proto default. Only camel mode
    /// skips (ports `should_skip`); snake mode keeps every field (matches
    /// `proto::to_js_value`, which never skipped).
    fn skips(self) -> bool {
        matches!(self, Mode::Camel)
    }

    /// Whether a sequence of all-`u8` elements collapses to `Bytes`. Camel only
    /// — the detector lived only in the camel serializers.
    fn detects_u8_seq(self) -> bool {
        matches!(self, Mode::Camel)
    }

    /// Materialize a signed 64-bit value per mode: camel → protobufjs `Long`
    /// object (`i64_to_long`); snake → `I64` carried for number/string
    /// materialization (`proto::to_js_value`).
    fn i64_value(self, v: i64) -> BridgeValue {
        match self {
            Mode::Camel => BridgeValue::Long(LongParts::from_i64(v)),
            Mode::Snake => BridgeValue::I64(v),
        }
    }

    /// Materialize an unsigned 64-bit value per mode (see [`Mode::i64_value`]).
    fn u64_value(self, v: u64) -> BridgeValue {
        match self {
            Mode::Camel => BridgeValue::Long(LongParts::from_u64(v)),
            Mode::Snake => BridgeValue::U64(v),
        }
    }
}

// ---------------------------------------------------------------------------
// snake_case → camelCase  (ported verbatim from camel_serializer.rs:167-191)
// ---------------------------------------------------------------------------

/// Pure `String` camelCaser (the old code interned the result in a thread-local
/// `JsValue` cache — irrelevant without `JsValue`, so dropped).
fn to_camel_case(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut upper_next = false;
    let mut started = false;

    for &b in s.as_bytes() {
        if b == b'_' {
            if started {
                upper_next = true;
            }
            continue;
        }
        started = true;
        let c = if upper_next {
            upper_next = false;
            b.to_ascii_uppercase()
        } else {
            b
        };
        bytes.push(c);
    }

    // Safe: input is valid UTF-8, we only uppercased ASCII letters.
    unsafe { String::from_utf8_unchecked(bytes) }
}

// ---------------------------------------------------------------------------
// protobufjs default-skipping  (ports should_skip / is_zero_long, camel only)
// ---------------------------------------------------------------------------

/// protobufjs "only output set fields" — ports `should_skip`
/// (camel_serializer.rs:516-549) and the JSON twin's `should_skip_json`
/// (camel_serializer.rs:807-816), which agreed on every case. Omits null,
/// false, 0 number, zero `Long`, empty string, empty array, empty bytes, and
/// empty object. Camel mode only (snake never skips).
///
/// Note: camel always emits 64-bit ints as [`BridgeValue::Long`]; the
/// `I64`/`U64` arms are covered for completeness (and to satisfy the exhaustive
/// match) — a non-zero one is kept, a zero one skipped, exactly like `Long`.
fn should_skip(val: &BridgeValue) -> bool {
    match val {
        BridgeValue::Null => true,
        BridgeValue::Bool(b) => !b,
        BridgeValue::F64(n) => *n == 0.0,
        // Zero `Long` (i64/u64 == 0) is a proto default — skip like a zero
        // number (ports `is_zero_long`, camel_serializer.rs:112-134, which
        // inspected `low == 0 && high == 0`). Non-zero is kept.
        BridgeValue::Long(l) => l.is_zero(),
        BridgeValue::I64(v) => *v == 0,
        BridgeValue::U64(v) => *v == 0,
        BridgeValue::Str(s) => s.is_empty(),
        BridgeValue::Bytes(b) => b.is_empty(),
        BridgeValue::Array(a) => a.is_empty(),
        BridgeValue::Object(m) => m.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

/// serde `Serializer` producing a [`BridgeValue`]. Cheap to copy (just a mode).
#[derive(Clone, Copy)]
struct BridgeSerializer {
    mode: Mode,
}

impl ser::Serializer for BridgeSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;

    type SerializeSeq = SeqSerializer;
    type SerializeTuple = SeqSerializer;
    type SerializeTupleStruct = SeqSerializer;
    type SerializeTupleVariant = SeqSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = StructSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, v: bool) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Bool(v))
    }

    // Small ints fit a JS `number` losslessly → F64 (matches the old code, which
    // emitted `JsValue::from_f64` for i8..=i32 / u8..=u32).
    fn serialize_i8(self, v: i8) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    fn serialize_i16(self, v: i16) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    fn serialize_i32(self, v: i32) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    /// 64-bit signed. Camel → `Long` object (`i64_to_long`); snake → `I64`
    /// carried (number/string at the binding, per `proto::to_js_value`).
    fn serialize_i64(self, v: i64) -> Result<BridgeValue, BridgeSerError> {
        Ok(self.mode.i64_value(v))
    }
    fn serialize_u8(self, v: u8) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    fn serialize_u16(self, v: u16) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    fn serialize_u32(self, v: u32) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    /// 64-bit unsigned. Camel → `Long` object (`u64_to_long`); snake → `U64`.
    fn serialize_u64(self, v: u64) -> Result<BridgeValue, BridgeSerError> {
        Ok(self.mode.u64_value(v))
    }
    /// `i128` — keep the full value via the 64-bit path when it fits (so it gets
    /// the correct per-mode Long/number treatment), else degrade to `F64` (only
    /// genuinely-128-bit values lose precision). The old `JsValue` path had no
    /// i128 support; this is the agreed explicit behavior.
    fn serialize_i128(self, v: i128) -> Result<BridgeValue, BridgeSerError> {
        if let Ok(n) = i64::try_from(v) {
            Ok(self.mode.i64_value(n))
        } else if let Ok(n) = u64::try_from(v) {
            Ok(self.mode.u64_value(n))
        } else {
            Ok(BridgeValue::F64(v as f64))
        }
    }
    /// `u128` — keep via the `u64` path when it fits, else `F64`.
    fn serialize_u128(self, v: u128) -> Result<BridgeValue, BridgeSerError> {
        if let Ok(n) = u64::try_from(v) {
            Ok(self.mode.u64_value(n))
        } else {
            Ok(BridgeValue::F64(v as f64))
        }
    }
    fn serialize_f32(self, v: f32) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::F64(v))
    }
    fn serialize_char(self, v: char) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Str(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Str(v.to_owned()))
    }
    /// Raw bytes → `Bytes` (Uint8Array) in BOTH modes. See module docs.
    fn serialize_bytes(self, v: &[u8]) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(
        self,
        value: &T,
    ) -> Result<BridgeValue, BridgeSerError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
    ) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Str(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<BridgeValue, BridgeSerError> {
        value.serialize(self)
    }
    /// `{ variant: value }`. The variant name is used verbatim (NOT camelCased),
    /// matching the old `CamelSerializer`/`JsonCamelSerializer`, which set the
    /// raw `variant` key.
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<BridgeValue, BridgeSerError> {
        let val = value.serialize(self)?;
        Ok(BridgeValue::Object(vec![(variant.to_owned(), val)]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSerializer, BridgeSerError> {
        Ok(SeqSerializer {
            mode: self.mode,
            items: Vec::with_capacity(len.unwrap_or(0)),
            all_u8: self.mode.detects_u8_seq(),
            u8_buf: Vec::new(),
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSerializer, BridgeSerError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SeqSerializer, BridgeSerError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<SeqSerializer, BridgeSerError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<MapSerializer, BridgeSerError> {
        Ok(MapSerializer {
            mode: self.mode,
            entries: Vec::new(),
            next_key: None,
        })
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<StructSerializer, BridgeSerError> {
        Ok(StructSerializer {
            mode: self.mode,
            fields: Vec::with_capacity(len),
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructVariantSerializer, BridgeSerError> {
        Ok(StructVariantSerializer {
            variant,
            inner: StructSerializer {
                mode: self.mode,
                fields: Vec::with_capacity(len),
            },
        })
    }
}

// ---------------------------------------------------------------------------
// SerializeSeq — camel-mode all-u8 detector → Bytes (ports SeqSerializer)
// ---------------------------------------------------------------------------

pub struct SeqSerializer {
    mode: Mode,
    items: Vec<BridgeValue>,
    /// In camel mode, tracks whether every element so far is an integral `u8`
    /// in `0..=255`. Starts `false` in snake mode so the detector never fires.
    all_u8: bool,
    u8_buf: Vec<u8>,
}

impl SeqSerializer {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeSerError> {
        let v = value.serialize(BridgeSerializer { mode: self.mode })?;
        if self.all_u8 {
            // Mirror the old detector (camel_serializer.rs:357-373): a JS
            // `number` whose value is an integer in 0..=255. Here the element is
            // an F64 (small ints serialize to F64).
            match &v {
                BridgeValue::F64(n) => {
                    let rounded = *n as u8;
                    if (rounded as f64 - n).abs() < f64::EPSILON && (0.0..=255.0).contains(n) {
                        self.u8_buf.push(rounded);
                    } else {
                        self.all_u8 = false;
                    }
                }
                _ => self.all_u8 = false,
            }
        }
        self.items.push(v);
        Ok(())
    }

    fn finish(self) -> Result<BridgeValue, BridgeSerError> {
        // Only collapse to Bytes when the detector stayed on AND saw ≥1 byte —
        // an empty seq stays an empty Array (camel_serializer.rs:375-384).
        if self.all_u8 && !self.u8_buf.is_empty() {
            return Ok(BridgeValue::Bytes(self.u8_buf));
        }
        Ok(BridgeValue::Array(self.items))
    }
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;
    fn serialize_element<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), BridgeSerError> {
        self.push(value)
    }
    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        self.finish()
    }
}

impl ser::SerializeTuple for SeqSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;
    fn serialize_element<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), BridgeSerError> {
        self.push(value)
    }
    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for SeqSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeSerError> {
        self.push(value)
    }
    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for SeqSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeSerError> {
        self.push(value)
    }
    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        self.finish()
    }
}

// ---------------------------------------------------------------------------
// SerializeStruct — key transform + (camel-only) skip; ORDER preserved (Vec)
// ---------------------------------------------------------------------------

pub struct StructSerializer {
    mode: Mode,
    fields: Vec<(String, BridgeValue)>,
}

impl ser::SerializeStruct for StructSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), BridgeSerError> {
        let v = value.serialize(BridgeSerializer { mode: self.mode })?;
        if self.mode.skips() && should_skip(&v) {
            return Ok(());
        }
        self.fields.push((self.mode.key(key), v));
        Ok(())
    }

    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Object(self.fields))
    }
}

// ---------------------------------------------------------------------------
// SerializeStructVariant — { variant: { ..fields } } (ports StructVariant*)
// ---------------------------------------------------------------------------

pub struct StructVariantSerializer {
    variant: &'static str,
    inner: StructSerializer,
}

impl ser::SerializeStructVariant for StructVariantSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), BridgeSerError> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, value)
    }

    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        // Variant name verbatim (not camelCased), matching the old code.
        let inner = ser::SerializeStruct::end(self.inner)?;
        Ok(BridgeValue::Object(vec![(self.variant.to_owned(), inner)]))
    }
}

// ---------------------------------------------------------------------------
// SerializeMap — maps as objects (serialize_maps_as_objects(true)); ORDER kept
// ---------------------------------------------------------------------------

pub struct MapSerializer {
    mode: Mode,
    entries: Vec<(String, BridgeValue)>,
    next_key: Option<String>,
}

impl ser::SerializeMap for MapSerializer {
    type Ok = BridgeValue;
    type Error = BridgeSerError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), BridgeSerError> {
        // Map keys are taken verbatim from the serialized key's string form (the
        // old code used `as_string()` / `as_str()`); map keys are NOT camelCased
        // in either mode — only struct field names are. A non-string key yields
        // an empty string, matching the old `unwrap_or_default()`.
        let k = key.serialize(BridgeSerializer { mode: self.mode })?;
        self.next_key = match k {
            BridgeValue::Str(s) => Some(s),
            _ => None,
        };
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeSerError> {
        let key = self.next_key.take().unwrap_or_default();
        let v = value.serialize(BridgeSerializer { mode: self.mode })?;
        self.entries.push((key, v));
        Ok(())
    }

    fn end(self) -> Result<BridgeValue, BridgeSerError> {
        Ok(BridgeValue::Object(self.entries))
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Serialize with `camelCase` keys, `Bytes` for byte payloads, and proto
/// default values skipped. For proto / message / `history_sync` types. Ports
/// `to_js_value_camel` / `to_json_value_camel`.
pub fn to_bridge_camel<T: Serialize>(val: &T) -> Result<BridgeValue, BridgeSerError> {
    val.serialize(BridgeSerializer { mode: Mode::Camel })
}

/// Serialize preserving key casing, with NOTHING skipped. For the `serialize {}`
/// event variants + `info`. Ports `proto::to_js_value`.
pub fn to_bridge_snake<T: Serialize>(val: &T) -> Result<BridgeValue, BridgeSerError> {
    val.serialize(BridgeSerializer { mode: Mode::Snake })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[test]
    fn camel_renames_keys() {
        #[derive(Serialize)]
        struct S {
            message_timestamp: u32,
            push_name: String,
        }
        let v = to_bridge_camel(&S {
            message_timestamp: 5,
            push_name: "x".into(),
        })
        .unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["messageTimestamp", "pushName"]);
    }

    #[test]
    fn snake_preserves_keys_and_order() {
        #[derive(Serialize)]
        struct S {
            push_name: String,
            message_timestamp: u32,
        }
        let v = to_bridge_snake(&S {
            push_name: "x".into(),
            message_timestamp: 5,
        })
        .unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["push_name", "message_timestamp"]);
    }

    #[test]
    fn camel_skips_proto_defaults() {
        #[derive(Serialize)]
        struct S {
            kept: u32,
            zero_num: u32,
            empty_str: String,
            falsey: bool,
            zero_i64: i64,
            zero_u64: u64,
            empty_vec: Vec<u32>,
        }
        let v = to_bridge_camel(&S {
            kept: 7,
            zero_num: 0,
            empty_str: String::new(),
            falsey: false,
            zero_i64: 0,
            zero_u64: 0,
            empty_vec: vec![],
        })
        .unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["kept"]);
    }

    #[test]
    fn snake_skips_nothing() {
        #[derive(Serialize)]
        struct S {
            kept: u32,
            zero_num: u32,
            empty_str: String,
            falsey: bool,
        }
        let v = to_bridge_snake(&S {
            kept: 7,
            zero_num: 0,
            empty_str: String::new(),
            falsey: false,
        })
        .unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        assert_eq!(fields.len(), 4);
    }

    #[test]
    fn camel_64bit_is_long_object() {
        #[derive(Serialize)]
        struct S {
            big: i64,
            ubig: u64,
        }
        // Camel mode emits protobufjs Long objects (split low/high, unsigned).
        let v = to_bridge_camel(&S {
            big: 2310452236816384026,
            ubig: 18446744073709551615,
        })
        .unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        assert_eq!(
            fields[0].1,
            BridgeValue::Long(LongParts::from_i64(2310452236816384026))
        );
        assert_eq!(
            fields[1].1,
            BridgeValue::Long(LongParts::from_u64(18446744073709551615))
        );
    }

    #[test]
    fn snake_64bit_is_carried_raw() {
        #[derive(Serialize)]
        struct S {
            big: i64,
            ubig: u64,
        }
        // Snake mode carries the raw 64-bit value; the binding emits number or
        // out-of-range string (matches proto::to_js_value).
        let v = to_bridge_snake(&S {
            big: 2310452236816384026,
            ubig: 18446744073709551615,
        })
        .unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        assert_eq!(fields[0].1, BridgeValue::I64(2310452236816384026));
        assert_eq!(fields[1].1, BridgeValue::U64(18446744073709551615));
    }

    #[test]
    fn serialize_bytes_is_bytes_both_modes() {
        // serde_bytes-style explicit bytes go through serialize_bytes.
        struct B(Vec<u8>);
        impl Serialize for B {
            fn serialize<Sr: serde::Serializer>(&self, s: Sr) -> Result<Sr::Ok, Sr::Error> {
                s.serialize_bytes(&self.0)
            }
        }
        assert_eq!(
            to_bridge_camel(&B(vec![1, 2, 3])).unwrap(),
            BridgeValue::Bytes(vec![1, 2, 3])
        );
        assert_eq!(
            to_bridge_snake(&B(vec![1, 2, 3])).unwrap(),
            BridgeValue::Bytes(vec![1, 2, 3])
        );
    }

    #[test]
    fn all_u8_seq_collapses_only_in_camel() {
        // A Vec<u8> serializes as a seq of u8 via serde's default impl.
        let data: Vec<u8> = vec![10, 20, 30];
        assert_eq!(
            to_bridge_camel(&data).unwrap(),
            BridgeValue::Bytes(vec![10, 20, 30])
        );
        // Snake mode leaves it a number array (matches serde_wasm_bindgen).
        assert_eq!(
            to_bridge_snake(&data).unwrap(),
            BridgeValue::Array(vec![
                BridgeValue::F64(10.0),
                BridgeValue::F64(20.0),
                BridgeValue::F64(30.0),
            ])
        );
    }

    #[test]
    fn empty_u8_seq_stays_array_in_camel() {
        let data: Vec<u8> = vec![];
        // Empty seq must NOT become empty Bytes (the detector requires ≥1 byte).
        assert_eq!(to_bridge_camel(&data).unwrap(), BridgeValue::Array(vec![]));
    }

    #[test]
    fn map_is_object_keys_verbatim() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("some_key".to_string(), 1u32);
        let v = to_bridge_camel(&m).unwrap();
        let BridgeValue::Object(fields) = v else {
            panic!("expected object")
        };
        // Map keys are NOT camelCased — only struct field names are.
        assert_eq!(fields[0].0, "some_key");
    }

    #[test]
    fn i128_fits_i64_else_f64() {
        struct I(i128);
        impl Serialize for I {
            fn serialize<Sr: serde::Serializer>(&self, s: Sr) -> Result<Sr::Ok, Sr::Error> {
                s.serialize_i128(self.0)
            }
        }
        assert_eq!(to_bridge_snake(&I(42)).unwrap(), BridgeValue::I64(42));
        // Beyond u64 → F64.
        let huge: i128 = (u64::MAX as i128) + 1;
        assert!(matches!(
            to_bridge_snake(&I(huge)).unwrap(),
            BridgeValue::F64(_)
        ));
    }

    #[test]
    fn newtype_variant_wraps_in_single_key_object() {
        #[derive(Serialize)]
        enum E {
            SomeVariant(u32),
        }
        let v = to_bridge_camel(&E::SomeVariant(5)).unwrap();
        // Variant name is verbatim (not camelCased here — derive already gives
        // the on-the-wire name).
        assert_eq!(
            v,
            BridgeValue::Object(vec![("SomeVariant".to_string(), BridgeValue::F64(5.0))])
        );
    }
}
