//! The engine-side runtime: a single-threaded `wacore::runtime::Runtime`.
//!
//! The bridge holds the same idea in `src/runtime.rs`; this copy exists so the
//! engine crate never names the bridge. `spawn` schedules through
//! `wasm_bindgen_futures::spawn_local`, `sleep` arms `setTimeout`, and the
//! yielding policy matches the bridge's: every iteration, because a
//! single-threaded loop starves I/O otherwise.
//!
//! `wacore::runtime::Runtime` has two shapes: the wasm32 one is `?Send`, the
//! host one demands `Send`. This crate ships on wasm32; the host impl below
//! exists only so the engine proof test (`call::tests`, `cargo test` on the
//! host) can drive the same `run_call` with the same types. Both impls share
//! the `setTimeout` sleep — which resolves immediately without a JS global —
//! and differ only in how they park the spawned task.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use wacore::runtime::{AbortHandle, Runtime};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The engine crate's executor: `spawn_local` plus a `setTimeout` sleep.
pub struct EngineRuntime;

#[cfg(target_arch = "wasm32")]
unsafe impl Send for EngineRuntime {}
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for EngineRuntime {}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl Runtime for EngineRuntime {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) -> AbortHandle {
        let (abort_handle, abort_reg) = futures::future::AbortHandle::new_pair();
        let abortable = futures::future::Abortable::new(future, abort_reg);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = abortable.await;
        });
        AbortHandle::new(move || abort_handle.abort())
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()>>> {
        let ms = duration.as_millis().min(i32::MAX as u128) as i32;
        Box::pin(set_timeout_future(ms.max(0)))
    }

    fn spawn_blocking(&self, f: Box<dyn FnOnce() + 'static>) -> Pin<Box<dyn Future<Output = ()>>> {
        Box::pin(async move {
            set_timeout_future(0).await;
            f();
            set_timeout_future(0).await;
        })
    }

    fn yield_now(&self) -> Option<Pin<Box<dyn Future<Output = ()>>>> {
        Some(Box::pin(set_timeout_future(0)))
    }

    fn yield_frequency(&self) -> u32 {
        1
    }
}

/// Host twin of [`EngineRuntime`]: parks spawned tasks instead of driving
/// them, which is all the proof test needs — it `block_on`s `run_call`
/// itself and never goes through `spawn`. Without this impl the crate would
/// not compile for `cargo test` on the host at all.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl Runtime for EngineRuntime {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> AbortHandle {
        drop(future);
        AbortHandle::noop()
    }

    fn sleep(&self, _duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn spawn_blocking(
        &self,
        _f: Box<dyn FnOnce() + Send + 'static>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn yield_now(&self) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        None
    }

    fn yield_frequency(&self) -> u32 {
        1
    }
}

/// Spawns the drive loop on the engine runtime and returns its abort
/// handle. Dropping the handle aborts the task, which drops the transport
/// `Arc` and closes the relay channel with it.
#[cfg(target_arch = "wasm32")]
pub fn spawn_drive(future: impl std::future::Future<Output = ()> + 'static) -> AbortHandle {
    EngineRuntime.spawn(Box::pin(future))
}

/// A cancellable `setTimeout` sleep shared by the runtime and the stats fan.
/// Dropping it before the timer fires clears the JS timer, so an aborted
/// drive task never keeps the host event loop alive behind it.
#[cfg(target_arch = "wasm32")]
pub async fn set_timeout_future(ms: i32) {
    struct Sleep {
        id: Option<JsValue>,
        done: bool,
        ms: i32,
    }
    impl Drop for Sleep {
        fn drop(&mut self) {
            if !self.done
                && let Some(id) = self.id.take()
            {
                CLEAR_TIMEOUT.with(|slot| {
                    if slot.borrow().is_none() {
                        *slot.borrow_mut() = global_fn("clearTimeout");
                    }
                    if let Some(clear) = slot.borrow().as_ref() {
                        let _ = clear.call1(&JsValue::NULL, &id);
                    }
                });
            }
        }
    }
    impl Future for Sleep {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
            let this = self.get_mut();
            if this.done {
                return std::task::Poll::Ready(());
            }
            if this.id.is_some() {
                return std::task::Poll::Pending;
            }
            let waker = cx.waker().clone();
            let ms = this.ms;
            let closure = Closure::once(move || {
                waker.wake();
            });
            let id = SET_TIMEOUT.with(|slot| {
                if slot.borrow().is_none() {
                    *slot.borrow_mut() = global_fn("setTimeout");
                }
                let ms = JsValue::from(ms);
                slot.borrow().as_ref().and_then(|set| {
                    set.call2(&JsValue::NULL, closure.as_ref().unchecked_ref(), &ms)
                        .ok()
                })
            });
            closure.forget();
            match id {
                Some(id) => {
                    this.id = Some(id);
                    std::task::Poll::Pending
                }
                None => {
                    this.done = true;
                    std::task::Poll::Ready(())
                }
            }
        }
    }
    Sleep {
        id: None,
        done: false,
        ms,
    }
    .await;
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static SET_TIMEOUT: std::cell::RefCell<Option<js_sys::Function>> =
        const { std::cell::RefCell::new(None) };
    static CLEAR_TIMEOUT: std::cell::RefCell<Option<js_sys::Function>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_arch = "wasm32")]
fn global_fn(name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(&js_sys::global(), &name.into())
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
}
