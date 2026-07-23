//! Diagnostics-only WASM heap attribution.
//!
//! `WebAssembly.Memory.buffer.byteLength` tells us how many 64 KiB pages were
//! committed, but not how much requested Rust heap is still live or which
//! bridge phase caused allocator churn. A `memory-profiling` build installs the
//! counting allocator below and exposes allocation-free scoped counters for the
//! history-sync boundary. Production builds keep the same API with counters
//! disabled so consumers can feature-detect without conditional imports.

use serde::Serialize;
use tsify::Tsify;

#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct AllocationBucketSnapshot {
    pub allocated_bytes: f64,
    pub allocations: f64,
    pub largest_allocation_bytes: f64,
    /// WASM pages committed by allocator calls executed in this scope.
    pub linear_growth_bytes: f64,
    pub linear_growth_events: f64,
}

#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct HistorySyncAllocationSnapshot {
    pub events: f64,
    pub compressed_bytes: f64,
    pub decompressed_bytes: f64,
    pub conversations: f64,
    pub batches: f64,
    pub skipped_conversations: f64,
    pub largest_compressed_bytes: f64,
    pub largest_decompressed_bytes: f64,
    /// Compressed history events waiting in the bridge's ordered event queue.
    pub queued_events: f64,
    pub queued_compressed_bytes: f64,
    pub peak_queued_events: f64,
    pub peak_queued_compressed_bytes: f64,
    /// Dequeued history events still being decoded or synchronously delivered.
    pub active_events: f64,
    pub active_compressed_bytes: f64,
    pub peak_active_events: f64,
    pub peak_active_compressed_bytes: f64,
    /// Queue plus active bridge ownership. This attributes retention without
    /// guessing from allocator lifetime or duplicating core protocol logic.
    pub retained_events: f64,
    pub retained_compressed_bytes: f64,
    pub peak_retained_events: f64,
    pub peak_retained_compressed_bytes: f64,
}

/// Allocation scope active when an allocator call committed more WASM pages.
/// These are bridge boundaries, not WhatsApp protocol operation names.
#[derive(Debug, Clone, Copy, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
#[repr(usize)]
pub enum AllocationScope {
    Other = 0,
    CoreTask = 1,
    HistoryDecode = 2,
    HistorySerialize = 3,
    HistoryEnvelope = 4,
    HistoryCallback = 5,
    Diagnostics = 6,
}

/// Allocation totals for one tracing callsite emitted by whatsapp-rust itself.
/// Names and source locations come from `tracing::Metadata`; the bridge does
/// not maintain a parallel list of core operations.
#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct CoreSpanAllocationSnapshot {
    pub name: String,
    pub target: String,
    pub source_file: Option<String>,
    pub source_line: Option<f64>,
    pub allocated_bytes: f64,
    pub allocations: f64,
    pub largest_allocation_bytes: f64,
    pub linear_growth_bytes: f64,
    pub linear_growth_events: f64,
}

/// One allocator call that exhausted the current linear-memory capacity.
/// History counters provide a deterministic progress marker without sampling
/// wall-clock time or reconstructing any core behavior in the bridge.
#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct AllocationEventSnapshot {
    pub sequence: f64,
    pub allocation_index: f64,
    pub scope: AllocationScope,
    pub core_span: Option<String>,
    pub allocation_bytes: f64,
    pub linear_growth_bytes: f64,
    pub linear_bytes_after: f64,
    pub heap_live_bytes_after: f64,
    pub history_events: f64,
    pub history_conversations: f64,
}

#[derive(Debug, Clone, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct WasmAllocationSnapshot {
    /// Whether allocator-level diagnostics were compiled in.
    pub enabled: bool,
    /// Committed WebAssembly linear memory (pages × 64 KiB).
    pub linear_bytes: f64,
    /// Sum of requested sizes of allocations currently alive.
    pub heap_live_bytes: f64,
    /// Peak requested live bytes since `beginWasmAllocationProfile()`.
    pub heap_peak_live_bytes: f64,
    /// Cumulative requested allocation/deallocation churn.
    pub allocated_bytes: f64,
    pub freed_bytes: f64,
    pub allocations: f64,
    pub deallocations: f64,
    pub reallocations: f64,
    pub largest_allocation_bytes: f64,
    /// Mutually exclusive allocation-site buckets. Core task allocation is a
    /// second measurement through the core's `AllocMeter`, exposed on the
    /// client for cross-checking this host-side scope.
    pub other: AllocationBucketSnapshot,
    pub core_task: AllocationBucketSnapshot,
    pub history_decode: AllocationBucketSnapshot,
    pub history_serialize: AllocationBucketSnapshot,
    pub history_envelope: AllocationBucketSnapshot,
    pub history_callback: AllocationBucketSnapshot,
    /// Allocations performed by the profiler while materializing snapshots.
    /// Kept separate so observation overhead cannot masquerade as workload.
    pub diagnostics: AllocationBucketSnapshot,
    pub history_sync: HistorySyncAllocationSnapshot,
    /// Dynamic attribution sourced from core tracing callsites.
    pub core_spans: Vec<CoreSpanAllocationSnapshot>,
    /// Chronological allocator calls that triggered `memory.grow` since the
    /// last `beginWasmAllocationProfile()`.
    pub memory_grow_events: Vec<AllocationEventSnapshot>,
    /// All allocations at or above `large_allocation_threshold_bytes`, whether
    /// they reused committed pages or triggered a grow.
    pub large_allocation_events: Vec<AllocationEventSnapshot>,
    pub large_allocation_threshold_bytes: f64,
    pub dropped_core_span_callsites: f64,
    pub dropped_memory_grow_events: f64,
    pub dropped_large_allocation_events: f64,
}

#[cfg(feature = "memory-profiling")]
mod imp {
    use super::{
        AllocationBucketSnapshot, AllocationEventSnapshot, AllocationScope,
        CoreSpanAllocationSnapshot, HistorySyncAllocationSnapshot, WasmAllocationSnapshot,
    };
    use core::cell::{Cell, RefCell};
    use core::future::Future;
    use core::pin::Pin;
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::Arc;
    use std::time::Duration;
    use tracing::Instrument as _;
    use tracing::metadata::LevelFilter;
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::{Interest, Subscriber};
    use tracing::{Event, Metadata};
    use tracing_core::span::Current;
    use whatsapp_rust::wacore::runtime::{AbortHandle, Runtime};
    use whatsapp_rust::wacore::stats::AllocMeter;

    const BUCKET_COUNT: usize = 7;
    const OTHER_BUCKET: usize = AllocationScope::Other as usize;
    const DIAGNOSTICS_BUCKET: usize = AllocationScope::Diagnostics as usize;

    /// Fixed metadata storage keeps allocator attribution allocation-free.
    /// Core tracing currently has far fewer callsites; overflow is reported in
    /// the snapshot instead of silently allocating a map from inside tracing.
    const MAX_PROFILED_SPAN_CALLSITES: usize = 256;
    const SPAN_SLOT_BITS: u32 = 9;
    const SPAN_SLOT_MASK: u64 = (1_u64 << SPAN_SLOT_BITS) - 1;
    const TRACE_STACK_INITIAL_CAPACITY: usize = 32;
    /// A history-sync run normally grows fewer than twenty times. A bounded
    /// ring makes pathological runs observable without unbounded profiler RAM.
    const ALLOCATION_EVENT_CAPACITY: usize = 64;
    const LARGE_ALLOCATION_THRESHOLD_BYTES: u64 = 1024 * 1024;

    static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
    static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
    static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
    static FREED_BYTES: AtomicU64 = AtomicU64::new(0);
    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static LARGEST_ALLOCATION: AtomicU64 = AtomicU64::new(0);

    static BUCKET_ALLOCATED: [AtomicU64; BUCKET_COUNT] =
        [const { AtomicU64::new(0) }; BUCKET_COUNT];
    static BUCKET_ALLOCATIONS: [AtomicU64; BUCKET_COUNT] =
        [const { AtomicU64::new(0) }; BUCKET_COUNT];
    static BUCKET_LARGEST: [AtomicU64; BUCKET_COUNT] = [const { AtomicU64::new(0) }; BUCKET_COUNT];
    static OBSERVED_LINEAR_BYTES: AtomicU64 = AtomicU64::new(0);
    static BUCKET_LINEAR_GROWTH: [AtomicU64; BUCKET_COUNT] =
        [const { AtomicU64::new(0) }; BUCKET_COUNT];
    static BUCKET_LINEAR_GROW_EVENTS: [AtomicU64; BUCKET_COUNT] =
        [const { AtomicU64::new(0) }; BUCKET_COUNT];

    static SPAN_ALLOCATED: [AtomicU64; MAX_PROFILED_SPAN_CALLSITES] =
        [const { AtomicU64::new(0) }; MAX_PROFILED_SPAN_CALLSITES];
    static SPAN_ALLOCATIONS: [AtomicU64; MAX_PROFILED_SPAN_CALLSITES] =
        [const { AtomicU64::new(0) }; MAX_PROFILED_SPAN_CALLSITES];
    static SPAN_LARGEST: [AtomicU64; MAX_PROFILED_SPAN_CALLSITES] =
        [const { AtomicU64::new(0) }; MAX_PROFILED_SPAN_CALLSITES];
    static SPAN_LINEAR_GROWTH: [AtomicU64; MAX_PROFILED_SPAN_CALLSITES] =
        [const { AtomicU64::new(0) }; MAX_PROFILED_SPAN_CALLSITES];
    static SPAN_LINEAR_GROW_EVENTS: [AtomicU64; MAX_PROFILED_SPAN_CALLSITES] =
        [const { AtomicU64::new(0) }; MAX_PROFILED_SPAN_CALLSITES];
    static NEXT_SPAN_INSTANCE: AtomicU64 = AtomicU64::new(1);
    static DROPPED_SPAN_CALLSITES: AtomicU64 = AtomicU64::new(0);

    struct AllocationEventCounters {
        sequence: AtomicU64,
        allocation_index: AtomicU64,
        scope: AtomicU64,
        span_slot: AtomicU64,
        allocation_bytes: AtomicU64,
        linear_growth_bytes: AtomicU64,
        linear_bytes_after: AtomicU64,
        heap_live_bytes_after: AtomicU64,
        history_events: AtomicU64,
        history_conversations: AtomicU64,
    }

    impl AllocationEventCounters {
        const fn new() -> Self {
            Self {
                sequence: AtomicU64::new(0),
                allocation_index: AtomicU64::new(0),
                scope: AtomicU64::new(0),
                span_slot: AtomicU64::new(0),
                allocation_bytes: AtomicU64::new(0),
                linear_growth_bytes: AtomicU64::new(0),
                linear_bytes_after: AtomicU64::new(0),
                heap_live_bytes_after: AtomicU64::new(0),
                history_events: AtomicU64::new(0),
                history_conversations: AtomicU64::new(0),
            }
        }

        fn reset(&self) {
            self.sequence.store(0, Ordering::Relaxed);
        }
    }

    static MEMORY_GROW_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static MEMORY_GROW_EVENTS: [AllocationEventCounters; ALLOCATION_EVENT_CAPACITY] =
        [const { AllocationEventCounters::new() }; ALLOCATION_EVENT_CAPACITY];
    static LARGE_ALLOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static LARGE_ALLOCATION_EVENTS: [AllocationEventCounters; ALLOCATION_EVENT_CAPACITY] =
        [const { AllocationEventCounters::new() }; ALLOCATION_EVENT_CAPACITY];

    static HS_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_COMPRESSED_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_DECOMPRESSED_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_CONVERSATIONS: AtomicU64 = AtomicU64::new(0);
    static HS_BATCHES: AtomicU64 = AtomicU64::new(0);
    static HS_SKIPPED: AtomicU64 = AtomicU64::new(0);
    static HS_LARGEST_COMPRESSED: AtomicU64 = AtomicU64::new(0);
    static HS_LARGEST_DECOMPRESSED: AtomicU64 = AtomicU64::new(0);
    static HS_QUEUED_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_QUEUED_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_PEAK_QUEUED_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_PEAK_QUEUED_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_ACTIVE_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_ACTIVE_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_PEAK_ACTIVE_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_PEAK_ACTIVE_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_RETAINED_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_RETAINED_BYTES: AtomicU64 = AtomicU64::new(0);
    static HS_PEAK_RETAINED_EVENTS: AtomicU64 = AtomicU64::new(0);
    static HS_PEAK_RETAINED_BYTES: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, Copy)]
    struct ActiveSpan {
        raw_id: u64,
        slot: usize,
    }

    #[derive(Default)]
    struct TraceState {
        callsites: Vec<&'static Metadata<'static>>,
        active: Vec<ActiveSpan>,
    }

    thread_local! {
        static ACTIVE_BUCKET: Cell<usize> = const { Cell::new(OTHER_BUCKET) };
        static CORE_TASK_DEPTH: Cell<u32> = const { Cell::new(0) };
        static CORE_TASK_PREVIOUS_BUCKET: Cell<usize> = const { Cell::new(OTHER_BUCKET) };
        static TRACE_STATE: RefCell<TraceState> = RefCell::new(TraceState::default());
    }

    fn prepare_trace_state() {
        TRACE_STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.callsites.reserve_exact(MAX_PROFILED_SPAN_CALLSITES);
            state.active.reserve_exact(TRACE_STACK_INITIAL_CAPACITY);
        });
    }

    fn register_callsite(metadata: &'static Metadata<'static>) -> Option<usize> {
        TRACE_STATE.with(|state| {
            let mut state = state.borrow_mut();
            if let Some(slot) = state
                .callsites
                .iter()
                .position(|known| core::ptr::eq(*known, metadata))
            {
                return Some(slot);
            }
            if state.callsites.len() == MAX_PROFILED_SPAN_CALLSITES {
                DROPPED_SPAN_CALLSITES.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            let slot = state.callsites.len();
            state.callsites.push(metadata);
            Some(slot)
        })
    }

    fn current_span_slot() -> Option<usize> {
        TRACE_STATE
            .try_with(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|state| state.active.last().map(|span| span.slot))
            })
            .ok()
            .flatten()
    }

    fn span_metadata(slot: usize) -> Option<&'static Metadata<'static>> {
        TRACE_STATE
            .try_with(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|state| state.callsites.get(slot).copied())
            })
            .ok()
            .flatten()
    }

    fn span_name(slot: Option<usize>) -> Option<String> {
        slot.and_then(span_metadata)
            .map(|metadata| metadata.name().to_owned())
    }

    /// Allocation-free tracing subscriber: span IDs encode their callsite slot,
    /// and only static `Metadata` pointers plus a preallocated active stack are
    /// retained. This avoids pulling a general-purpose span registry into the
    /// profiler and materially perturbing the WASM heap under observation.
    struct AllocationTraceSubscriber;

    impl Subscriber for AllocationTraceSubscriber {
        fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
            if metadata.is_span() {
                Interest::always()
            } else {
                Interest::never()
            }
        }

        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.is_span()
        }

        fn max_level_hint(&self) -> Option<LevelFilter> {
            Some(LevelFilter::TRACE)
        }

        fn new_span(&self, attributes: &Attributes<'_>) -> Id {
            let slot_code = register_callsite(attributes.metadata())
                .map(|slot| slot as u64 + 1)
                .unwrap_or(0);
            let instance = NEXT_SPAN_INSTANCE.fetch_add(1, Ordering::Relaxed);
            let raw = (instance << SPAN_SLOT_BITS) | slot_code;
            Id::from_u64(raw.max(1))
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, _event: &Event<'_>) {}

        fn enter(&self, id: &Id) {
            let raw_id = id.into_u64();
            let slot_code = raw_id & SPAN_SLOT_MASK;
            if slot_code == 0 {
                return;
            }
            let slot = slot_code as usize - 1;
            let _ = TRACE_STATE.try_with(|state| {
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.active.push(ActiveSpan { raw_id, slot });
                }
            });
        }

        fn exit(&self, id: &Id) {
            let raw_id = id.into_u64();
            let _ = TRACE_STATE.try_with(|state| {
                if let Ok(mut state) = state.try_borrow_mut() {
                    if state
                        .active
                        .last()
                        .is_some_and(|span| span.raw_id == raw_id)
                    {
                        state.active.pop();
                    } else if let Some(position) =
                        state.active.iter().rposition(|span| span.raw_id == raw_id)
                    {
                        state.active.remove(position);
                    }
                }
            });
        }

        fn current_span(&self) -> Current {
            TRACE_STATE
                .try_with(|state| {
                    let Ok(state) = state.try_borrow() else {
                        return Current::none();
                    };
                    let Some(active) = state.active.last() else {
                        return Current::none();
                    };
                    let Some(metadata) = state.callsites.get(active.slot).copied() else {
                        return Current::none();
                    };
                    Current::new(Id::from_u64(active.raw_id), metadata)
                })
                .unwrap_or_else(|_| Current::none())
        }
    }

    pub(crate) fn install_trace_profiler() -> Result<(), tracing::subscriber::SetGlobalDefaultError>
    {
        prepare_trace_state();
        tracing::subscriber::set_global_default(AllocationTraceSubscriber)
    }

    fn update_max(target: &AtomicU64, value: u64) {
        let mut current = target.load(Ordering::Relaxed);
        while value > current {
            match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    fn allocation_scope(bucket: usize) -> AllocationScope {
        match bucket {
            value if value == AllocationScope::CoreTask as usize => AllocationScope::CoreTask,
            value if value == AllocationScope::HistoryDecode as usize => {
                AllocationScope::HistoryDecode
            }
            value if value == AllocationScope::HistorySerialize as usize => {
                AllocationScope::HistorySerialize
            }
            value if value == AllocationScope::HistoryEnvelope as usize => {
                AllocationScope::HistoryEnvelope
            }
            value if value == AllocationScope::HistoryCallback as usize => {
                AllocationScope::HistoryCallback
            }
            value if value == AllocationScope::Diagnostics as usize => AllocationScope::Diagnostics,
            _ => AllocationScope::Other,
        }
    }

    #[derive(Clone, Copy)]
    struct AllocationEventSample {
        bucket: usize,
        span_slot: Option<usize>,
        allocation_size: u64,
        growth: u64,
        linear_bytes_after: u64,
        heap_live_bytes_after: u64,
        allocation_index: u64,
    }

    fn record_allocation_event(
        sequence_counter: &AtomicU64,
        events: &[AllocationEventCounters; ALLOCATION_EVENT_CAPACITY],
        sample: AllocationEventSample,
    ) {
        let sequence = sequence_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let event = &events[((sequence - 1) as usize) % ALLOCATION_EVENT_CAPACITY];
        // Publish `sequence` last. Snapshot readers skip zero/old slots; on a
        // threaded WASM build the release/acquire pair also prevents torn rows.
        event.sequence.store(0, Ordering::Release);
        event
            .allocation_index
            .store(sample.allocation_index, Ordering::Relaxed);
        event.scope.store(sample.bucket as u64, Ordering::Relaxed);
        event.span_slot.store(
            sample.span_slot.map_or(0, |slot| slot as u64 + 1),
            Ordering::Relaxed,
        );
        event
            .allocation_bytes
            .store(sample.allocation_size, Ordering::Relaxed);
        event
            .linear_growth_bytes
            .store(sample.growth, Ordering::Relaxed);
        event
            .linear_bytes_after
            .store(sample.linear_bytes_after, Ordering::Relaxed);
        event
            .heap_live_bytes_after
            .store(sample.heap_live_bytes_after, Ordering::Relaxed);
        event
            .history_events
            .store(HS_EVENTS.load(Ordering::Relaxed), Ordering::Relaxed);
        event
            .history_conversations
            .store(HS_CONVERSATIONS.load(Ordering::Relaxed), Ordering::Relaxed);
        event.sequence.store(sequence, Ordering::Release);
    }

    fn record_linear_growth(
        bucket: usize,
        span_slot: Option<usize>,
        allocation_size: u64,
        heap_live_bytes_after: u64,
        allocation_index: u64,
    ) -> u64 {
        let current = (core::arch::wasm32::memory_size::<0>() * crate::WASM_PAGE_BYTES) as u64;
        let mut previous = OBSERVED_LINEAR_BYTES.load(Ordering::Relaxed);
        while current > previous {
            match OBSERVED_LINEAR_BYTES.compare_exchange_weak(
                previous,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let growth = current - previous;
                    BUCKET_LINEAR_GROWTH[bucket].fetch_add(growth, Ordering::Relaxed);
                    BUCKET_LINEAR_GROW_EVENTS[bucket].fetch_add(1, Ordering::Relaxed);
                    if let Some(slot) = span_slot {
                        SPAN_LINEAR_GROWTH[slot].fetch_add(growth, Ordering::Relaxed);
                        SPAN_LINEAR_GROW_EVENTS[slot].fetch_add(1, Ordering::Relaxed);
                    }
                    record_allocation_event(
                        &MEMORY_GROW_SEQUENCE,
                        &MEMORY_GROW_EVENTS,
                        AllocationEventSample {
                            bucket,
                            span_slot,
                            allocation_size,
                            growth,
                            linear_bytes_after: current,
                            heap_live_bytes_after,
                            allocation_index,
                        },
                    );
                    return growth;
                }
                Err(observed) => previous = observed,
            }
        }
        0
    }

    fn record_alloc(size: usize) {
        let size = size as u64;
        let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
        update_max(&PEAK_LIVE_BYTES, live);
        update_max(&LARGEST_ALLOCATION, size);
        ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
        let allocation_index = ALLOCATIONS.fetch_add(1, Ordering::Relaxed) + 1;
        AllocMeter::on_alloc(size as usize);

        let bucket = ACTIVE_BUCKET.with(Cell::get);
        BUCKET_ALLOCATED[bucket].fetch_add(size, Ordering::Relaxed);
        BUCKET_ALLOCATIONS[bucket].fetch_add(1, Ordering::Relaxed);
        update_max(&BUCKET_LARGEST[bucket], size);
        let span_slot = if bucket == DIAGNOSTICS_BUCKET {
            None
        } else {
            current_span_slot()
        };
        if let Some(slot) = span_slot {
            SPAN_ALLOCATED[slot].fetch_add(size, Ordering::Relaxed);
            SPAN_ALLOCATIONS[slot].fetch_add(1, Ordering::Relaxed);
            update_max(&SPAN_LARGEST[slot], size);
        }
        // `System.alloc` has already returned, so any allocator-triggered
        // `memory.grow` is visible now and can be attributed to this scope.
        let linear_growth = record_linear_growth(bucket, span_slot, size, live, allocation_index);
        if size >= LARGE_ALLOCATION_THRESHOLD_BYTES {
            record_allocation_event(
                &LARGE_ALLOCATION_SEQUENCE,
                &LARGE_ALLOCATION_EVENTS,
                AllocationEventSample {
                    bucket,
                    span_slot,
                    allocation_size: size,
                    growth: linear_growth,
                    linear_bytes_after: (core::arch::wasm32::memory_size::<0>()
                        * crate::WASM_PAGE_BYTES) as u64,
                    heap_live_bytes_after: live,
                    allocation_index,
                },
            );
        }
    }

    fn record_dealloc(size: usize) {
        let size = size as u64;
        LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
        FREED_BYTES.fetch_add(size, Ordering::Relaxed);
        DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        AllocMeter::on_dealloc(size as usize);
    }

    struct ProfilingAllocator;

    unsafe impl GlobalAlloc for ProfilingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                record_alloc(layout.size());
            }
            ptr
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                record_alloc(layout.size());
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            record_dealloc(layout.size());
            unsafe { System.dealloc(ptr, layout) };
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let next = unsafe { System.realloc(ptr, layout, new_size) };
            if !next.is_null() {
                record_dealloc(layout.size());
                record_alloc(new_size);
                REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
            next
        }
    }

    #[global_allocator]
    static GLOBAL_ALLOCATOR: ProfilingAllocator = ProfilingAllocator;

    pub(crate) struct ScopeGuard {
        previous: usize,
    }

    impl Drop for ScopeGuard {
        fn drop(&mut self) {
            ACTIVE_BUCKET.with(|active| active.set(self.previous));
        }
    }

    pub(crate) fn enter_scope(scope: AllocationScope) -> ScopeGuard {
        let previous = ACTIVE_BUCKET.with(|active| active.replace(scope as usize));
        ScopeGuard { previous }
    }

    fn enter_core_task() {
        CORE_TASK_DEPTH.with(|depth| {
            let current = depth.get();
            if current == 0 {
                let previous =
                    ACTIVE_BUCKET.with(|active| active.replace(AllocationScope::CoreTask as usize));
                CORE_TASK_PREVIOUS_BUCKET.with(|slot| slot.set(previous));
            }
            depth.set(current.saturating_add(1));
        });
    }

    fn exit_core_task() {
        CORE_TASK_DEPTH.with(|depth| {
            let current = depth.get();
            if current == 0 {
                return;
            }
            let next = current - 1;
            depth.set(next);
            if next == 0 {
                let previous = CORE_TASK_PREVIOUS_BUCKET.with(Cell::get);
                ACTIVE_BUCKET.with(|active| active.set(previous));
            }
        });
    }

    /// Profiling-only composite: the core's own meter remains the source of
    /// task-level churn, while the host allocator gains a mutually exclusive
    /// scope for live/linear-memory attribution.
    pub(crate) struct CoreTaskInstrument {
        meter: std::sync::Arc<AllocMeter>,
    }

    impl CoreTaskInstrument {
        pub(crate) fn new(meter: std::sync::Arc<AllocMeter>) -> Self {
            Self { meter }
        }
    }

    impl whatsapp_rust::wacore::stats::TaskInstrument for CoreTaskInstrument {
        fn on_poll_start(&self) {
            enter_core_task();
            whatsapp_rust::wacore::stats::TaskInstrument::on_poll_start(&*self.meter);
        }

        fn on_poll_end(&self) {
            whatsapp_rust::wacore::stats::TaskInstrument::on_poll_end(&*self.meter);
            exit_core_task();
        }
    }

    /// Profiling-only runtime decorator that preserves the core's current
    /// tracing span across detached futures and blocking closures. In
    /// particular, the large history extractor runs through `spawn_blocking`;
    /// without this generic context propagation it would appear as an unnamed
    /// core task even though whatsapp-rust already supplied authoritative span
    /// metadata at the callsite.
    pub(crate) struct TraceContextRuntime {
        inner: Arc<dyn Runtime>,
    }

    impl TraceContextRuntime {
        pub(crate) fn new(inner: Arc<dyn Runtime>) -> Self {
            Self { inner }
        }
    }

    // The concrete bridge runtime is single-threaded on wasm32. This mirrors
    // the escape hatch used by whatsapp-rust's own InstrumentedRuntime.
    unsafe impl Send for TraceContextRuntime {}
    unsafe impl Sync for TraceContextRuntime {}

    #[async_trait::async_trait(?Send)]
    impl Runtime for TraceContextRuntime {
        fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) -> AbortHandle {
            let parent = tracing::Span::current();
            self.inner.spawn(Box::pin(future.instrument(parent)))
        }

        fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()>>> {
            self.inner.sleep(duration)
        }

        fn spawn_blocking(
            &self,
            operation: Box<dyn FnOnce() + 'static>,
        ) -> Pin<Box<dyn Future<Output = ()>>> {
            let parent = tracing::Span::current();
            self.inner.spawn_blocking(Box::new(move || {
                let _entered = parent.enter();
                operation();
            }))
        }

        fn yield_now(&self) -> Option<Pin<Box<dyn Future<Output = ()>>>> {
            self.inner.yield_now()
        }

        fn yield_frequency(&self) -> u32 {
            self.inner.yield_frequency()
        }
    }

    pub(crate) fn record_history_sync(compressed: usize, decompressed: usize) {
        let compressed = compressed as u64;
        let decompressed = decompressed as u64;
        HS_EVENTS.fetch_add(1, Ordering::Relaxed);
        HS_COMPRESSED_BYTES.fetch_add(compressed, Ordering::Relaxed);
        HS_DECOMPRESSED_BYTES.fetch_add(decompressed, Ordering::Relaxed);
        update_max(&HS_LARGEST_COMPRESSED, compressed);
        update_max(&HS_LARGEST_DECOMPRESSED, decompressed);
    }

    pub(crate) fn record_history_conversation() {
        HS_CONVERSATIONS.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_history_batch() {
        HS_BATCHES.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_history_skipped(count: usize) {
        HS_SKIPPED.fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(crate) fn record_history_event_enqueued(compressed: usize) {
        let compressed = compressed as u64;
        let queued_events = HS_QUEUED_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
        let queued_bytes = HS_QUEUED_BYTES.fetch_add(compressed, Ordering::Relaxed) + compressed;
        let retained_events = HS_RETAINED_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
        let retained_bytes =
            HS_RETAINED_BYTES.fetch_add(compressed, Ordering::Relaxed) + compressed;
        update_max(&HS_PEAK_QUEUED_EVENTS, queued_events);
        update_max(&HS_PEAK_QUEUED_BYTES, queued_bytes);
        update_max(&HS_PEAK_RETAINED_EVENTS, retained_events);
        update_max(&HS_PEAK_RETAINED_BYTES, retained_bytes);
    }

    pub(crate) fn record_history_event_dequeued(compressed: usize) {
        let compressed = compressed as u64;
        HS_QUEUED_EVENTS.fetch_sub(1, Ordering::Relaxed);
        HS_QUEUED_BYTES.fetch_sub(compressed, Ordering::Relaxed);
        let active_events = HS_ACTIVE_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
        let active_bytes = HS_ACTIVE_BYTES.fetch_add(compressed, Ordering::Relaxed) + compressed;
        update_max(&HS_PEAK_ACTIVE_EVENTS, active_events);
        update_max(&HS_PEAK_ACTIVE_BYTES, active_bytes);
    }

    pub(crate) fn record_history_event_completed(compressed: usize) {
        let compressed = compressed as u64;
        HS_ACTIVE_EVENTS.fetch_sub(1, Ordering::Relaxed);
        HS_ACTIVE_BYTES.fetch_sub(compressed, Ordering::Relaxed);
        HS_RETAINED_EVENTS.fetch_sub(1, Ordering::Relaxed);
        HS_RETAINED_BYTES.fetch_sub(compressed, Ordering::Relaxed);
    }

    pub(crate) fn record_history_event_cancelled(compressed: usize) {
        let compressed = compressed as u64;
        HS_QUEUED_EVENTS.fetch_sub(1, Ordering::Relaxed);
        HS_QUEUED_BYTES.fetch_sub(compressed, Ordering::Relaxed);
        HS_RETAINED_EVENTS.fetch_sub(1, Ordering::Relaxed);
        HS_RETAINED_BYTES.fetch_sub(compressed, Ordering::Relaxed);
    }

    fn bucket(index: usize) -> AllocationBucketSnapshot {
        AllocationBucketSnapshot {
            allocated_bytes: BUCKET_ALLOCATED[index].load(Ordering::Relaxed) as f64,
            allocations: BUCKET_ALLOCATIONS[index].load(Ordering::Relaxed) as f64,
            largest_allocation_bytes: BUCKET_LARGEST[index].load(Ordering::Relaxed) as f64,
            linear_growth_bytes: BUCKET_LINEAR_GROWTH[index].load(Ordering::Relaxed) as f64,
            linear_growth_events: BUCKET_LINEAR_GROW_EVENTS[index].load(Ordering::Relaxed) as f64,
        }
    }

    fn core_span_snapshots() -> Vec<CoreSpanAllocationSnapshot> {
        TRACE_STATE.with(|state| {
            let state = state.borrow();
            state
                .callsites
                .iter()
                .enumerate()
                .filter_map(|(slot, metadata)| {
                    let allocated = SPAN_ALLOCATED[slot].load(Ordering::Relaxed);
                    let allocations = SPAN_ALLOCATIONS[slot].load(Ordering::Relaxed);
                    let growth = SPAN_LINEAR_GROWTH[slot].load(Ordering::Relaxed);
                    if allocated == 0 && allocations == 0 && growth == 0 {
                        return None;
                    }
                    Some(CoreSpanAllocationSnapshot {
                        name: metadata.name().to_owned(),
                        target: metadata.target().to_owned(),
                        source_file: metadata.file().map(str::to_owned),
                        source_line: metadata.line().map(|line| line as f64),
                        allocated_bytes: allocated as f64,
                        allocations: allocations as f64,
                        largest_allocation_bytes: SPAN_LARGEST[slot].load(Ordering::Relaxed) as f64,
                        linear_growth_bytes: growth as f64,
                        linear_growth_events: SPAN_LINEAR_GROW_EVENTS[slot].load(Ordering::Relaxed)
                            as f64,
                    })
                })
                .collect()
        })
    }

    fn allocation_event_snapshots(
        sequence_counter: &AtomicU64,
        source: &[AllocationEventCounters; ALLOCATION_EVENT_CAPACITY],
    ) -> Vec<AllocationEventSnapshot> {
        let total = sequence_counter.load(Ordering::Acquire);
        let first = total
            .saturating_sub(ALLOCATION_EVENT_CAPACITY as u64)
            .saturating_add(1)
            .max(1);
        let mut events = Vec::with_capacity(
            usize::try_from(total.saturating_sub(first).saturating_add(1))
                .unwrap_or(ALLOCATION_EVENT_CAPACITY)
                .min(ALLOCATION_EVENT_CAPACITY),
        );
        for sequence in first..=total {
            let event = &source[((sequence - 1) as usize) % ALLOCATION_EVENT_CAPACITY];
            if event.sequence.load(Ordering::Acquire) != sequence {
                continue;
            }
            let span_code = event.span_slot.load(Ordering::Relaxed);
            let bucket = event.scope.load(Ordering::Relaxed) as usize;
            events.push(AllocationEventSnapshot {
                sequence: sequence as f64,
                allocation_index: event.allocation_index.load(Ordering::Relaxed) as f64,
                scope: allocation_scope(bucket),
                core_span: span_name((span_code > 0).then_some(span_code as usize - 1)),
                allocation_bytes: event.allocation_bytes.load(Ordering::Relaxed) as f64,
                linear_growth_bytes: event.linear_growth_bytes.load(Ordering::Relaxed) as f64,
                linear_bytes_after: event.linear_bytes_after.load(Ordering::Relaxed) as f64,
                heap_live_bytes_after: event.heap_live_bytes_after.load(Ordering::Relaxed) as f64,
                history_events: event.history_events.load(Ordering::Relaxed) as f64,
                history_conversations: event.history_conversations.load(Ordering::Relaxed) as f64,
            });
        }
        events
    }

    pub fn begin_profile() {
        PEAK_LIVE_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
        LARGEST_ALLOCATION.store(0, Ordering::Relaxed);
        OBSERVED_LINEAR_BYTES.store(
            (core::arch::wasm32::memory_size::<0>() * crate::WASM_PAGE_BYTES) as u64,
            Ordering::Relaxed,
        );
        for largest in &BUCKET_LARGEST {
            largest.store(0, Ordering::Relaxed);
        }
        for slot in 0..MAX_PROFILED_SPAN_CALLSITES {
            SPAN_ALLOCATED[slot].store(0, Ordering::Relaxed);
            SPAN_ALLOCATIONS[slot].store(0, Ordering::Relaxed);
            SPAN_LARGEST[slot].store(0, Ordering::Relaxed);
            SPAN_LINEAR_GROWTH[slot].store(0, Ordering::Relaxed);
            SPAN_LINEAR_GROW_EVENTS[slot].store(0, Ordering::Relaxed);
        }
        MEMORY_GROW_SEQUENCE.store(0, Ordering::Relaxed);
        for event in &MEMORY_GROW_EVENTS {
            event.reset();
        }
        LARGE_ALLOCATION_SEQUENCE.store(0, Ordering::Relaxed);
        for event in &LARGE_ALLOCATION_EVENTS {
            event.reset();
        }
        HS_PEAK_QUEUED_EVENTS.store(HS_QUEUED_EVENTS.load(Ordering::Relaxed), Ordering::Relaxed);
        HS_PEAK_QUEUED_BYTES.store(HS_QUEUED_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
        HS_PEAK_ACTIVE_EVENTS.store(HS_ACTIVE_EVENTS.load(Ordering::Relaxed), Ordering::Relaxed);
        HS_PEAK_ACTIVE_BYTES.store(HS_ACTIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
        HS_PEAK_RETAINED_EVENTS.store(
            HS_RETAINED_EVENTS.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        HS_PEAK_RETAINED_BYTES.store(HS_RETAINED_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    pub fn snapshot() -> WasmAllocationSnapshot {
        WasmAllocationSnapshot {
            enabled: true,
            linear_bytes: (core::arch::wasm32::memory_size::<0>() * crate::WASM_PAGE_BYTES) as f64,
            heap_live_bytes: LIVE_BYTES.load(Ordering::Relaxed) as f64,
            heap_peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed) as f64,
            allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed) as f64,
            freed_bytes: FREED_BYTES.load(Ordering::Relaxed) as f64,
            allocations: ALLOCATIONS.load(Ordering::Relaxed) as f64,
            deallocations: DEALLOCATIONS.load(Ordering::Relaxed) as f64,
            reallocations: REALLOCATIONS.load(Ordering::Relaxed) as f64,
            largest_allocation_bytes: LARGEST_ALLOCATION.load(Ordering::Relaxed) as f64,
            other: bucket(0),
            core_task: bucket(1),
            history_decode: bucket(2),
            history_serialize: bucket(3),
            history_envelope: bucket(4),
            history_callback: bucket(5),
            diagnostics: bucket(6),
            history_sync: HistorySyncAllocationSnapshot {
                events: HS_EVENTS.load(Ordering::Relaxed) as f64,
                compressed_bytes: HS_COMPRESSED_BYTES.load(Ordering::Relaxed) as f64,
                decompressed_bytes: HS_DECOMPRESSED_BYTES.load(Ordering::Relaxed) as f64,
                conversations: HS_CONVERSATIONS.load(Ordering::Relaxed) as f64,
                batches: HS_BATCHES.load(Ordering::Relaxed) as f64,
                skipped_conversations: HS_SKIPPED.load(Ordering::Relaxed) as f64,
                largest_compressed_bytes: HS_LARGEST_COMPRESSED.load(Ordering::Relaxed) as f64,
                largest_decompressed_bytes: HS_LARGEST_DECOMPRESSED.load(Ordering::Relaxed) as f64,
                queued_events: HS_QUEUED_EVENTS.load(Ordering::Relaxed) as f64,
                queued_compressed_bytes: HS_QUEUED_BYTES.load(Ordering::Relaxed) as f64,
                peak_queued_events: HS_PEAK_QUEUED_EVENTS.load(Ordering::Relaxed) as f64,
                peak_queued_compressed_bytes: HS_PEAK_QUEUED_BYTES.load(Ordering::Relaxed) as f64,
                active_events: HS_ACTIVE_EVENTS.load(Ordering::Relaxed) as f64,
                active_compressed_bytes: HS_ACTIVE_BYTES.load(Ordering::Relaxed) as f64,
                peak_active_events: HS_PEAK_ACTIVE_EVENTS.load(Ordering::Relaxed) as f64,
                peak_active_compressed_bytes: HS_PEAK_ACTIVE_BYTES.load(Ordering::Relaxed) as f64,
                retained_events: HS_RETAINED_EVENTS.load(Ordering::Relaxed) as f64,
                retained_compressed_bytes: HS_RETAINED_BYTES.load(Ordering::Relaxed) as f64,
                peak_retained_events: HS_PEAK_RETAINED_EVENTS.load(Ordering::Relaxed) as f64,
                peak_retained_compressed_bytes: HS_PEAK_RETAINED_BYTES.load(Ordering::Relaxed)
                    as f64,
            },
            core_spans: core_span_snapshots(),
            memory_grow_events: allocation_event_snapshots(
                &MEMORY_GROW_SEQUENCE,
                &MEMORY_GROW_EVENTS,
            ),
            large_allocation_events: allocation_event_snapshots(
                &LARGE_ALLOCATION_SEQUENCE,
                &LARGE_ALLOCATION_EVENTS,
            ),
            large_allocation_threshold_bytes: LARGE_ALLOCATION_THRESHOLD_BYTES as f64,
            dropped_core_span_callsites: DROPPED_SPAN_CALLSITES.load(Ordering::Relaxed) as f64,
            dropped_memory_grow_events: MEMORY_GROW_SEQUENCE
                .load(Ordering::Relaxed)
                .saturating_sub(ALLOCATION_EVENT_CAPACITY as u64)
                as f64,
            dropped_large_allocation_events: LARGE_ALLOCATION_SEQUENCE
                .load(Ordering::Relaxed)
                .saturating_sub(ALLOCATION_EVENT_CAPACITY as u64)
                as f64,
        }
    }
}

#[cfg(not(feature = "memory-profiling"))]
mod imp {
    use super::{
        AllocationBucketSnapshot, AllocationScope, HistorySyncAllocationSnapshot,
        WasmAllocationSnapshot,
    };

    pub(crate) struct ScopeGuard;

    pub(crate) fn enter_scope(_scope: AllocationScope) -> ScopeGuard {
        ScopeGuard
    }

    pub(crate) fn record_history_sync(_compressed: usize, _decompressed: usize) {}
    pub(crate) fn record_history_conversation() {}
    pub(crate) fn record_history_batch() {}
    pub(crate) fn record_history_skipped(_count: usize) {}
    pub(crate) fn record_history_event_enqueued(_compressed: usize) {}
    pub(crate) fn record_history_event_dequeued(_compressed: usize) {}
    pub(crate) fn record_history_event_completed(_compressed: usize) {}
    pub(crate) fn record_history_event_cancelled(_compressed: usize) {}
    pub fn begin_profile() {}

    fn empty_bucket() -> AllocationBucketSnapshot {
        AllocationBucketSnapshot {
            allocated_bytes: 0.0,
            allocations: 0.0,
            largest_allocation_bytes: 0.0,
            linear_growth_bytes: 0.0,
            linear_growth_events: 0.0,
        }
    }

    pub fn snapshot() -> WasmAllocationSnapshot {
        WasmAllocationSnapshot {
            enabled: false,
            linear_bytes: (core::arch::wasm32::memory_size::<0>() * 65_536) as f64,
            heap_live_bytes: 0.0,
            heap_peak_live_bytes: 0.0,
            allocated_bytes: 0.0,
            freed_bytes: 0.0,
            allocations: 0.0,
            deallocations: 0.0,
            reallocations: 0.0,
            largest_allocation_bytes: 0.0,
            other: empty_bucket(),
            core_task: empty_bucket(),
            history_decode: empty_bucket(),
            history_serialize: empty_bucket(),
            history_envelope: empty_bucket(),
            history_callback: empty_bucket(),
            diagnostics: empty_bucket(),
            history_sync: HistorySyncAllocationSnapshot {
                events: 0.0,
                compressed_bytes: 0.0,
                decompressed_bytes: 0.0,
                conversations: 0.0,
                batches: 0.0,
                skipped_conversations: 0.0,
                largest_compressed_bytes: 0.0,
                largest_decompressed_bytes: 0.0,
                queued_events: 0.0,
                queued_compressed_bytes: 0.0,
                peak_queued_events: 0.0,
                peak_queued_compressed_bytes: 0.0,
                active_events: 0.0,
                active_compressed_bytes: 0.0,
                peak_active_events: 0.0,
                peak_active_compressed_bytes: 0.0,
                retained_events: 0.0,
                retained_compressed_bytes: 0.0,
                peak_retained_events: 0.0,
                peak_retained_compressed_bytes: 0.0,
            },
            core_spans: Vec::new(),
            memory_grow_events: Vec::new(),
            large_allocation_events: Vec::new(),
            large_allocation_threshold_bytes: 0.0,
            dropped_core_span_callsites: 0.0,
            dropped_memory_grow_events: 0.0,
            dropped_large_allocation_events: 0.0,
        }
    }
}

#[cfg(feature = "memory-profiling")]
pub(crate) use imp::{CoreTaskInstrument, TraceContextRuntime, install_trace_profiler};
pub use imp::{begin_profile, snapshot};
pub(crate) use imp::{
    enter_scope, record_history_batch, record_history_conversation, record_history_event_cancelled,
    record_history_event_completed, record_history_event_dequeued, record_history_event_enqueued,
    record_history_skipped, record_history_sync,
};
