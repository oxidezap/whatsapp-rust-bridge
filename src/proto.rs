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
    match val.serialize(&serializer) {
        Ok(v) => Ok(v),
        // serde_wasm_bindgen ERRORS (not lossy) on a 64-bit value outside the JS
        // safe-integer range — e.g. a business `verified_name` certificate serial
        // like 2310452236816384026. Without a fallback the WHOLE event fails to
        // serialize and is dropped (the message never reaches the consumer).
        // Re-serialize via serde_json, emitting only the out-of-range integers as
        // strings (protobufjs/Baileys already treat huge ids as strings/Longs);
        // every other field keeps its exact prior shape, incl. plain `number`s
        // for the in-range 64-bit fields that `asNumber`-style consumers read.
        Err(_) => {
            let json = serde_json::to_value(val).map_err(|e| JsValue::from_str(&e.to_string()))?;
            json_to_js(&json)
        }
    }
}

/// JS `Number.MAX_SAFE_INTEGER`. Integers with magnitude beyond this lose
/// precision as `f64`, so the fallback emits them as strings instead.
const JS_MAX_SAFE_INT: u64 = 9_007_199_254_740_991;

/// Convert a `serde_json::Value` to a `JsValue`, mirroring the
/// `serde_wasm_bindgen` output (numbers stay numbers, maps are objects) EXCEPT
/// that integers outside the JS safe range become strings instead of crashing.
/// Only reached on the rare large-integer fallback in [`to_js_value`].
fn json_to_js(v: &serde_json::Value) -> Result<JsValue, JsValue> {
    use serde_json::Value;
    Ok(match v {
        Value::Null => JsValue::NULL,
        Value::Bool(b) => JsValue::from_bool(*b),
        Value::String(s) => JsValue::from_str(s),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                if u <= JS_MAX_SAFE_INT {
                    JsValue::from_f64(u as f64)
                } else {
                    JsValue::from_str(&u.to_string())
                }
            } else if let Some(i) = n.as_i64() {
                if i.unsigned_abs() <= JS_MAX_SAFE_INT {
                    JsValue::from_f64(i as f64)
                } else {
                    JsValue::from_str(&i.to_string())
                }
            } else {
                JsValue::from_f64(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::Array(a) => {
            let arr = js_sys::Array::new();
            for item in a {
                arr.push(&json_to_js(item)?);
            }
            arr.into()
        }
        Value::Object(o) => {
            let obj = js_sys::Object::new();
            for (k, val) in o {
                js_sys::Reflect::set(&obj, &JsValue::from_str(k), &json_to_js(val)?)?;
            }
            obj.into()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // Alias `#[test]` -> wasm_bindgen_test so these run on wasm32 via
    // `wasm-pack test --node` (the crate has no native test target).
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[derive(serde::Serialize)]
    struct Info {
        // A business `verified_name` certificate serial — out of JS safe range.
        verified_name_serial: i64,
        // A microsecond timestamp — large but within JS safe range.
        server_timestamp_us: i64,
        push_name: String,
    }

    #[test]
    fn out_of_range_i64_serializes_as_string_instead_of_crashing() {
        let js = to_js_value(&Info {
            verified_name_serial: 2310452236816384026,
            server_timestamp_us: 1780439000381003,
            push_name: "x".into(),
        })
        .expect("must not fail (the bug dropped the whole event here)");

        // Out-of-range serial → string (was crashing serde_wasm_bindgen).
        let serial = js_sys::Reflect::get(&js, &JsValue::from_str("verified_name_serial")).unwrap();
        assert_eq!(serial.as_string().as_deref(), Some("2310452236816384026"));

        // In-range 64-bit stays a plain number (asNumber-style consumers rely on it).
        let ts = js_sys::Reflect::get(&js, &JsValue::from_str("server_timestamp_us")).unwrap();
        assert_eq!(ts.as_f64(), Some(1780439000381003.0));

        // Other fields unchanged.
        let pn = js_sys::Reflect::get(&js, &JsValue::from_str("push_name")).unwrap();
        assert_eq!(pn.as_string().as_deref(), Some("x"));
    }
}
