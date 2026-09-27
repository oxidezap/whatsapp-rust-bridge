//! The two talc bugs that took this bridge off talc in June.
//!
//! Both are `WasmGrowAndClaim`/`WasmGrowAndExtend` bugs, so both need a real
//! `memory.grow`: they only run on wasm32.

pub(crate) fn memory_pages() -> usize {
    core::arch::wasm32::memory_size::<0>()
}

/// Writes `pattern` over `len` bytes at `ptr`.
pub(crate) fn fill(ptr: *mut u8, len: usize, pattern: u8) {
    // SAFETY: every caller here passes a live allocation and the size it was
    // allocated with, so the whole range is owned and writable.
    unsafe { core::ptr::write_bytes(ptr, pattern, len) }
}

/// Panics naming the first byte at `ptr` that is not `pattern`.
#[track_caller]
pub(crate) fn verify(ptr: *const u8, len: usize, pattern: u8) {
    // SAFETY: every caller passes a live allocation and the size it was
    // allocated with, so the whole range is owned and readable.
    let buf = unsafe { core::slice::from_raw_parts(ptr, len) };
    if let Some(pos) = buf.iter().position(|&b| b != pattern) {
        panic!(
            "heap corruption: {len}-byte allocation with pattern {pattern:#04x} \
             holds {:#04x} at offset {pos}",
            buf[pos]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::{GlobalAlloc, Layout};
    use talc::base::Talc;
    use talc::cell::TalcCell;
    use talc::wasm::{WasmBinning, WasmGrowAndClaim, WasmGrowAndExtend};
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// An AES-GCM plaintext is `payload - 16`, so a payload that is a whole
    /// number of wasm pages asks the allocator for exactly `n * 65536 - 16`.
    /// Before 5.0.4 `delta_pages` rounded that down to `n` pages, one chunk
    /// header short of what the request needs, so the claimed heap could never
    /// fit it and `allocate` looped growing linear memory until the growth
    /// itself failed. In production that walk reached 4 GiB.
    #[test]
    fn aes_gcm_plaintext_size_does_not_run_away() {
        let mut talc = Talc::<_, WasmBinning>::new(WasmGrowAndClaim);

        // the first claim hosts allocator metadata, so prime it out of the way
        let prime = Layout::new::<u128>();
        unsafe { talc.allocate(prime) }.unwrap();

        for pages in 1..=4usize {
            let size = pages * 65536 - 16;
            let layout = Layout::from_size_align(size, 1).unwrap();

            let before = memory_pages();
            let alloc = unsafe { talc.allocate(layout) };
            let grown = memory_pages() - before;

            assert!(alloc.is_some(), "allocation of {size} bytes failed");
            assert!(
                grown <= pages + 1,
                "grew {grown} pages for a {size}-byte allocation"
            );

            unsafe { talc.deallocate(alloc.unwrap().as_ptr(), layout) };
        }
    }

    /// A gap records its size as a `usize` in its last word; an allocation
    /// records its `Tag` in the low byte of the same word. Before 5.0.4 the tag
    /// was a `u8` read at `end - 1`, which on little-endian wasm32 is the gap
    /// size's *most significant* byte: any gap of 16 MiB or more with bit 24
    /// set read back as allocated.
    ///
    /// Two things follow, and this pins the one that is visible from outside
    /// the allocator. Extending a heap whose top gap is misread strands that
    /// gap instead of fusing it, so the request that forced the extend lands
    /// above the freed region rather than in it. The other is the corruption
    /// the next test covers.
    #[test]
    fn extending_over_a_16_mib_gap_reuses_it() {
        let cell = TalcCell::<_, WasmBinning>::new(WasmGrowAndExtend::new());

        // keeps the gap below off the heap base, the way a live client would
        let pin = Layout::from_size_align(64, 1).unwrap();
        let pinned = unsafe { cell.alloc(pin) };
        assert!(!pinned.is_null());

        // 20 MiB, so bit 24 of the gap it leaves behind is set
        let inflate = Layout::from_size_align(0x140_0000, 1).unwrap();
        let freed = unsafe { cell.alloc(inflate) };
        assert!(!freed.is_null(), "20 MiB allocation failed");
        unsafe { cell.dealloc(freed, inflate) };

        // too big for the gap, so this grows memory and extends over it
        let bigger = Layout::from_size_align(0x280_0000, 1).unwrap();
        let extended = unsafe { cell.alloc(bigger) };
        assert!(!extended.is_null(), "40 MiB allocation failed");
        assert!(
            extended as usize <= freed as usize,
            "extend stranded the 20 MiB gap: it starts at {freed:p}, \
             the 40 MiB allocation at {extended:p}"
        );

        fill(extended, bigger.size(), 0x5a);
        verify(extended, bigger.size(), 0x5a);
        unsafe { cell.dealloc(extended, bigger) };
        unsafe { cell.dealloc(pinned, pin) };
    }

    /// The same misread, seen as the write it causes. Freeing the chunk above a
    /// misclassified gap takes `deallocate`'s "mark the chunk below as having a
    /// gap above it" branch, and that OR lands in the gap size's most
    /// significant byte: the recorded size gains `1 << 25`, 32 MiB, over a gap
    /// that is 20 MiB of real memory.
    #[test]
    fn freeing_above_a_16_mib_gap_does_not_grow_its_recorded_size() {
        let cell = TalcCell::<_, WasmBinning>::new(WasmGrowAndExtend::new());

        let pin = Layout::from_size_align(64, 1).unwrap();
        let pinned = unsafe { cell.alloc(pin) };
        assert!(!pinned.is_null());

        let inflate = Layout::from_size_align(0x140_0000, 1).unwrap();
        let freed = unsafe { cell.alloc(inflate) };
        assert!(!freed.is_null(), "20 MiB allocation failed");
        unsafe { cell.dealloc(freed, inflate) };

        // the top gap's trailing size word, read before anything can touch it
        let heap_end = memory_pages() * 65536;
        let tail = (heap_end - size_of::<usize>()) as *const usize;
        let before = unsafe { tail.read() };
        assert!(
            before >= 0x100_0000,
            "the gap is {before} bytes, too small to carry bit 24"
        );

        let bigger = Layout::from_size_align(0x280_0000, 1).unwrap();
        let extended = unsafe { cell.alloc(bigger) };
        assert!(!extended.is_null(), "40 MiB allocation failed");
        unsafe { cell.dealloc(extended, bigger) };

        let after = unsafe { tail.read() };
        assert_eq!(
            after,
            before,
            "the gap's recorded size moved by {} bytes",
            after.wrapping_sub(before) as isize
        );

        unsafe { cell.dealloc(pinned, pin) };
    }

    /// The same size class through the source 5.1.0 installs by default.
    /// `WasmGrowAndExtend` sized its growth with the same expression that broke
    /// `WasmGrowAndClaim`, and 5.0.4 patched both, but the shortfall is absorbed
    /// by the top gap it extends over: this passes on 5.0.3 too. It is here to
    /// keep the size class pinned under the source that would actually ship,
    /// not as a second half of the repro above.
    #[test]
    fn aes_gcm_plaintext_size_does_not_run_away_when_extending() {
        let cell = TalcCell::<_, WasmBinning>::new(WasmGrowAndExtend::new());

        let prime = Layout::new::<u128>();
        let primed = unsafe { cell.alloc(prime) };
        assert!(!primed.is_null());

        for pages in 1..=4usize {
            let size = pages * 65536 - 16;
            let layout = Layout::from_size_align(size, 1).unwrap();

            let before = memory_pages();
            let ptr = unsafe { cell.alloc(layout) };
            let grown = memory_pages() - before;

            assert!(!ptr.is_null(), "allocation of {size} bytes failed");
            assert!(
                grown <= pages + 1,
                "grew {grown} pages for a {size}-byte allocation"
            );

            unsafe { cell.dealloc(ptr, layout) };
        }

        unsafe { cell.dealloc(primed, prime) };
    }
}
