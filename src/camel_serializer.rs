//! Custom serde Serializer that outputs JsValue with:
//! - camelCase field names (converts from Rust snake_case)
//! - Uint8Array for byte sequences (detects Vec<u8> serialized as seq of u8)
//! - Skips None, empty Vec, empty String, zero numbers, false booleans
//! - protobufjs-style `Long` objects `{ low, high, unsigned }` for i64/u64
//!
//! This lives entirely in the bridge — waproto stays agnostic.
//!
//! ## Why `Long` objects and not `BigInt`
//!
//! protobuf `int64`/`uint64`/`fixed64`/`sfixed64` fields don't fit in a JS
//! `number` without precision loss. We used to emit `BigInt`, but `BigInt`
//! cannot be serialized by `JSON.stringify` (it throws `TypeError: cannot
//! serialize BigInt`), while downstream consumers commonly use
//! `JSON.stringify(event)` for logging/debugging. protobufjs represents 64-bit fields as a `Long` object
//! `{ low, high, unsigned }`, which `JSON.stringify` handles and which the
//! numeric conversion helpers already understand. The output follows that
//! interoperable representation.

use js_sys::{Object, Uint8Array};
use serde::ser::{self, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

thread_local! {
    /// Cache of camelCase field-name JsStrings, keyed by the `&'static str` proto
    /// field name. Proto field names are a small fixed set, so this is bounded.
    /// Reusing one JsString per name across every message avoids — per field, per
    /// message — a `to_camel_case` Rust `String` alloc AND a `JsValue::from_str`
    /// (JS string alloc + externref register), which the heap profile showed to be
    /// the bulk of the JS↔WASM message-serialization allocations.
    static CAMEL_KEY_CACHE: RefCell<HashMap<&'static str, JsValue>> = RefCell::new(HashMap::new());
}

/// Run `f` with the cached camelCase-key JsString for `key` (built once per key).
/// `f` is the `Reflect::set` call; passing the cached `&JsValue` by reference
/// avoids creating/registering a new JS string for the key on every field.
#[inline]
fn with_camel_key<R>(key: &'static str, f: impl FnOnce(&JsValue) -> R) -> R {
    CAMEL_KEY_CACHE.with(|c| {
        let mut map = c.borrow_mut();
        let js = map
            .entry(key)
            .or_insert_with(|| JsValue::from_str(&to_camel_case(key)));
        f(js)
    })
}

thread_local! {
    /// The three protobufjs `Long` object keys, interned once per thread. Every
    /// 64-bit field (e.g. `messageTimestamp`, present on every message) builds a
    /// `Long` object; without interning each one re-allocates these three JS
    /// strings per field per message. They never need camelCasing (already plain
    /// lowercase identifiers), so they get dedicated slots instead of the
    /// `CAMEL_KEY_CACHE` HashMap — no hashing on this hot path.
    static LONG_KEY_LOW: JsValue = JsValue::from_str("low");
    static LONG_KEY_HIGH: JsValue = JsValue::from_str("high");
    static LONG_KEY_UNSIGNED: JsValue = JsValue::from_str("unsigned");
}

thread_local! {
    /// Interned JS strings for JID values. Serialized events repeat the same
    /// handful of addresses (chat/sender/participant) on every message, and
    /// each uncached crossing pays a TextDecoder decode plus a fresh JS string
    /// allocation. Bounded: cleared when it reaches `JID_CACHE_MAX` entries.
    static JID_VALUE_CACHE: RefCell<HashMap<String, JsValue>> = RefCell::new(HashMap::new());
}

const JID_CACHE_MAX: usize = 1024;

/// High-precision test for interning: the core's zero-alloc JID parser plus
/// its canonical server table, so only address-shaped values enter the cache
/// (unbounded user content like message text never does) and new servers stay
/// recognized without a duplicated list here.
fn is_jid_like(v: &str) -> bool {
    use whatsapp_rust::wacore_binary::jid::{Server, parse_jid_fast};
    v.len() <= 64 && parse_jid_fast(v).is_some_and(|parts| Server::try_from(parts.server).is_ok())
}

fn intern_str_value(v: &str) -> JsValue {
    JID_VALUE_CACHE.with(|c| {
        let mut map = c.borrow_mut();
        if let Some(js) = map.get(v) {
            return js.clone();
        }
        if map.len() >= JID_CACHE_MAX {
            map.clear();
        }
        let js = JsValue::from_str(v);
        map.insert(v.to_owned(), js.clone());
        js
    })
}

/// Split a signed `i64` into protobufjs-style `Long` parts: the low 32 bits as
/// an unsigned value and the high 32 bits as a *signed* (two's-complement)
/// value. `value = high * 2^32 + (low >>> 0)`. Pure arithmetic — unit-tested
/// natively (the `JsValue` assembly in `long_object` is trivial plumbing).
pub(crate) fn i64_to_long_parts(v: i64) -> (u32, i32) {
    (v as u32, (v >> 32) as i32)
}

/// Split a `u64` into `Long` parts (low unsigned, high reinterpreted as the
/// signed 32-bit field protobufjs uses). `value = (high >>> 0) * 2^32 + low`.
pub(crate) fn u64_to_long_parts(v: u64) -> (u32, i32) {
    (v as u32, (v >> 32) as u32 as i32)
}

/// Build a protobufjs-style `Long` JS object `{ low, high, unsigned }` from the
/// split 32-bit halves. `low`/`high` are emitted as JS numbers (both fit in a
/// 32-bit range, so exact). `JSON.stringify`-safe and consumed by host
/// `toNumber()`. Mirrors `protobufjs/Long`.
fn long_object(low: u32, high: i32, unsigned: bool) -> JsValue {
    let obj = Object::new();
    LONG_KEY_LOW.with(|k| {
        let _ = js_sys::Reflect::set(&obj, k, &JsValue::from_f64(low as f64));
    });
    LONG_KEY_HIGH.with(|k| {
        let _ = js_sys::Reflect::set(&obj, k, &JsValue::from_f64(high as f64));
    });
    LONG_KEY_UNSIGNED.with(|k| {
        let _ = js_sys::Reflect::set(&obj, k, &JsValue::from_bool(unsigned));
    });
    obj.into()
}

/// `i64` → signed `Long` object.
pub(crate) fn i64_to_long(v: i64) -> JsValue {
    let (low, high) = i64_to_long_parts(v);
    long_object(low, high, false)
}

/// `u64` → unsigned `Long` object.
pub(crate) fn u64_to_long(v: u64) -> JsValue {
    let (low, high) = u64_to_long_parts(v);
    long_object(low, high, true)
}

/// True when `val` is a `Long` object whose 64-bit value is zero (`low` and
/// `high` both 0). Used by `should_skip` so a default-zero 64-bit field is
/// omitted just like a zero `number` is — without this a `0` int64 would be
/// emitted as `{low:0,high:0,unsigned:..}` instead of being skipped.
fn is_zero_long(val: &JsValue) -> bool {
    if !val.is_object() {
        return false;
    }
    let low = LONG_KEY_LOW.with(|k| js_sys::Reflect::get(val, k)).ok();
    let high = LONG_KEY_HIGH.with(|k| js_sys::Reflect::get(val, k)).ok();
    match (
        low.as_ref().and_then(JsValue::as_f64),
        high.as_ref().and_then(JsValue::as_f64),
    ) {
        (Some(0.0), Some(0.0)) => {
            // Guard against false positives: only treat as a Long if it also
            // carries the `unsigned` discriminant (plain `{low,high}` data
            // objects without it are vanishingly unlikely in proto output, but
            // be precise).
            LONG_KEY_UNSIGNED
                .with(|k| js_sys::Reflect::get(val, k))
                .ok()
                .is_some_and(|u| u.as_bool().is_some())
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

impl From<Error> for JsValue {
    fn from(e: Error) -> Self {
        JsValue::from_str(&e.0)
    }
}

// ---------------------------------------------------------------------------
// snake_case → camelCase
// ---------------------------------------------------------------------------

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

    // Safe: input is valid UTF-8, we only uppercased ASCII letters
    unsafe { String::from_utf8_unchecked(bytes) }
}

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

/// What a top-level struct field does with a value equal to its type's default.
///
/// Absent is absent either way: `None` never crosses. This is only about a
/// value the wire did supply that happens to equal the default.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Defaults {
    /// protobufjs: a default-valued field is indistinguishable from an unset
    /// one, so it is omitted.
    Skip,
    /// Every supplied value crosses, `0` and `false` and `""` alike.
    Keep,
    /// Every value the wire supplied crosses — `false`, `0` and `""` alike,
    /// since an unpin is `pinned: false` and dropping it loses the transition.
    /// A repeated field with no elements is still omitted: protobuf gives it no
    /// presence, so an empty one is the same as one never set.
    ///
    /// Applies all the way down, unlike [`Keep`](Self::Keep). A message nested
    /// in a mutation carries presence too — `messageRange.messages[].key.fromMe`
    /// is what identifies the message an archive covers — and it is `None`, not
    /// `Some(false)`, that the wire left out.
    KeepPresent,
}

impl Defaults {
    /// What a nested value is serialized with. Only [`KeepPresent`](Self::KeepPresent)
    /// recurses; the envelope mode preserves its own scalars and leaves protobuf
    /// nested inside it to protobufjs semantics.
    fn nested(self) -> CamelSerializer {
        match self {
            Defaults::KeepPresent => CamelSerializer::PRESERVE_TOP_LEVEL_PRESENCE,
            Defaults::Skip | Defaults::Keep => CamelSerializer::PROTO,
        }
    }
}

/// Serializes Rust values to JsValue with camelCase keys and proto-friendly output.
#[derive(Clone, Copy)]
pub struct CamelSerializer {
    struct_defaults: Defaults,
}

impl CamelSerializer {
    const PROTO: Self = Self {
        struct_defaults: Defaults::Skip,
    };
    const PRESERVE_TOP_LEVEL_DEFAULTS: Self = Self {
        struct_defaults: Defaults::Keep,
    };
    const PRESERVE_TOP_LEVEL_PRESENCE: Self = Self {
        struct_defaults: Defaults::KeepPresent,
    };
}

impl ser::Serializer for CamelSerializer {
    type Ok = JsValue;
    type Error = Error;

    type SerializeSeq = SeqSerializer;
    type SerializeTuple = SeqSerializer;
    type SerializeTupleStruct = SeqSerializer;
    type SerializeTupleVariant = SeqSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = StructSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, v: bool) -> Result<JsValue, Error> {
        Ok(JsValue::from_bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_i16(self, v: i16) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_i32(self, v: i32) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_i64(self, v: i64) -> Result<JsValue, Error> {
        Ok(i64_to_long(v))
    }
    fn serialize_u8(self, v: u8) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_u16(self, v: u16) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_u32(self, v: u32) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_u64(self, v: u64) -> Result<JsValue, Error> {
        Ok(u64_to_long(v))
    }
    fn serialize_f32(self, v: f32) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<JsValue, Error> {
        Ok(JsValue::from_f64(v))
    }
    fn serialize_char(self, v: char) -> Result<JsValue, Error> {
        Ok(JsValue::from_str(&v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<JsValue, Error> {
        if is_jid_like(v) {
            return Ok(intern_str_value(v));
        }
        Ok(JsValue::from_str(v))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<JsValue, Error> {
        Ok(Uint8Array::from(v).into())
    }
    fn serialize_none(self) -> Result<JsValue, Error> {
        Ok(JsValue::NULL)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<JsValue, Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<JsValue, Error> {
        Ok(JsValue::NULL)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<JsValue, Error> {
        Ok(JsValue::NULL)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
    ) -> Result<JsValue, Error> {
        Ok(JsValue::from_str(variant))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<JsValue, Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<JsValue, Error> {
        let obj = Object::new();
        let val = value.serialize(Self::PROTO)?;
        js_sys::Reflect::set(&obj, &JsValue::from_str(variant), &val)
            .map_err(|e| Error(format!("{e:?}")))?;
        Ok(obj.into())
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSerializer, Error> {
        Ok(SeqSerializer {
            items: SeqItems::Unknown {
                capacity: len.unwrap_or(0),
            },
            defaults: self.struct_defaults,
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSerializer, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SeqSerializer, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<SeqSerializer, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<MapSerializer, Error> {
        Ok(MapSerializer {
            obj: Object::new(),
            next_key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<StructSerializer, Error> {
        Ok(StructSerializer {
            obj: Object::new(),
            defaults: self.struct_defaults,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<StructVariantSerializer, Error> {
        Ok(StructVariantSerializer {
            variant,
            inner: StructSerializer {
                obj: Object::new(),
                defaults: self.struct_defaults,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// SerializeSeq — detects all-u8 sequences → outputs Uint8Array
// ---------------------------------------------------------------------------

enum SeqItems {
    /// No element has established the output representation yet. Keeping only
    /// the capacity hint avoids allocating two candidate buffers for every
    /// protobuf sequence.
    Unknown {
        capacity: usize,
    },
    Bytes(Vec<u8>),
    Values(Vec<JsValue>),
}

pub struct SeqSerializer {
    items: SeqItems,
    defaults: Defaults,
}

#[inline]
fn js_u8(value: &JsValue) -> Option<u8> {
    let number = value.as_f64()?;
    let byte = number as u8;
    ((byte as f64 - number).abs() < f64::EPSILON && (0.0..=255.0).contains(&number)).then_some(byte)
}

impl SeqSerializer {
    fn finish_element(&mut self, js: JsValue, byte: Option<u8>) -> Result<(), Error> {
        match &mut self.items {
            SeqItems::Unknown { capacity } => {
                if let Some(byte) = byte {
                    let mut bytes = Vec::with_capacity(*capacity);
                    bytes.push(byte);
                    self.items = SeqItems::Bytes(bytes);
                } else {
                    let mut values = Vec::with_capacity(*capacity);
                    values.push(js);
                    self.items = SeqItems::Values(values);
                }
            }
            SeqItems::Bytes(bytes) => {
                if let Some(byte) = byte {
                    bytes.push(byte);
                } else {
                    // Preserve the established heterogeneous-sequence behavior:
                    // numeric values seen before the first non-u8 become normal
                    // JS numbers in the final Array.
                    let mut values = Vec::with_capacity(bytes.capacity().max(bytes.len() + 1));
                    values.extend(bytes.drain(..).map(|byte| JsValue::from_f64(byte as f64)));
                    values.push(js);
                    self.items = SeqItems::Values(values);
                }
            }
            SeqItems::Values(values) => values.push(js),
        }
        Ok(())
    }
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = JsValue;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let js = value.serialize(self.defaults.nested())?;
        let byte = js_u8(&js);
        self.finish_element(js, byte)
    }

    fn end(self) -> Result<JsValue, Error> {
        match self.items {
            SeqItems::Unknown { .. } => Ok(js_sys::Array::new().into()),
            SeqItems::Bytes(bytes) => Ok(Uint8Array::from(bytes.as_slice()).into()),
            SeqItems::Values(values) => {
                let arr = js_sys::Array::new_with_length(values.len() as u32);
                for (index, item) in values.into_iter().enumerate() {
                    arr.set(index as u32, item);
                }
                Ok(arr.into())
            }
        }
    }
}

// Reuse SeqSerializer for tuples
impl ser::SerializeTuple for SeqSerializer {
    type Ok = JsValue;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<JsValue, Error> {
        ser::SerializeSeq::end(self)
    }
}
impl ser::SerializeTupleStruct for SeqSerializer {
    type Ok = JsValue;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<JsValue, Error> {
        ser::SerializeSeq::end(self)
    }
}
impl ser::SerializeTupleVariant for SeqSerializer {
    type Ok = JsValue;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<JsValue, Error> {
        ser::SerializeSeq::end(self)
    }
}

// ---------------------------------------------------------------------------
// SerializeStruct — camelCase keys, skip defaults
// ---------------------------------------------------------------------------

pub struct StructSerializer {
    obj: Object,
    defaults: Defaults,
}

impl StructSerializer {
    fn finish_field(&mut self, key: &'static str, js_val: JsValue) -> Result<(), Error> {
        let skip = match self.defaults {
            Defaults::Skip => should_skip(&js_val),
            Defaults::Keep => js_val.is_null() || js_val.is_undefined(),
            Defaults::KeepPresent => is_absent(&js_val),
        };
        if skip {
            return Ok(());
        }
        with_camel_key(key, |k| js_sys::Reflect::set(&self.obj, k, &js_val))
            .map_err(|e| Error(format!("{e:?}")))?;
        Ok(())
    }
}

impl ser::SerializeStruct for StructSerializer {
    type Ok = JsValue;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        let js_val = value.serialize(self.defaults.nested())?;
        self.finish_field(key, js_val)
    }

    fn end(self) -> Result<JsValue, Error> {
        Ok(self.obj.into())
    }
}

// ---------------------------------------------------------------------------
// SerializeStructVariant
// ---------------------------------------------------------------------------

pub struct StructVariantSerializer {
    variant: &'static str,
    inner: StructSerializer,
}

impl ser::SerializeStructVariant for StructVariantSerializer {
    type Ok = JsValue;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, value)
    }

    fn end(self) -> Result<JsValue, Error> {
        let obj = Object::new();
        let inner = ser::SerializeStruct::end(self.inner)?;
        js_sys::Reflect::set(&obj, &JsValue::from_str(self.variant), &inner)
            .map_err(|e| Error(format!("{e:?}")))?;
        Ok(obj.into())
    }
}

// ---------------------------------------------------------------------------
// SerializeMap
// ---------------------------------------------------------------------------

pub struct MapSerializer {
    obj: Object,
    next_key: Option<String>,
}

impl ser::SerializeMap for MapSerializer {
    type Ok = JsValue;
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        let js_key = key.serialize(CamelSerializer::PROTO)?;
        self.next_key = js_key.as_string();
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let key = self.next_key.take().unwrap_or_default();
        let js_val = value.serialize(CamelSerializer::PROTO)?;
        js_sys::Reflect::set(&self.obj, &JsValue::from_str(&key), &js_val)
            .map_err(|e| Error(format!("{e:?}")))?;
        Ok(())
    }

    fn end(self) -> Result<JsValue, Error> {
        Ok(self.obj.into())
    }
}

// ---------------------------------------------------------------------------
// Skip logic — matches protobufjs behavior (only output set fields)
// ---------------------------------------------------------------------------

/// Nothing was supplied. `null` is an absent `Option`; an empty repeated field
/// is the one other shape protobuf cannot tell from one never set, so `""` and
/// `{}` — which only a `Some` produces — are not absent.
fn is_absent(val: &JsValue) -> bool {
    if val.is_null() || val.is_undefined() {
        return true;
    }
    is_empty_sequence(val)
}

/// `[]` or an empty `Uint8Array`.
fn is_empty_sequence(val: &JsValue) -> bool {
    if !val.is_object() {
        return false;
    }
    if val.is_instance_of::<js_sys::Array>() {
        let arr: js_sys::Array = js_sys::Array::unchecked_from_js(val.clone());
        return arr.length() == 0;
    }
    if val.is_instance_of::<Uint8Array>() {
        let arr: Uint8Array = Uint8Array::unchecked_from_js(val.clone());
        return arr.length() == 0;
    }
    false
}

/// `[]`, an empty `Uint8Array`, or a message with no fields.
fn is_empty_collection(val: &JsValue) -> bool {
    if !val.is_object() {
        return false;
    }
    if is_empty_sequence(val) {
        return true;
    }
    if val.is_instance_of::<js_sys::Array>() || val.is_instance_of::<Uint8Array>() {
        return false;
    }
    if is_zero_long(val) {
        return false;
    }
    let obj: Object = Object::unchecked_from_js(val.clone());
    js_sys::Object::keys(&obj).length() == 0
}

fn should_skip(val: &JsValue) -> bool {
    if val.is_null() || val.is_undefined() {
        return true;
    }
    if let Some(s) = val.as_string() {
        return s.is_empty();
    }
    if let Some(n) = val.as_f64() {
        return n == 0.0;
    }
    if let Some(b) = val.as_bool() {
        return !b;
    }
    // A zero-valued `Long` object (i64/u64 == 0) is a proto default — skip it
    // exactly like a zero `number`. Without this, a default 0 int64 leaks into
    // the output as `{low:0,high:0,unsigned:...}`.
    if is_zero_long(val) {
        return true;
    }
    // Expensive checks only for objects — avoid clone when possible
    is_empty_collection(val)
}

// ---------------------------------------------------------------------------
// Public API — JS (existing)
// ---------------------------------------------------------------------------

/// Serialize a value to JsValue with camelCase keys, Uint8Array for bytes,
/// and proto default values skipped. For proto types only.
pub fn to_js_value_camel<T: Serialize>(val: &T) -> Result<JsValue, JsValue> {
    val.serialize(CamelSerializer::PROTO).map_err(|e| e.into())
}

/// Same JS representation as [`to_js_value_camel`], but preserves scalar
/// defaults for fields of the outer struct while still omitting absent
/// `Option`s. This is for non-protobuf envelopes whose `0`/`false` values carry
/// presence semantics; nested protobuf values keep the normal default-skipping
/// behavior.
pub fn to_js_value_camel_preserve_top_level_defaults<T: Serialize>(
    val: &T,
) -> Result<JsValue, JsValue> {
    val.serialize(CamelSerializer::PRESERVE_TOP_LEVEL_DEFAULTS)
        .map_err(|e| e.into())
}

/// Same JS representation as [`to_js_value_camel`], but every value the wire
/// supplied crosses even at its default — see [`Defaults::KeepPresent`]. For a
/// protobuf value whose `false` or `0` is the thing being reported.
pub fn to_js_value_camel_preserve_top_level_presence<T: Serialize>(
    val: &T,
) -> Result<JsValue, JsValue> {
    val.serialize(CamelSerializer::PRESERVE_TOP_LEVEL_PRESENCE)
        .map_err(|e| e.into())
}

#[cfg(test)]
mod js_serializer_tests {
    use super::{to_js_value_camel, to_js_value_camel_preserve_top_level_defaults};
    use serde::Serialize;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[derive(Serialize)]
    struct Envelope {
        sync_type: i32,
        is_final: bool,
        progress: Option<u32>,
        absent: Option<u32>,
    }

    #[derive(Serialize)]
    struct Sequences {
        bytes: Vec<u8>,
        // Starts in the byte candidate state, then transitions to a JS Array.
        wider_numbers: Vec<u16>,
    }

    fn field(value: &JsValue, key: &str) -> JsValue {
        js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("read serialized field")
    }

    #[test]
    fn envelope_mode_preserves_scalar_defaults_but_not_absent_options() {
        let envelope = Envelope {
            sync_type: 0,
            is_final: false,
            progress: Some(0),
            absent: None,
        };

        let regular = to_js_value_camel(&envelope).expect("regular serialization");
        assert!(field(&regular, "syncType").is_undefined());
        assert!(field(&regular, "isFinal").is_undefined());
        assert!(field(&regular, "progress").is_undefined());

        let preserved = to_js_value_camel_preserve_top_level_defaults(&envelope)
            .expect("envelope serialization");
        assert_eq!(field(&preserved, "syncType").as_f64(), Some(0.0));
        assert_eq!(field(&preserved, "isFinal").as_bool(), Some(false));
        assert_eq!(field(&preserved, "progress").as_f64(), Some(0.0));
        assert!(field(&preserved, "absent").is_undefined());
    }

    #[test]
    fn terminal_modes_keep_presence_and_the_supplied_object_identity() {
        use super::{Defaults, StructSerializer, u64_to_long};
        let future = js_sys::Object::new();
        js_sys::Reflect::set(&future, &"unknown_name".into(), &JsValue::NULL).unwrap();
        let cases = [
            (JsValue::NULL, [false, false, false]),
            (JsValue::UNDEFINED, [false, false, false]),
            (JsValue::FALSE, [false, true, true]),
            (JsValue::from_f64(0.0), [false, true, true]),
            (JsValue::from_str(""), [false, true, true]),
            (js_sys::Array::new().into(), [false, true, false]),
            (
                js_sys::Uint8Array::new_with_length(0).into(),
                [false, true, false],
            ),
            (js_sys::Object::new().into(), [false, true, true]),
            (u64_to_long(0), [false, true, true]),
            (u64_to_long(u64::MAX), [true, true, true]),
            (future.into(), [true, true, true]),
        ];
        for (value, present) in cases {
            for (index, defaults) in [Defaults::Skip, Defaults::Keep, Defaults::KeepPresent]
                .into_iter()
                .enumerate()
            {
                let object = js_sys::Object::new();
                let mut writer = StructSerializer {
                    obj: object.clone(),
                    defaults,
                };
                writer.finish_field("some_field", value.clone()).unwrap();
                assert_eq!(writer.obj, object);
                assert_eq!(
                    js_sys::Reflect::has(&object, &"someField".into()).unwrap(),
                    present[index]
                );
                if present[index] {
                    assert_eq!(field(&object, "someField"), value);
                }
            }
        }
    }

    struct CountedBool<'a> {
        calls: &'a std::cell::Cell<u32>,
        fail: bool,
    }

    impl Serialize for CountedBool<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::Error as _;
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                return Err(S::Error::custom("value control failure"));
            }
            serializer.serialize_bool(false)
        }
    }

    #[test]
    fn value_runs_once_before_skip_and_value_failure_never_sets() {
        use super::{Defaults, StructSerializer};
        use serde::ser::SerializeStruct as _;
        let calls = std::cell::Cell::new(0);
        let object = js_sys::Object::new();
        let mut writer = StructSerializer {
            obj: object.clone(),
            defaults: Defaults::Skip,
        };
        writer
            .serialize_field(
                "counted_field",
                &CountedBool {
                    calls: &calls,
                    fail: false,
                },
            )
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert!(!js_sys::Reflect::has(&object, &"countedField".into()).unwrap());
        let error = writer
            .serialize_field(
                "counted_field",
                &CountedBool {
                    calls: &calls,
                    fail: true,
                },
            )
            .unwrap_err();
        assert_eq!(calls.get(), 2);
        assert_eq!(error.to_string(), "value control failure");
        assert!(!js_sys::Reflect::has(&object, &"countedField".into()).unwrap());
    }

    #[test]
    fn cached_key_borrow_stays_held_through_the_setter_and_then_releases() {
        use super::{CAMEL_KEY_CACHE, Defaults, StructSerializer};
        use serde::ser::SerializeStruct as _;
        use std::{cell::Cell, rc::Rc};
        use wasm_bindgen::closure::Closure;
        let calls = Rc::new(Cell::new(0));
        let observed = Rc::new(Cell::new(0));
        let callback_calls = calls.clone();
        let callback_observed = observed.clone();
        let setter = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
            assert_eq!(value.as_bool(), Some(false));
            assert_eq!(callback_calls.get(), callback_observed.get() + 1);
            assert!(CAMEL_KEY_CACHE.with(|cache| cache.try_borrow_mut().is_err()));
            callback_observed.set(callback_observed.get() + 1);
        });
        let descriptor = js_sys::Object::new();
        js_sys::Reflect::set(&descriptor, &"set".into(), setter.as_ref()).unwrap();
        let object = js_sys::Object::new();
        js_sys::Object::define_property(&object, &"controlledField".into(), &descriptor);
        let mut writer = StructSerializer {
            obj: object.clone(),
            defaults: Defaults::KeepPresent,
        };
        for _ in 0..2 {
            writer
                .serialize_field(
                    "controlled_field",
                    &CountedBool {
                        calls: &calls,
                        fail: false,
                    },
                )
                .unwrap();
            assert!(CAMEL_KEY_CACHE.with(|cache| cache.try_borrow_mut().is_ok()));
        }
        assert_eq!(calls.get(), 2);
        assert_eq!(observed.get(), 2);
        assert_eq!(writer.obj, object);
    }

    #[test]
    fn setter_exception_format_and_false_result_match_the_existing_reflect_contract() {
        use super::{CAMEL_KEY_CACHE, Defaults, StructSerializer};
        use serde::ser::SerializeStruct as _;
        let descriptor = js_sys::Object::new();
        let setter = js_sys::Function::new_no_args("throw 'terminal setter control'");
        js_sys::Reflect::set(&descriptor, &"set".into(), &setter).unwrap();
        let object = js_sys::Object::new();
        js_sys::Object::define_property(&object, &"controlledField".into(), &descriptor);
        let expected =
            js_sys::Reflect::set(&object, &"controlledField".into(), &JsValue::TRUE).unwrap_err();
        let mut writer = StructSerializer {
            obj: object,
            defaults: Defaults::KeepPresent,
        };
        let error = writer
            .serialize_field("controlled_field", &true)
            .unwrap_err();
        assert_eq!(error.to_string(), format!("{expected:?}"));
        assert!(CAMEL_KEY_CACHE.with(|cache| cache.try_borrow_mut().is_ok()));
        let object = js_sys::Object::new();
        js_sys::Object::prevent_extensions(&object);
        let mut writer = StructSerializer {
            obj: object.clone(),
            defaults: Defaults::KeepPresent,
        };
        assert!(!js_sys::Reflect::set(&object, &"controlledField".into(), &JsValue::TRUE).unwrap());
        writer.serialize_field("controlled_field", &true).unwrap();
        assert!(!js_sys::Reflect::has(&object, &"controlledField".into()).unwrap());
    }

    #[test]
    fn nested_presence_large_integer_map_and_bytes_keep_their_representations() {
        use super::{i64_to_long_parts, to_js_value_camel_preserve_top_level_presence};
        #[derive(Serialize)]
        struct Nested {
            supplied_zero: Option<u32>,
            supplied_false: Option<bool>,
            supplied_empty: Option<String>,
            absent: Option<u32>,
            large: i64,
            bytes: Vec<u8>,
            empty_bytes: Vec<u8>,
            future_map: std::collections::BTreeMap<String, Option<bool>>,
        }
        #[derive(Serialize)]
        struct Outer {
            nested: Nested,
        }
        let value = Outer {
            nested: Nested {
                supplied_zero: Some(0),
                supplied_false: Some(false),
                supplied_empty: Some(String::new()),
                absent: None,
                large: i64::MAX,
                bytes: vec![1, 255],
                empty_bytes: Vec::new(),
                future_map: [("unknown_name".to_owned(), None)].into_iter().collect(),
            },
        };
        let skipped = field(&to_js_value_camel(&value).unwrap(), "nested");
        let kept = field(
            &to_js_value_camel_preserve_top_level_defaults(&value).unwrap(),
            "nested",
        );
        let present = field(
            &to_js_value_camel_preserve_top_level_presence(&value).unwrap(),
            "nested",
        );
        for object in [&skipped, &kept] {
            assert!(field(object, "suppliedZero").is_undefined());
            assert!(field(object, "suppliedFalse").is_undefined());
            assert!(field(object, "suppliedEmpty").is_undefined());
        }
        assert_eq!(field(&present, "suppliedZero").as_f64(), Some(0.0));
        assert_eq!(field(&present, "suppliedFalse").as_bool(), Some(false));
        assert_eq!(
            field(&present, "suppliedEmpty").as_string().as_deref(),
            Some("")
        );
        for object in [&skipped, &kept, &present] {
            assert!(!js_sys::Reflect::has(object, &"absent".into()).unwrap());
            assert!(!js_sys::Reflect::has(object, &"emptyBytes".into()).unwrap());
            let (low, high) = i64_to_long_parts(i64::MAX);
            let large = field(object, "large");
            assert_eq!(field(&large, "low").as_f64(), Some(low as f64));
            assert_eq!(field(&large, "high").as_f64(), Some(high as f64));
            assert_eq!(field(&large, "unsigned").as_bool(), Some(false));
            let bytes = field(object, "bytes");
            assert!(bytes.is_instance_of::<js_sys::Uint8Array>());
            assert_eq!(js_sys::Uint8Array::from(bytes).to_vec(), vec![1, 255]);
            assert!(field(&field(object, "futureMap"), "unknown_name").is_null());
        }
    }

    #[test]
    fn empty_nonbyte_first_and_byte_promotion_keep_order_and_representation() {
        let empty = to_js_value_camel(&Vec::<u8>::new()).unwrap();
        assert!(empty.is_instance_of::<js_sys::Array>());
        assert_eq!(js_sys::Array::from(&empty).length(), 0);
        for (values, bytes) in [
            (vec![0i32, 255], true),
            (vec![-1i32, 0, 1], false),
            (vec![1i32, 2, 256, 3], false),
        ] {
            let encoded = to_js_value_camel(&values).unwrap();
            assert_eq!(encoded.is_instance_of::<js_sys::Uint8Array>(), bytes);
            if bytes {
                assert_eq!(js_sys::Uint8Array::from(encoded).to_vec(), vec![0, 255]);
            } else {
                assert!(encoded.is_instance_of::<js_sys::Array>());
                let array = js_sys::Array::from(&encoded);
                assert_eq!(array.length(), values.len() as u32);
                for (index, value) in values.into_iter().enumerate() {
                    assert_eq!(array.get(index as u32).as_f64(), Some(value as f64));
                }
            }
        }
        let fractional = js_sys::Array::from(&to_js_value_camel(&vec![1.0, 1.5, 2.0]).unwrap());
        assert_eq!(fractional.length(), 3);
        assert_eq!(fractional.get(0).as_f64(), Some(1.0));
        assert_eq!(fractional.get(1).as_f64(), Some(1.5));
        assert_eq!(fractional.get(2).as_f64(), Some(2.0));
    }

    #[test]
    fn sequence_hint_growth_and_promotion_match_the_existing_vec_policy() {
        use super::{Defaults, SeqItems, SeqSerializer};
        use serde::ser::SerializeSeq as _;
        let mut writer = SeqSerializer {
            items: SeqItems::Unknown { capacity: 7 },
            defaults: Defaults::KeepPresent,
        };
        let mut bytes = Vec::with_capacity(7);
        for byte in 0u8..10 {
            bytes.push(byte);
            writer.serialize_element(&byte).unwrap();
            match &writer.items {
                SeqItems::Bytes(actual) => {
                    assert_eq!(actual, &bytes);
                    assert_eq!(actual.capacity(), bytes.capacity());
                }
                _ => panic!("all byte values remain bytes"),
            }
        }
        let mut values = Vec::with_capacity(bytes.capacity().max(bytes.len() + 1));
        values.extend(bytes.into_iter().map(|byte| JsValue::from_f64(byte as f64)));
        values.push(JsValue::from_f64(256.0));
        writer.serialize_element(&256u16).unwrap();
        for number in 257u16..270 {
            match &writer.items {
                SeqItems::Values(actual) => {
                    assert_eq!(actual, &values);
                    assert_eq!(actual.capacity(), values.capacity());
                }
                _ => panic!("the first non-byte promotes prior byte values"),
            }
            values.push(JsValue::from_f64(number as f64));
            writer.serialize_element(&number).unwrap();
        }
        let array = js_sys::Array::from(&writer.end().unwrap());
        assert_eq!(array.length(), values.len() as u32);
        for (index, value) in values.into_iter().enumerate() {
            assert_eq!(array.get(index as u32), value);
        }
    }

    #[test]
    fn sequence_value_failure_evaluates_once_and_does_not_advance_state() {
        use super::{Defaults, SeqItems, SeqSerializer};
        use serde::ser::SerializeSeq as _;
        let calls = std::cell::Cell::new(0);
        let mut writer = SeqSerializer {
            items: SeqItems::Unknown { capacity: 7 },
            defaults: Defaults::KeepPresent,
        };
        let error = writer
            .serialize_element(&CountedBool {
                calls: &calls,
                fail: true,
            })
            .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(error.to_string(), "value control failure");
        assert!(matches!(writer.items, SeqItems::Unknown { capacity: 7 }));
        writer
            .serialize_element(&CountedBool {
                calls: &calls,
                fail: false,
            })
            .unwrap();
        assert_eq!(calls.get(), 2);
        let error = writer
            .serialize_element(&CountedBool {
                calls: &calls,
                fail: true,
            })
            .unwrap_err();
        assert_eq!(calls.get(), 3);
        assert_eq!(error.to_string(), "value control failure");
        match &writer.items {
            SeqItems::Values(values) => {
                assert_eq!(values.len(), 1);
                assert_eq!(values.capacity(), 7);
                assert_eq!(values[0].as_bool(), Some(false));
            }
            _ => panic!("failure leaves the existing value buffer intact"),
        }
    }

    #[test]
    fn tuple_mixed_and_nested_sequences_keep_their_distinct_representations() {
        let bytes = to_js_value_camel(&(1u8, 2u8)).unwrap();
        assert!(bytes.is_instance_of::<js_sys::Uint8Array>());
        assert_eq!(js_sys::Uint8Array::from(bytes).to_vec(), vec![1, 2]);
        let mixed = to_js_value_camel(&(1u8, "tail", false)).unwrap();
        assert!(mixed.is_instance_of::<js_sys::Array>());
        let array = js_sys::Array::from(&mixed);
        assert_eq!(array.length(), 3);
        assert_eq!(array.get(0).as_f64(), Some(1.0));
        assert_eq!(array.get(1).as_string().as_deref(), Some("tail"));
        assert_eq!(array.get(2).as_bool(), Some(false));
        let nested = to_js_value_camel(&vec![Vec::<u8>::new(), vec![1, 255]]).unwrap();
        assert!(nested.is_instance_of::<js_sys::Array>());
        let array = js_sys::Array::from(&nested);
        assert_eq!(array.length(), 2);
        assert!(array.get(0).is_instance_of::<js_sys::Array>());
        assert_eq!(js_sys::Array::from(&array.get(0)).length(), 0);
        assert!(array.get(1).is_instance_of::<js_sys::Uint8Array>());
        assert_eq!(
            js_sys::Uint8Array::from(array.get(1)).to_vec(),
            vec![1, 255]
        );
    }

    #[test]
    fn sequence_state_preserves_byte_and_array_representations() {
        let serialized = to_js_value_camel(&Sequences {
            bytes: vec![1, 2, 255],
            wider_numbers: vec![1, 256],
        })
        .expect("sequence serialization");

        let bytes = field(&serialized, "bytes");
        assert!(bytes.is_instance_of::<js_sys::Uint8Array>());
        assert_eq!(js_sys::Uint8Array::from(bytes).to_vec(), vec![1, 2, 255]);

        let wider = js_sys::Array::from(&field(&serialized, "widerNumbers"));
        assert_eq!(wider.length(), 2);
        assert_eq!(wider.get(0).as_f64(), Some(1.0));
        assert_eq!(wider.get(1).as_f64(), Some(256.0));
    }
}
