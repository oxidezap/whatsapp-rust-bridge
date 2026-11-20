use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

pub(crate) async fn resolve_connection(value: JsValue) -> Result<JsValue, JsValue> {
    let promise = js_sys::Promise::resolve(&value);
    let mut pending = PendingConnect(Some(promise.clone()));
    let result = JsFuture::from(promise).await;
    pending.0.take();
    result
}

pub(crate) fn observe_close(result: JsValue) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let _ = JsFuture::from(js_sys::Promise::resolve(&result)).await;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = result;
}

pub(crate) fn close_detached(close: &js_sys::Function, receiver: &JsValue) {
    if let Ok(result) = close.call0(receiver) {
        observe_close(result);
    }
}

// The late-close callback must not retain relay state after cancellation.
struct PendingConnect(Option<js_sys::Promise>);

impl Drop for PendingConnect {
    fn drop(&mut self) {
        let Some(promise) = self.0.take() else { return };
        let close = Closure::wrap(Box::new(|value: JsValue| {
            if let Ok(close) = js_sys::Reflect::get(&value, &"close".into())
                && let Ok(close) = close.dyn_into::<js_sys::Function>()
            {
                close_detached(&close, &value);
            }
        }) as Box<dyn FnMut(JsValue)>)
        .into_js_value();
        let ignore =
            Closure::wrap(Box::new(|_: JsValue| {}) as Box<dyn FnMut(JsValue)>).into_js_value();
        if let Ok(then) = js_sys::Reflect::get(promise.as_ref(), &"then".into())
            && let Ok(then) = then.dyn_into::<js_sys::Function>()
        {
            let _ = then.call2(promise.as_ref(), &close, &ignore);
        }
    }
}
