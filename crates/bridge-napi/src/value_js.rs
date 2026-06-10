//! `BridgeValue` → JS materialization (runs on the JS thread, has `Env`).
//!
//! Reproduces the WASM serializer's JS shapes: `Long` → `{low,high,unsigned}`,
//! `Bytes` → `Uint8Array`, 64-bit number-or-string fallback for `I64`/`U64`.
//! Reused by the event sink and every dynamic-value-returning client method.

use napi::bindgen_prelude::*;

use bridge_core::value::{BridgeValue, JS_MAX_SAFE_INT};

/// Newtype so we can implement the foreign `ToNapiValue` trait for the foreign
/// `BridgeValue` (orphan rule).
pub struct JsBridgeValue(pub BridgeValue);

impl ToNapiValue for JsBridgeValue {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
        build(env, val.0)
    }
}

fn build(env: sys::napi_env, v: BridgeValue) -> Result<sys::napi_value> {
    let e = unsafe { Env::from_raw(env) };
    match v {
        BridgeValue::Null => unsafe { ToNapiValue::to_napi_value(env, Null) },
        BridgeValue::Bool(b) => unsafe { ToNapiValue::to_napi_value(env, b) },
        BridgeValue::F64(f) => unsafe { ToNapiValue::to_napi_value(env, f) },
        BridgeValue::I64(i) => {
            if i.unsigned_abs() <= JS_MAX_SAFE_INT {
                unsafe { ToNapiValue::to_napi_value(env, i as f64) }
            } else {
                unsafe { ToNapiValue::to_napi_value(env, i.to_string()) }
            }
        }
        BridgeValue::U64(u) => {
            if u <= JS_MAX_SAFE_INT {
                unsafe { ToNapiValue::to_napi_value(env, u as f64) }
            } else {
                unsafe { ToNapiValue::to_napi_value(env, u.to_string()) }
            }
        }
        BridgeValue::Long(p) => {
            let mut obj = Object::new(&e)?;
            obj.set_named_property("low", p.low)?;
            obj.set_named_property("high", p.high)?;
            obj.set_named_property("unsigned", p.unsigned)?;
            unsafe { ToNapiValue::to_napi_value(env, obj) }
        }
        BridgeValue::Str(s) => unsafe { ToNapiValue::to_napi_value(env, s) },
        BridgeValue::Bytes(b) => unsafe { ToNapiValue::to_napi_value(env, Uint8Array::from(b)) },
        BridgeValue::Array(items) => {
            let mut arr = e.create_array(items.len() as u32)?;
            for (i, item) in items.into_iter().enumerate() {
                arr.set(i as u32, JsBridgeValue(item))?;
            }
            unsafe { ToNapiValue::to_napi_value(env, arr) }
        }
        BridgeValue::Object(fields) => {
            let mut obj = Object::new(&e)?;
            for (k, val) in fields {
                obj.set_named_property(&k, JsBridgeValue(val))?;
            }
            unsafe { ToNapiValue::to_napi_value(env, obj) }
        }
    }
}
