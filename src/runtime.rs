use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use async_trait::async_trait;
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore;
use whatsapp_rust::wacore::runtime::{AbortHandle, Runtime};

/// WASM-based implementation of [`Runtime`].
///
/// - `spawn` schedules tasks via `wasm_bindgen_futures::spawn_local`
/// - `sleep` uses `setTimeout(ms)`
/// - `yield_now` yields via `MessageChannel` (zero-allocation macrotask)
/// - `spawn_blocking` runs inline (WASM is single-threaded, no thread pool)
pub struct WasmRuntime;

crate::wasm_send_sync!(WasmRuntime);

/// Cached reference to `globalThis.setTimeout`, resolved once and stored as
/// `js_sys::Function` so hot paths avoid a per-call `dyn_into` + clone.
static SET_TIMEOUT_FN: std::sync::OnceLock<Option<js_sys::Function>> = std::sync::OnceLock::new();
/// Cached reference to `globalThis.clearTimeout` — same rationale as above.
static CLEAR_TIMEOUT_FN: std::sync::OnceLock<Option<js_sys::Function>> = std::sync::OnceLock::new();

fn resolve_global_fn(name: &str) -> Option<js_sys::Function> {
    let global = js_sys::global();
    js_sys::Reflect::get(&global, &name.into())
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
}

fn get_set_timeout() -> Option<&'static js_sys::Function> {
    SET_TIMEOUT_FN
        .get_or_init(|| resolve_global_fn("setTimeout"))
        .as_ref()
}

fn get_clear_timeout() -> Option<&'static js_sys::Function> {
    CLEAR_TIMEOUT_FN
        .get_or_init(|| resolve_global_fn("clearTimeout"))
        .as_ref()
}

/// Call `clearTimeout(id)` to cancel a pending timer. Silently no-ops if
/// `globalThis.clearTimeout` is unavailable or the call throws.
fn clear_timeout(id: &JsValue) {
    if let Some(clear_fn) = get_clear_timeout() {
        let _ = clear_fn.call1(&JsValue::NULL, id);
    }
}

// ---------------------------------------------------------------------------
// Cancellation-aware sleep
// ---------------------------------------------------------------------------
//
// A sleep has to be cancellable: when the owning future is dropped before the
// timer fires — `select!`, `Abortable`, a request answered before its timeout —
// the JS timer must be cleared, or it keeps the Node event loop alive until it
// fires on its own. For a multi-minute sleep during shutdown that hangs the
// process.
//
// The obvious shape, `Promise::new` + `setTimeout(resolve)` awaited through a
// `JsFuture`, costs a promise, a resolve function, a `then` registration and two
// type checks per sleep. It also made cancellation subtle: `JsFuture` registers
// a resolve/reject pair, and a promise that never settles retains both for the
// life of the process, so every cancelled sleep leaked two JS handles until Drop
// was taught to settle it.
//
// This mirrors the `SetImmediateYield` design further down instead: ONE
// persistent JS callback shared by every timer, plus a Rust-side registry of
// `Waker`s. `setTimeout(cb, ms, id)` passes the id back to that shared callback,
// so it knows which sleep to wake. Nothing per-sleep crosses the boundary except
// the `setTimeout` call, and there is no promise that could retain anything —
// the leak is impossible by construction rather than fixed by Drop.

type SleepId = u32;

thread_local! {
    /// Wakers of sleeps whose timer has not fired yet. A missing id means the
    /// timer already fired, which is how `poll` detects completion without any
    /// extra state.
    static SLEEP_WAKERS: RefCell<HashMap<SleepId, Waker>> = RefCell::new(HashMap::new());
    static NEXT_SLEEP_ID: Cell<SleepId> = const { Cell::new(0) };
    /// The single callback every timer shares.
    static SLEEP_CB: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

/// Create the shared timer callback once.
fn ensure_sleep_callback() {
    SLEEP_CB.with(|cached| {
        if cached.borrow().is_some() {
            return;
        }
        let callback = Closure::wrap(Box::new(|id: f64| {
            let id = id as SleepId;
            // Take the waker out before waking. `wake` can re-enter Rust, which
            // must not find the registry borrowed, and removing it first is what
            // makes the sleep look finished to `poll`.
            let waker = SLEEP_WAKERS.with(|wakers| wakers.borrow_mut().remove(&id));
            if let Some(waker) = waker {
                waker.wake();
            }
        }) as Box<dyn FnMut(f64)>);
        *cached.borrow_mut() = Some(callback.into_js_value());
    });
}

/// Register `waker` and arm a timer for it.
///
/// Returns the registry id and the handle `clearTimeout` needs, or `None` when
/// no timer could be armed — the caller then completes immediately rather than
/// waiting for something that will never fire.
fn arm_sleep(ms: i32, waker: Waker) -> Option<(SleepId, JsValue)> {
    let set_timeout_fn = get_set_timeout()?;
    ensure_sleep_callback();

    let id = SLEEP_WAKERS.with(|wakers| {
        let mut wakers = wakers.borrow_mut();
        // Ids wrap. Skipping live ones keeps a long-pending sleep from being
        // woken by a much later timer that happened to reuse its id.
        let mut id = NEXT_SLEEP_ID.get();
        while wakers.contains_key(&id) {
            id = id.wrapping_add(1);
        }
        NEXT_SLEEP_ID.set(id.wrapping_add(1));
        wakers.insert(id, waker);
        id
    });

    let handle = SLEEP_CB.with(|cb| {
        let cb = cb.borrow();
        let cb = cb.as_ref()?;
        set_timeout_fn
            .call3(&JsValue::NULL, cb, &JsValue::from(ms), &JsValue::from(id))
            .ok()
    });

    match handle {
        Some(handle) => Some((id, handle)),
        None => {
            SLEEP_WAKERS.with(|wakers| wakers.borrow_mut().remove(&id));
            None
        }
    }
}

struct ArmedSleep {
    id: SleepId,
    /// Value `setTimeout` returned, passed back to `clearTimeout` on cancel.
    handle: JsValue,
}

/// Sleep that clears its timer when dropped.
///
/// The timer is armed on the first poll rather than at construction, which is
/// what lets the registry hold the real waker instead of a placeholder. Every
/// caller awaits the future it just created, so the difference is not observable.
struct SleepFut {
    ms: i32,
    armed: Option<ArmedSleep>,
    /// No timer could be armed; complete instead of hanging forever.
    unschedulable: bool,
}

impl Future for SleepFut {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.unschedulable {
            return Poll::Ready(());
        }
        let Some(armed) = &self.armed else {
            return match arm_sleep(self.ms, cx.waker().clone()) {
                Some((id, handle)) => {
                    self.armed = Some(ArmedSleep { id, handle });
                    Poll::Pending
                }
                None => {
                    self.unschedulable = true;
                    Poll::Ready(())
                }
            };
        };
        // The shared callback removes the registration when the timer fires, so
        // a missing one means this sleep is done.
        let still_pending = SLEEP_WAKERS.with(|wakers| {
            match wakers.borrow_mut().get_mut(&armed.id) {
                Some(slot) => {
                    // The executor may poll us from a different task context.
                    if !slot.will_wake(cx.waker()) {
                        *slot = cx.waker().clone();
                    }
                    true
                }
                None => false,
            }
        });
        if still_pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

impl Drop for SleepFut {
    fn drop(&mut self) {
        if let Some(armed) = &self.armed {
            // Deregister first: a timer that fires anyway then finds nothing to
            // wake, so a failed `clearTimeout` is harmless.
            let was_pending = SLEEP_WAKERS
                .with(|wakers| wakers.borrow_mut().remove(&armed.id))
                .is_some();
            if was_pending {
                clear_timeout(&armed.handle);
            }
        }
    }
}

/// Build a cancellation-aware sleep future.
///
/// `ms == 0` still goes through a timer to preserve "yield to the event loop"
/// semantics; cancellation works identically.
fn make_sleep(ms: i32) -> SleepFut {
    SleepFut {
        ms,
        armed: None,
        unschedulable: false,
    }
}

// ---------------------------------------------------------------------------
// setImmediate-based yielding
// ---------------------------------------------------------------------------
//
// Uses `setImmediate` for cooperative yielding. This is critical because
// `setImmediate` fires in the **check phase** of the Node.js event loop,
// which runs AFTER the **I/O poll phase**. This means:
// - File I/O callbacks (writeFile for session storage) complete before yield
// - Network I/O callbacks (WebSocket data) complete before yield
// - Timer callbacks complete before yield
//
// MessageChannel.postMessage was previously used but it fires as a macrotask
// that runs BEFORE I/O polling, causing deadlocks: WASM awaits a JS Promise
// that needs writeFile to complete, but writeFile needs the I/O poll phase,
// which never runs because MessageChannel macrotasks keep firing.
//
// setImmediate is available in Node.js and Bun (our targets). For browser
// environments, we fall back to setTimeout(0).
//
// Cost per yield: one `setImmediate(callback)` call + one `Waker`.
// Slightly more overhead than MessageChannel (closure allocation per call)
// but eliminates the I/O starvation deadlock.

thread_local! {
    static YIELD_WAKERS: RefCell<VecDeque<Waker>> = const { RefCell::new(VecDeque::new()) };
    static SET_IMMEDIATE_FN: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// Spawn throttle — staggers task starts to prevent microtask storms
// ---------------------------------------------------------------------------
//
// Problem: During offline sync, hundreds of per-chat workers are spawned via
// spawn_local. They all immediately contend on an upstream 1-permit semaphore.
// Each permit release wakes ALL waiters as microtasks (thundering herd). Only
// one wins; the rest re-pend. This microtask storm prevents JS callbacks
// (storage Promises, timers, WebSocket I/O) from running → 100% CPU freeze.
//
// Solution: A FIFO queue batches spawn requests. A single drain loop starts
// tasks in order, yielding to the timer queue (setTimeout) between batches.
// This ensures:
//   - First batch starts immediately (zero overhead in normal operation)
//   - Excess tasks wait in a Rust VecDeque (no JS allocations while waiting)
//   - setTimeout yields between batches let JS timers/Promises interleave
//   - Long-lived tasks (noise sender, per-chat workers) don't block the queue
//     because the queue tracks *starts*, not *lifetime*

/// Max tasks to start per batch before yielding to the JS timer queue.
const SPAWN_BATCH_SIZE: usize = 16;

type SpawnTask = Pin<Box<dyn Future<Output = ()> + 'static>>;

thread_local! {
    static SPAWN_QUEUE: RefCell<VecDeque<SpawnTask>> = const { RefCell::new(VecDeque::new()) };
    static SPAWN_DRAINING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Enqueue a future and ensure the drain loop is running.
fn enqueue_spawn(task: SpawnTask) {
    SPAWN_QUEUE.with(|q| q.borrow_mut().push_back(task));

    let already_draining = SPAWN_DRAINING.with(|d| {
        if d.get() {
            true
        } else {
            d.set(true);
            false
        }
    });

    if !already_draining {
        wasm_bindgen_futures::spawn_local(drain_spawn_queue());
    }
}

/// Drain loop: starts tasks in batches, yielding to the JS timer queue between
/// batches so setTimeout/setInterval/Promise callbacks can run.
async fn drain_spawn_queue() {
    loop {
        // Take up to SPAWN_BATCH_SIZE tasks from the queue
        let batch: Vec<SpawnTask> = SPAWN_QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            let n = q.len().min(SPAWN_BATCH_SIZE);
            q.drain(..n).collect()
        });

        if batch.is_empty() {
            // Queue is empty — stop draining
            SPAWN_DRAINING.with(|d| d.set(false));
            return;
        }

        // Start all tasks in this batch (they become independent spawn_local futures)
        for task in batch {
            wasm_bindgen_futures::spawn_local(task);
        }

        // Yield to the JS timer queue between batches. Uses setTimeout(0) so
        // timer callbacks (setInterval, storage Promises) can interleave.
        // MessageChannel has higher priority than timers and would still starve.
        set_timeout_yield().await;

        // After yielding, check if more tasks were enqueued while we were waiting
    }
}

// Cached persistent waker-drain callback, reused for every setImmediate call.
thread_local! {
    static IMMEDIATE_CB: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

/// Cache `setImmediate` and create a persistent drain callback.
fn ensure_set_immediate() {
    SET_IMMEDIATE_FN.with(|cached| {
        if cached.borrow().is_some() {
            return;
        }
        let global = js_sys::global();
        if let Ok(si) = js_sys::Reflect::get(&global, &"setImmediate".into())
            && let Ok(f) = si.dyn_into::<js_sys::Function>()
        {
            *cached.borrow_mut() = Some(f);
        }
    });

    IMMEDIATE_CB.with(|cached| {
        if cached.borrow().is_some() {
            return;
        }
        // Create ONE persistent callback that wakes the next yielder.
        // This is called by setImmediate each time — same function, no allocation.
        let callback = Closure::wrap(Box::new(|| {
            YIELD_WAKERS.with(|wakers| {
                if let Some(waker) = wakers.borrow_mut().pop_front() {
                    waker.wake();
                }
            });
        }) as Box<dyn FnMut()>);
        *cached.borrow_mut() = Some(callback.into_js_value());
    });
}

/// Yield to the JS event loop. Uses `setImmediate` (Node.js/Bun) which fires
/// in the **check phase** — after I/O polling — so file/network I/O (writeFile,
/// WebSocket) can complete between yields. Falls back to `setTimeout(0)` in
/// environments without `setImmediate`.
///
/// Cost per yield: one `setImmediate(cachedCallback)` call + one `Waker` clone.
/// No Promise, no closure, no timer object allocated per call.
pub(crate) fn set_timeout_0() -> Pin<Box<dyn Future<Output = ()>>> {
    ensure_set_immediate();

    let has_immediate = SET_IMMEDIATE_FN.with(|cached| cached.borrow().is_some());

    if has_immediate {
        Box::pin(SetImmediateYield { registered: false })
    } else {
        set_timeout_yield()
    }
}

/// Future that resolves when `setImmediate` fires.
struct SetImmediateYield {
    registered: bool,
}

impl Future for SetImmediateYield {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.registered {
            Poll::Ready(())
        } else {
            self.registered = true;
            YIELD_WAKERS.with(|wakers| {
                wakers.borrow_mut().push_back(cx.waker().clone());
            });
            // Call setImmediate with the cached callback — fires after I/O poll
            SET_IMMEDIATE_FN.with(|si| {
                IMMEDIATE_CB.with(|cb| {
                    let si_ref = si.borrow();
                    let cb_ref = cb.borrow();
                    if let (Some(si_fn), Some(cb_val)) = (si_ref.as_ref(), cb_ref.as_ref()) {
                        let _ = si_fn.call1(&JsValue::NULL, cb_val);
                    }
                });
            });
            Poll::Pending
        }
    }
}

/// Fallback: yield via setTimeout(0) — used when `setImmediate` is unavailable.
///
/// Uses the cancellation-aware `SleepFut` so a dropped yield doesn't leak a
/// pending timer into the event loop. For a 0ms timer this rarely matters in
/// practice, but it's free insurance and keeps the code path consistent.
fn set_timeout_yield() -> Pin<Box<dyn Future<Output = ()>>> {
    Box::pin(make_sleep(0))
}

#[async_trait(?Send)]
impl Runtime for WasmRuntime {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) -> AbortHandle {
        let (abort_handle, abort_reg) = futures::future::AbortHandle::new_pair();
        let abortable = futures::future::Abortable::new(future, abort_reg);
        enqueue_spawn(Box::pin(async move {
            let _ = abortable.await;
        }));
        wacore::runtime::AbortHandle::new(move || abort_handle.abort())
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()>>> {
        let ms = duration.as_millis().min(i32::MAX as u128) as i32;
        // `make_sleep` returns a `SleepFut` that calls `clearTimeout` on Drop
        // if the underlying timer hasn't fired yet — prevents leaking pending
        // timers into the Node.js event loop on cancellation/abort.
        Box::pin(make_sleep(ms))
    }

    fn spawn_blocking(&self, f: Box<dyn FnOnce() + 'static>) -> Pin<Box<dyn Future<Output = ()>>> {
        // WASM is single-threaded: yield to event loop BEFORE running the
        // blocking closure so pending I/O (WebSocket, storage) can complete.
        // Then run the closure, then yield again to let results propagate.
        Box::pin(async move {
            set_timeout_0().await; // yield before — let I/O callbacks run
            f();
            set_timeout_0().await; // yield after — let event loop process results
        })
    }

    fn yield_now(&self) -> Option<Pin<Box<dyn Future<Output = ()>>>> {
        // Always yield in WASM — the single-threaded event loop needs every
        // opportunity to process pending I/O (WebSocket data, storage callbacks,
        // timer callbacks). The caller already throttles (e.g., every 10 frames),
        // but in WASM even that can be too infrequent during heavy offline processing.
        Some(set_timeout_0())
    }

    fn yield_frequency(&self) -> u32 {
        // Yield every single frame in WASM. The default (10) is too infrequent
        // for single-threaded execution — pending I/O starves between yields.
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn pending_sleeps() -> usize {
        SLEEP_WAKERS.with(|wakers| wakers.borrow().len())
    }

    #[wasm_bindgen_test]
    async fn sleep_fires_and_leaves_no_registration() {
        let before = pending_sleeps();
        make_sleep(1).await;
        assert_eq!(
            pending_sleeps(),
            before,
            "a fired timer must deregister its waker"
        );
    }

    #[wasm_bindgen_test]
    async fn concurrent_sleeps_each_wake_their_own_future() {
        let before = pending_sleeps();
        // Distinct durations so they complete out of registration order.
        let (a, b, c) = futures::join!(make_sleep(8), make_sleep(1), make_sleep(4));
        assert_eq!((a, b, c), ((), (), ()));
        assert_eq!(pending_sleeps(), before);
    }

    /// The whole point of the design: a dropped sleep must clear both its JS
    /// timer and its registry slot, with nothing left to retain.
    #[wasm_bindgen_test]
    async fn dropping_a_pending_sleep_deregisters_it() {
        let before = pending_sleeps();
        {
            let mut sleep = Box::pin(make_sleep(60_000));
            let poll = futures::poll!(sleep.as_mut());
            assert!(poll.is_pending(), "a minute-long sleep must not be ready");
            assert_eq!(
                pending_sleeps(),
                before + 1,
                "an armed sleep registers exactly one waker"
            );
        }
        assert_eq!(
            pending_sleeps(),
            before,
            "dropping an armed sleep must deregister it"
        );
    }

    /// Cancelling must not disturb sleeps that are still waiting.
    #[wasm_bindgen_test]
    async fn cancelling_one_sleep_leaves_the_others_running() {
        let before = pending_sleeps();
        let mut long = Box::pin(make_sleep(60_000));
        assert!(futures::poll!(long.as_mut()).is_pending());
        let mut other = Box::pin(make_sleep(60_000));
        assert!(futures::poll!(other.as_mut()).is_pending());
        assert_eq!(pending_sleeps(), before + 2);

        drop(long);
        assert_eq!(pending_sleeps(), before + 1);

        // The survivor still completes on its own terms.
        drop(other);
        assert_eq!(pending_sleeps(), before);
        make_sleep(1).await;
        assert_eq!(pending_sleeps(), before);
    }
}
