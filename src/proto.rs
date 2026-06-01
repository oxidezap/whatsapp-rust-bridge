use wasm_bindgen::prelude::*;

/// Serialize a value to JsValue (snake_case-preserving path — used for the
/// `info` payload and the `serialize {}` event variants).
///
/// - 64-bit integer types (`i64`/`u64`) are emitted as plain JS `number`s, NOT
///   `BigInt`. `BigInt` cannot be serialized by `JSON.stringify` (it throws
///   `TypeError: cannot serialize BigInt`), and Baileys-style consumers
///   routinely `JSON.stringify(event)` for logging — a single `BigInt` field
///   crashes the whole serialization. The 64-bit fields that flow through this
///   path are bounded (microsecond timestamps, verified-name serials,
///   newsletter server ids / reaction counts), all comfortably under 2^53, so
///   `number` is lossless here. (The `message` proto payload, which can carry
///   genuinely large 64-bit ids, goes through `camel_serializer` instead, which
///   emits protobufjs-style `Long` objects — see that module.)
/// - `serialize_maps_as_objects(true)` makes any rust `serialize_map` (used
///   by hand-written `Serialize` impls — notably the `WireEnum` derive's
///   internally-tagged enum output) emit a plain JS `Object` instead of a
///   native JS `Map`. Plain objects round-trip through `JSON.stringify`,
///   support `obj.key` property access, and match what every downstream
///   adapter expects.
pub fn to_js_value<T: serde::Serialize>(val: &T) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new()
        .serialize_large_number_types_as_bigints(false)
        .serialize_maps_as_objects(true);
    val.serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
