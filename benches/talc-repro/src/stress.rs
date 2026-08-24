//! Allocation stress shaped like this bridge's worst load.
//!
//! It is not the bridge's code: the two bugs above are allocator bugs, and
//! reaching them through `connect()` would need a mock server CI does not have.
//! What it does reproduce is the shape that found them, which is a history sync
//! (a compressed blob, an inflate buffer that grows by doubling, then thousands
//! of small decoded fields released together) running against random churn.

use core::alloc::{GlobalAlloc, Layout};

pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// One live block, with the byte it was filled with.
pub struct Block {
    pub ptr: *mut u8,
    pub layout: Layout,
    pub pattern: u8,
}

pub fn alloc_filled<A: GlobalAlloc>(alloc: &A, size: usize, align: usize, pattern: u8) -> Block {
    let layout = Layout::from_size_align(size, align).unwrap();
    let ptr = unsafe { alloc.alloc(layout) };
    assert!(!ptr.is_null(), "allocation of {size} bytes failed");
    crate::repro::fill(ptr, size, pattern);
    Block {
        ptr,
        layout,
        pattern,
    }
}

pub fn free_verified<A: GlobalAlloc>(alloc: &A, block: Block) {
    crate::repro::verify(block.ptr, block.layout.size(), block.pattern);
    unsafe { alloc.dealloc(block.ptr, block.layout) };
}

pub fn regrow<A: GlobalAlloc>(alloc: &A, block: Block, new_size: usize) -> Block {
    crate::repro::verify(block.ptr, block.layout.size(), block.pattern);
    let next = unsafe { alloc.realloc(block.ptr, block.layout, new_size) };
    assert!(
        !next.is_null(),
        "realloc from {} to {new_size} failed",
        block.layout.size()
    );
    crate::repro::verify(next, block.layout.size().min(new_size), block.pattern);
    crate::repro::fill(next, new_size, block.pattern);
    Block {
        ptr: next,
        layout: Layout::from_size_align(new_size, block.layout.align()).unwrap(),
        pattern: block.pattern,
    }
}

/// Blocks one round decodes at once. The caller owns the `Vec` because it lives
/// on the *global* allocator, not on `alloc`, and growing it inside a
/// measurement window would charge dlmalloc's bookkeeping to the allocator
/// being measured.
pub const BATCH_BLOCKS: usize = 4096;

/// A history-sync round: inflate a blob into a buffer that doubles, decode it
/// into many small fields, hand the batch over, release it all.
///
/// `batch` must already have `BATCH_BLOCKS` of capacity.
pub fn history_sync_round<A: GlobalAlloc>(
    alloc: &A,
    rng: &mut Lcg,
    inflated_bytes: usize,
    batch: &mut Vec<Block>,
) {
    debug_assert!(batch.is_empty() && batch.capacity() >= BATCH_BLOCKS);

    let compressed = alloc_filled(alloc, 256 * 1024 + rng.below(768 * 1024) as usize, 1, 0xc0);

    let mut buffer = alloc_filled(alloc, 64 * 1024, 1, 0x1f);
    while buffer.layout.size() < inflated_bytes {
        let doubled = buffer.layout.size() * 2;
        buffer = regrow(alloc, buffer, doubled);
    }

    for _ in 0..BATCH_BLOCKS {
        let size = 16 + rng.below(496) as usize;
        batch.push(alloc_filled(
            alloc,
            size,
            1 << rng.below(4),
            (rng.next_u64() & 0xff) as u8,
        ));
    }
    for block in batch.drain(..) {
        free_verified(alloc, block);
    }

    free_verified(alloc, buffer);
    free_verified(alloc, compressed);
}

/// Pages of linear memory the allocator commits for `rounds` history-sync
/// rounds. Each source only ever hands out memory it grew itself, so this is
/// that allocator's own cost even when other allocators ran before it.
pub fn history_sync_peak_pages<A: GlobalAlloc>(
    alloc: &A,
    seed: u64,
    rounds: usize,
    inflated_bytes: usize,
) -> usize {
    // Reserved before the window opens: this is the global allocator's memory,
    // and pages it commits are not `alloc`'s to answer for.
    let mut batch = Vec::with_capacity(BATCH_BLOCKS);
    let batch_capacity = batch.capacity();

    let before = crate::repro::memory_pages();
    let mut rng = Lcg(seed);
    for _ in 0..rounds {
        history_sync_round(alloc, &mut rng, inflated_bytes, &mut batch);
    }
    let pages = crate::repro::memory_pages() - before;

    assert_eq!(
        batch.capacity(),
        batch_capacity,
        "the batch vector reallocated"
    );

    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use talc::cell::TalcCell;
    use talc::wasm::{WasmBinning, WasmGrowAndClaim, WasmGrowAndExtend};

    use wasm_bindgen_test::{console_log, wasm_bindgen_test as test};

    /// Headroom for the 16 MiB inflate round under the crate's 256 MiB cap.
    const LIVE_BUDGET_BYTES: usize = 48 * 1024 * 1024;

    /// Enough that `live` never reallocates, which the assertion at the end
    /// checks rather than assumes. 6 rounds of 3,000 actions allocate on 55% of
    /// them, so the run cannot push more than 9,900 blocks even if it frees
    /// none.
    const LIVE_CAPACITY: usize = 16 * 1024;

    fn churn_and_sync<A: GlobalAlloc>(alloc: &A, seed: u64) -> usize {
        // Both of these live on the *global* allocator, not on `alloc`, so they
        // are sized before the window opens: a `Vec` that grows inside it
        // charges dlmalloc's pages to whichever allocator is being measured.
        let mut live: Vec<Block> = Vec::with_capacity(LIVE_CAPACITY);
        let mut batch: Vec<Block> = Vec::with_capacity(BATCH_BLOCKS);
        // What was actually handed over, not what was asked for: with_capacity
        // guarantees at least the request, so only growth from here is a fault.
        let (live_capacity, batch_capacity) = (live.capacity(), batch.capacity());

        let before = crate::repro::memory_pages();
        let mut rng = Lcg(seed);
        let mut live_bytes = 0usize;

        for round in 0..6u64 {
            for _ in 0..3000 {
                while live_bytes > LIVE_BUDGET_BYTES {
                    let at = rng.below(live.len() as u64) as usize;
                    let block = live.swap_remove(at);
                    live_bytes -= block.layout.size();
                    free_verified(alloc, block);
                }

                match rng.below(100) {
                    0..=54 => {
                        let size = match rng.below(100) {
                            0..=79 => 1 + rng.below(4096) as usize,
                            80..=95 => 4096 + rng.below(61440) as usize,
                            _ => 65536 + rng.below(1024 * 1024) as usize,
                        };
                        let align = 1 << rng.below(4);
                        live_bytes += size;
                        live.push(alloc_filled(
                            alloc,
                            size,
                            align,
                            (rng.next_u64() & 0xff) as u8,
                        ));
                    }
                    55..=89 if !live.is_empty() => {
                        let at = rng.below(live.len() as u64) as usize;
                        let block = live.swap_remove(at);
                        live_bytes -= block.layout.size();
                        free_verified(alloc, block);
                    }
                    _ if !live.is_empty() => {
                        let at = rng.below(live.len() as u64) as usize;
                        let block = live.swap_remove(at);
                        let new_size = 1 + rng.below(256 * 1024) as usize;
                        live_bytes = live_bytes - block.layout.size() + new_size;
                        live.push(regrow(alloc, block, new_size));
                    }
                    _ => {}
                }
            }

            // 4, 8 and 16 MiB inflated: the last two leave top gaps in the size
            // class the pre-5.0.4 tag misread.
            history_sync_round(alloc, &mut rng, 4 << (20 + round % 3), &mut batch);
        }

        let pages = crate::repro::memory_pages() - before;

        assert_eq!(
            live.capacity(),
            live_capacity,
            "the live vector reallocated inside the measurement window, so its \
             pages are charged to the allocator under test"
        );
        assert_eq!(
            batch.capacity(),
            batch_capacity,
            "the batch vector reallocated"
        );

        for block in live.drain(..) {
            free_verified(alloc, block);
        }

        pages
    }

    /// A guard rather than a repro: this passes on 5.0.3 as well (1,615 pages
    /// against 5.1.0's 1,603), so it proves nothing about the two fixes. It is
    /// here so the next allocator change has something shaped like the load
    /// that broke this bridge to fail against.
    #[test]
    fn grow_and_extend_survives_history_sync_churn() {
        let cell = TalcCell::<_, WasmBinning>::new(WasmGrowAndExtend::new());
        let pages = churn_and_sync(&cell, 0x5eed_0001);
        console_log!("churn committed {pages} pages");
    }

    /// Why 5.1.0 moved the default off `WasmGrowAndClaim`: a buffer that grows
    /// by doubling is the case upstream measured at up to 10x, and inflating a
    /// history-sync blob is exactly that buffer.
    #[test]
    fn extend_commits_less_than_claim_on_a_growing_buffer() {
        let extend = TalcCell::<_, WasmBinning>::new(WasmGrowAndExtend::new());
        let extend_pages = history_sync_peak_pages(&extend, 0x5eed_0003, 3, 2 * 1024 * 1024);

        let claim = TalcCell::<_, WasmBinning>::new(WasmGrowAndClaim);
        let claim_pages = history_sync_peak_pages(&claim, 0x5eed_0003, 3, 2 * 1024 * 1024);

        assert!(
            extend_pages < claim_pages,
            "extend committed {extend_pages} pages, claim {claim_pages}"
        );
    }
}
