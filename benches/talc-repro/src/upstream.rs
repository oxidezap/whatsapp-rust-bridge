//! Two things both allocators leave on the table on wasm.
//!
//! These are not repros and nothing here is broken. They are the measurements
//! behind the upstream section of `docs/wasm-allocator-talc-5-1-0.md`, kept
//! runnable so the claims can be rechecked against a newer dlmalloc or talc
//! rather than reread off a diff.
//!
//! `System` here is the wasm32 target default, which is `dlmalloc-rs` through
//! `library/std/src/sys/alloc/wasm.rs`.

#[cfg(test)]
mod tests {
    use core::alloc::{GlobalAlloc, Layout};
    use std::alloc::System;
    use talc::cell::TalcCell;
    use talc::wasm::{WasmBinning, WasmGrowAndExtend};
    use wasm_bindgen_test::{console_log, wasm_bindgen_test as test};

    /// Reads `len` bytes at `ptr` volatilely and reports whether all are zero.
    ///
    /// Volatile byte reads rather than a `&[u8]`: the caller points at raw
    /// linear memory that no Rust allocation covers, and building a slice over
    /// it would be a reference to storage the compiler is entitled to assume
    /// nothing about.
    fn all_zero_volatile(ptr: *const u8, len: usize) -> bool {
        (0..len).all(|i| unsafe { ptr.add(i).read_volatile() } == 0)
    }

    /// Bytes copied while growing a buffer by doubling, and how many of the
    /// doublings moved it.
    fn doubling_chain<A: GlobalAlloc>(alloc: &A, label: &str, end: usize) -> (usize, usize) {
        let mut layout = Layout::from_size_align(64 * 1024, 1).unwrap();
        let mut ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null());

        let (mut moves, mut copied, mut steps) = (0usize, 0usize, 0usize);
        while layout.size() < end {
            let new_size = layout.size() * 2;
            let next = unsafe { alloc.realloc(ptr, layout, new_size) };
            assert!(!next.is_null(), "{label}: realloc to {new_size} failed");
            steps += 1;
            if next != ptr {
                moves += 1;
                copied += layout.size();
            }
            ptr = next;
            layout = Layout::from_size_align(new_size, layout.align()).unwrap();
        }
        unsafe { alloc.dealloc(ptr, layout) };

        console_log!(
            "{label}: {moves} of {steps} doublings moved, {} KiB copied",
            copied / 1024
        );
        (moves, copied)
    }

    /// A buffer that doubles is the topmost live chunk for most of its life,
    /// and on wasm the top of the heap is the end of linear memory, which
    /// `memory.grow` extends in place. Neither allocator asks for that during a
    /// grow: dlmalloc's `try_realloc_chunk` gives up in its `next == self.top`
    /// branch instead of calling `sys_alloc`, and talc's `try_grow_in_place`
    /// only looks at an existing adjacent gap and never calls `S::acquire`.
    /// So the buffer is copied about once in full to reach its size.
    ///
    /// Only the talc arm is asserted, because it gets a fresh instance and so a
    /// deterministic heap. dlmalloc is the process-wide allocator and shares
    /// its heap with whatever ran before, which changes how much adjacent free
    /// space the buffer happens to find; it is reported instead.
    /// `WasmGrowAndClaim` is left out because it commits enough on this shape
    /// to hit the crate's memory cap when it runs last;
    /// `extend_commits_less_than_claim_on_a_growing_buffer` covers it.
    ///
    /// A future talc that grows in place turns this red rather than passing
    /// quietly, and then the upstream section of the doc needs updating.
    #[test]
    fn a_doubling_buffer_is_copied_about_once_in_full() {
        // 4 MiB rather than the 16 MiB a history-sync blob reaches: the effect
        // is scale free, and three chains of it fit the crate's memory cap even
        // when this runs after every other test in the same instance.
        let target = 4 * 1024 * 1024;

        doubling_chain(&System, "dlmalloc (std System)", target);
        let extend = TalcCell::<_, WasmBinning>::new(WasmGrowAndExtend::new());
        let (_, extend_copied) = doubling_chain(&extend, "talc WasmGrowAndExtend", target);
        assert!(
            extend_copied >= target / 2,
            "talc WasmGrowAndExtend copied only {extend_copied} bytes growing to \
             {target}, which means it grew in place: the upstream doc is stale"
        );
    }

    /// `dl/src/wasm.rs` reports `allocates_zeros() = true`, and the guarantee is
    /// real: this asserts it against `memory.grow` directly. But the only
    /// consumer is `calloc_must_clear`, which is
    /// `!allocates_zeros() || !Chunk::mmapped(p)`, and dlmalloc-rs has no code
    /// that ever produces an mmapped chunk, so the right-hand side is always
    /// true and `alloc_zeroed` always memsets, including over pages
    /// `memory.grow` just handed back zeroed.
    ///
    /// Only the platform guarantee is testable from here. Whether a given
    /// `alloc()` lands on still-virgin pages is not: `GlobalAlloc::alloc`
    /// returns uninitialized storage, and reading it to find out would be
    /// undefined however the bytes happen to look. The redundancy is
    /// established by reading `calloc_must_clear`, not by probing the heap.
    #[test]
    fn memory_grow_hands_back_zeroed_pages() {
        let pages = 8;

        let prev = core::arch::wasm32::memory_grow::<0>(pages);
        assert_ne!(prev, usize::MAX, "memory.grow failed");
        let base = (prev * 65536) as *const u8;
        assert!(
            all_zero_volatile(base, pages * 65536),
            "memory.grow handed back non-zero pages"
        );
    }
}
