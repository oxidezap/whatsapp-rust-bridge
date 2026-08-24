# Whether talc 5.1.0 is worth taking back

This bridge ran talc as its `#[global_allocator]` and dropped it in June 2026,
after two bugs in 5.0.3 broke it in production. Both are fixed, and 5.1.0
changed the default source the fix left in place, so the question was worth
reopening.

**It is not worth taking back, and the reason is not the bugs.** The bugs are
gone: `benches/talc-repro/` runs them red on 5.0.3 and green on 5.1.0. What
stops it is that talc costs **5.0% on an allocation-dominated crossing**
(16 of 16 rounds, against a control arm that is a coin flip) and returns
**nothing** for it: committed linear memory is identical to dlmalloc's on every
workload measured, to the page. The artifact is 6,895 bytes smaller, which is
below what the per-process memory harness can resolve.

The owner's condition for spending bytes was "memory and CPU better". Memory is
equal and CPU is worse, so the trade is not on offer.

Everything below is reproducible from `benches/wasm-allocator/` (runtime and
size) and `benches/wasm-module-rss/` (per-process memory), with the two bugs in
`benches/talc-repro/`.

## What changed upstream

Two bugs, both fixed in **5.0.4**:

- **`WasmGrowAndClaim` undersized its growth.** `delta_pages` was
  `(size + CHUNK_UNIT + PAGE_SIZE - 1) / PAGE_SIZE`, and for `size = n*65536 -
  16` that floors to exactly `n` pages, one chunk header short of what
  `required_chunk_size` plus claim overhead needs. The claimed heap could never
  fit the request, so `Talc::allocate`'s `loop` grew linear memory and tried
  again, forever. `n*65536 - 16` is an AES-GCM plaintext of a page-multiple
  payload, which is why this bridge found it. 5.0.4 replaced the expression
  with `delta_pages_for`, which asks `required_chunk_size` instead of
  reimplementing it.

- **The chunk tag was read out of a gap's size.** `Tag` was a `u8` and
  `end_to_tag(end)` was `end - 1`; a gap's trailing size is a `usize` at
  `end - 4`. On little-endian wasm32 those are the same byte only while the gap
  is under 2^24, so every gap of 16 MiB or more with bit 24 set read back as an
  allocated chunk. 5.0.4 made `Tag` a `usize` and read the whole word, whose low
  byte is zero for a gap because a gap size is a multiple of `CHUNK_UNIT`.

  The consequences are both in `benches/talc-repro/`: an `extend` over a
  misread top gap strands it instead of fusing it, and freeing the chunk above
  one takes `deallocate`'s "mark the chunk below as having a gap above it"
  branch, whose `|= 1 << 1` lands in the size's most significant byte and adds
  **33,554,432 bytes** to the recorded size of a gap that holds 20 MiB.

5.0.4 declared one regression with the fix: per-allocation metadata is a `usize`
rather than a `u8`, said upstream to be free for allocations of 24 bytes or
under. It is not free here and the reason is not the 24 bytes: `CHUNK_UNIT` is
16 on wasm32 and `required_chunk_size` rounds up to it, so the three extra bytes
change the rounded size only for requests in the last three bytes of a
`CHUNK_UNIT` step, about 19% of a uniform distribution. That is a per-allocation
byte cost the tables below fold into the committed-memory columns, and those
columns come out identical to dlmalloc's, so it is not what decides this.

**5.1.0** then switched `new_wasm_dynamic_allocator()` and `WasmDynamicTalc`
from `WasmGrowAndClaim` to `WasmGrowAndExtend`, on the grounds
([issue #51](https://github.com/SFBdragon/talc/issues/51)) that grow-and-claim
can consume up to 10x more memory on a repeatedly growing vector, against 97
bytes of extra module size. Inflating a history-sync blob is a repeatedly
growing vector, so this is the change that made the question worth asking
again. It also fixed `Send` on two source types, which this bridge does not use.

## Method

One machine, one session: linux x64, 4 cores, node v22.22.2, rustc
1.96.0-nightly (2026-04-05), wasm-bindgen 0.2.126, **native** binaryen 132.

Every arm is one artifact from `benches/wasm-module-rss/build-variant.sh`: the
release profile from `Cargo.toml` verbatim, `wasm-bindgen`, then the release
wasm-opt pass list. The arms differ by cargo features and nothing else, and one
`talc-arms.patch` adds those features for the run and comes back out afterwards,
so the shipped manifest never carries an allocator feature nobody turns on.

**The control arm is the point.** `control.wasm` is `dlmalloc.wasm` copied under
a second name. Whatever separates those two rows is the harness's floor, and no
row closer to the base than the control is evidence. An earlier pass of this
work read a 3.6% "regression" on `inflate` that the control reproduced exactly.

Runtime: one process per sample, arms measured round-robin with the order
rotated every round pair and reversed on the odd round of each pair, 16 rounds
for the fast workloads and 14 for the heavy ones. Rotation on its own moves
every arm together, so an arm two launches after the base arm stays two
launches after it in almost every round, and the paired ratio absorbs
within-round drift rather than cancelling it. Mirroring makes each arm's mean
and median signed distance from the base exactly zero, which is why the round
counts are even.

Per-operation time is the **fastest of seven batches** inside a sample rather
than the mean of one, because a batch that lost the CPU reports the scheduler.
The `paired` column compares an arm to the base arm within the same round, which
cancels whatever the machine was doing that round, and `slower` counts the
rounds it lost outright. Read those two, not the medians, when the rows are
close.

Per-process memory: `benches/wasm-module-rss/measure.mjs`, 9 repetitions,
round-robin, `Private_Dirty` from `/proc/self/smaps_rollup`. The reasons for
that metric rather than RSS or USS are in `docs/wasm-artifact-private-memory.md`
and are unchanged here.

Committed linear memory: `getWasmMemoryBytes()`, which is pages the module
holds, not heap the allocator handed out. The distinction is the whole of the
grow-and-claim against grow-and-extend argument, so both `peak` and `final` are
reported; on wasm32 a page never goes back to the host, so `final` is what the
process keeps for good.

### The arms

| arm | what it installs |
|---|---|
| `dlmalloc` | nothing: the wasm32 target default through `std` |
| `control` | `dlmalloc`, byte for byte, under a second name |
| `talc-extend` | `WasmDynamicTalc`, the 5.1.0 default (`WasmGrowAndExtend`) |
| `talc-claim` | `TalcSyncCell::new_wasm(WasmGrowAndClaim)`, the pre-5.1.0 default |
| `talc-arena` | `new_wasm_arena_allocator` over a 128 MiB `static mut` |
| `talc-extend-nogrow` | `talc-extend` plus `disable-grow-in-place` |
| `talc-extend-norealloc` | `talc-extend` plus `disable-realloc-in-place` |

`TalcCell` against `TalcSyncCell` is not an arm, because it is not a choice. In
5.1.0 `TalcSyncCell` is a newtype over `TalcCell` with an `unsafe impl Sync` and
nothing else: there is no lock to pay for and no allocation path that differs.
A `#[global_allocator]` static has to be `Sync`, so `TalcCell` cannot be one.
`TalcSyncCell::new_wasm` panics unless the target is single-threaded wasm, and
the install `cfg` is upstream's own
`all(not(target_feature = "atomics"), target_family = "wasm")`, which this
crate's `.cargo/config.toml` satisfies: the wasm rustflags are `+simd128` and a
getrandom backend, with no `atomics`. A future build with atomics keeps the
target default rather than silently installing something unsound.

`counters` was off in every arm above. It adds accounting to each allocator
call, which is the thing being measured.

## Results

### Size

One build per arm, so the numbers are exact rather than medians.

| arm | file | vs dlmalloc | code | functions | largest body |
|---|---:|---:|---:|---:|---:|
| `dlmalloc` | 5,597,177 | | 5,076,641 | 11,972 | 87,576 |
| `talc-extend` | 5,590,282 | **−6,895** | 5,069,919 | 11,970 | 87,576 |
| `talc-claim` | 5,590,154 | −7,023 | 5,069,785 | 11,970 | 87,576 |
| `talc-arena` | 5,592,747 | −4,430 | 5,072,362 | 11,970 | 87,577 |
| `talc-extend-nogrow` | 5,590,154 | −7,023 | 5,069,779 | 11,970 | 87,576 |
| `talc-extend-norealloc` | 5,590,076 | −7,101 | 5,069,689 | 11,970 | 87,576 |

talc is **smaller** than dlmalloc, not larger, which turns the framing of this
question around: there is no size budget to argue about. The gap between
`talc-claim` and `talc-extend` is 128 bytes, against the 97 upstream quotes for
that change, which is close enough to say the same thing on a different
codegen profile. The largest body does not move, so `check:wasm-shape` has
nothing to say about any of these.

### Per-process memory

`Private_Dirty` after compiling the module and dropping the wire bytes,
9 repetitions, round-robin.

| arm | code MiB | file MiB | retained ΔPriv_Dirty MiB |
|---|---:|---:|---:|
| `dlmalloc` | 4.84 | 5.34 | 6.58 ±0.03 |
| `control` | 4.84 | 5.34 | 6.57 ±0.03 |
| `talc-extend` | 4.84 | 5.33 | 6.54 ±0.03 |
| `talc-claim` | 4.83 | 5.33 | 6.57 ±0.03 |
| `talc-arena` | 4.84 | 5.33 | 6.56 ±0.03 |
| `talc-extend-nogrow` | 4.83 | 5.33 | 6.56 ±0.04 |
| `talc-extend-norealloc` | 4.83 | 5.33 | 6.56 ±0.04 |

**This table cannot resolve the size win, and says so.** The model in
`docs/wasm-artifact-private-memory.md` prices a wire byte at 1.052 bytes of
`Private_Dirty`, so 6,895 bytes buys 7,254 bytes, or 0.007 MiB. The uncertainty
on every row is ±0.03 MiB, four times larger, and `dlmalloc` and `control` are
the same artifact 0.01 MiB apart. Take the size saving from the size table,
where it is exact, and read this table only as "nothing here got worse".

### Committed linear memory

Peak and final, medians of 14 rounds. Every arm reported the same value in every
round on every workload, hence ±0.00: committed pages are a step function of the
allocation pattern, and the pattern is deterministic.

| arm | `inflate` peak | `historySync` peak | `retention` peak | after peak |
|---|---:|---:|---:|---:|
| `dlmalloc` | 6.81 ±0.00 | 34.50 ±0.00 | 32.81 ±0.00 | 0.00 ±0.00 |
| `control` | 6.81 ±0.00 | 34.50 ±0.00 | 32.81 ±0.00 | 0.00 ±0.00 |
| `talc-extend` | 6.81 ±0.00 | 34.50 ±0.00 | 32.81 ±0.00 | 0.00 ±0.00 |
| `talc-claim` | 6.81 ±0.00 | 34.50 ±0.00 | 32.81 ±0.00 | 0.00 ±0.00 |
| `talc-arena` | 129.50 ±0.00 | 129.50 ±0.00 | 129.50 ±0.00 | 0.00 ±0.00 |
| `talc-extend-nogrow` | **11.25** ±0.00 | 34.50 ±0.00 | **59.38** ±0.00 | 0.00 ±0.00 |
| `talc-extend-norealloc` | **11.25** ±0.00 | 34.50 ±0.00 | **59.38** ±0.00 | 0.00 ±0.00 |

Three readings:

- **talc and dlmalloc commit exactly the same memory, to the page**, on all
  three shapes. This is the finding that decides the question, and it is the one
  the reopening was betting against.
- **`talc-arena` commits 128 MiB before the first message.** It is the arm that
  cannot be argued with a percentage: 129.50 MiB against 6.81 on a 4 MiB
  inflate. A fleet pays that per process, so it is disqualified on the metric
  that matters most here rather than on intuition.
- **The two size features are expensive in memory.** Turning off in-place
  growth costs 4.44 MiB on `inflate` and **26.57 MiB** on `retention`, because
  a buffer that grows by doubling has to be copied to a new chunk each time
  instead of extending into the gap above it. They buy 128 and 206 bytes of
  artifact off `talc-extend`.

`after peak` is `committedAfterPeak`, which `retention()` reports as
`committedFinal - afterPeak`, and it is 0.00
everywhere: no arm asked the host for a page after the peak had been reached and
released, so nothing here separates the allocators on reuse
either. That is the fragmentation question, and on this workload it has no
answer because no allocator failed it.

### CPU, per crossing

16 rounds. `paired` is the median of the per-round ratio against `dlmalloc`;
`slower` is the rounds the arm lost.

**`callOnly` — `getWasmMemoryBytes()`, no allocation at all:**

| arm | ns/op | paired | slower |
|---|---:|---:|---:|
| `dlmalloc` | 6.9 ±0.2 | | |
| `control` | 6.9 ±0.7 | +1.1% | 9/16 |
| `talc-extend` | 6.9 ±1.1 | +0.4% | 9/16 |
| `talc-claim` | 6.8 ±0.7 | −0.3% | 7/16 |
| `talc-arena` | 7.0 ±0.5 | +0.9% | 10/16 |
| `talc-extend-nogrow` | 6.9 ±0.3 | +1.2% | 9/16 |
| `talc-extend-norealloc` | 6.8 ±0.7 | −1.4% | 7/16 |

Every arm inside a control that is itself +1.1% and 9/16. A crossing that does
not allocate does not care which allocator is installed, which is the check that
says the next table is measuring the allocator and not the boundary.

**`churn` — `md5` of 16 bytes: the same crossing, plus `__wbindgen_malloc`,
the copy in and `__wbindgen_free`. The digest coming back is not part of it:
`byte_array` builds it with `Uint8Array::new_with_length`, which is the JS
engine's heap, so every row here prices the inbound half of a crossing:**

| arm | ns/op | paired | slower |
|---|---:|---:|---:|
| `dlmalloc` | 319.1 ±7.6 | | |
| `control` | 317.6 ±48.6 | +0.5% | 9/16 |
| `talc-extend` | 334.4 ±31.9 | **+5.0%** | **16/16** |
| `talc-claim` | 332.2 ±36.9 | +4.5% | 16/16 |
| `talc-arena` | 329.1 ±5.0 | +2.8% | 15/16 |
| `talc-extend-nogrow` | 331.9 ±11.8 | +5.1% | 16/16 |
| `talc-extend-norealloc` | 333.9 ±8.7 | +4.9% | 15/16 |

**This is the result.** The control lands on a coin flip, 9 of 16 and +0.5%, so
the harness has no bias of its own; against it, every talc arm is 2.8–5.1%
slower and every one of them loses at least 15 rounds of 16. The old comment
this bridge carried said talc was "~2x faster than dlmalloc"; on this profile,
on this workload, it is not faster at all.

**`boundary` — `md5` of 1 KiB: the same again, with real work behind it:**

| arm | ns/op | paired | slower |
|---|---:|---:|---:|
| `dlmalloc` | 1953.9 ±30.5 | | |
| `control` | 1956.9 ±23.4 | +0.1% | 9/16 |
| `talc-extend` | 1991.1 ±34.8 | +1.9% | 14/16 |
| `talc-claim` | 1977.0 ±40.5 | +2.2% | 14/16 |
| `talc-arena` | 1975.9 ±48.1 | +2.3% | 11/16 |
| `talc-extend-nogrow` | 1978.7 ±38.5 | +1.4% | 11/16 |
| `talc-extend-norealloc` | 1972.1 ±32.0 | +1.3% | 14/16 |

The same regression, diluted: 5.0% at 16 bytes becomes 1.9% at 1 KiB, because
the fixed allocator cost is now a smaller share of a call that also hashes a
kilobyte. The direction survives (14 of 16), the magnitude does not.

### CPU, per message and on allocation-heavy load

14 rounds.

| workload | arm | ns/op | paired | slower |
|---|---|---:|---:|---:|
| `ratchet` | `dlmalloc` | 175,215.7 ±6,033.2 | | |
| | `control` | 176,536.5 ±4,818.9 | +0.0% | 7/14 |
| | `talc-extend` | 177,754.7 ±2,869.6 | +0.7% | 9/14 |
| | `talc-claim` | 176,990.6 ±4,901.8 | +0.3% | 10/14 |
| | `talc-arena` | 176,812.5 ±4,042.9 | +0.4% | 9/14 |
| `inflate` | `dlmalloc` | 3,114,340.7 ±160,583.2 | | |
| | `control` | 3,074,596.4 ±220,719.3 | +1.2% | 8/14 |
| | `talc-extend` | 3,099,552.1 ±154,969.2 | −1.0% | 6/14 |
| | `talc-claim` | 3,034,134.2 ±109,326.8 | −2.8% | 3/14 |
| | `talc-arena` | 3,081,092.8 ±83,792.3 | −1.1% | 6/14 |
| `historySync` | `dlmalloc` | 8,069,911.8 ±157,026.7 | | |
| | `control` | 8,054,518.2 ±207,841.9 | −0.3% | 7/14 |
| | `talc-extend` | 8,013,780.4 ±162,178.3 | +0.2% | 7/14 |
| | `talc-claim` | 8,408,798.6 ±226,779.6 | **+4.0%** | **14/14** |
| | `talc-arena` | 8,055,983.1 ±101,936.1 | −0.7% | 5/14 |
| `retention` | `dlmalloc` | 4,211,455.1 ±61,795.8 | | |
| | `control` | 4,238,021.6 ±89,186.9 | +0.6% | 9/14 |
| | `talc-extend` | 4,253,690.6 ±109,551.4 | +1.4% | 11/14 |
| | `talc-claim` | 4,222,169.5 ±71,689.4 | +0.3% | 9/14 |
| | `talc-arena` | 4,245,130.8 ±86,497.6 | +0.9% | 9/14 |

`ratchet` is 175 µs of curve arithmetic per message and the allocator is
invisible in it: `talc-extend` at +0.7% and 9 of 14 is `control` at +0.0% and
7 of 14. `inflate` and `retention` say the same in a wider band.

The one row that clears its control is `talc-claim` on `historySync`: +4.0% and
**14 of 14**, against a control of −0.3% and 7 of 14. That is grow-and-claim paying
for the fragmentation upstream's issue #51 describes, on the workload where a
buffer grows by doubling. It does not show up in the committed-memory table
because the 64 MiB inflate ceiling caps the buffer before the 10x can
accumulate, and it does not change the recommendation, because grow-and-claim is
not the arm on offer. It is the clearest confirmation in this run that the
5.1.0 default change was the right one for this shape of load.

The `disable-*` arms are omitted from this table because they are already
disqualified by the committed-memory table above.

## What dlmalloc actually does, and what both of them miss

The measurement above says talc and dlmalloc commit the same memory to the page,
which is a suspiciously exact tie. Reading `dlmalloc-rs` says why, and turns up
two things neither allocator does on wasm that the platform would allow.

`benches/talc-repro/src/upstream.rs` is the runnable half of this section, so a
newer dlmalloc or talc can be rechecked rather than reread.

### dlmalloc already grows and extends

`library/std/src/sys/alloc/wasm.rs` is a thin shim: a `SyncUnsafeCell` around
`dlmalloc::Dlmalloc`, a no-op lock without `atomics`, and four forwarding
functions. The interesting code is `dlmalloc-rs`, and its `sys_alloc` has this,
after the system allocator hands back new memory:

```rust
let mut sp: *mut Segment = &mut self.seg;
while !sp.is_null() && tbase != Segment::top(sp) { sp = (*sp).next; }
if !sp.is_null() && ... && Segment::holds(sp, self.top.cast()) {
    (*sp).size += tsize;
    self.init_top(self.top, self.topsize + tsize);
}
```

If the new region is contiguous with the segment that holds the current top, the
top chunk is **extended in place** instead of a new segment being added. On wasm
`memory.grow` is always contiguous, so this is always the branch taken.

That is `WasmGrowAndExtend`. dlmalloc has had it the whole time. So talc 5.1.0's
headline change, moving the default off `WasmGrowAndClaim` because grow-and-claim
can cost 10x on a growing vector, brought talc **to** dlmalloc's behaviour rather
than past it, and the page-for-page tie in the table above is two implementations
of one strategy. It also explains the other row: `talc-claim` is the arm that is
slower on `historySync`, because grow-and-claim is the strategy dlmalloc never
had.

### The runaway bug class cannot exist there

`sys_alloc` asks for `align_up(size + top_foot_size + malloc_alignment,
DEFAULT_GRANULARITY)` with `DEFAULT_GRANULARITY = 64 * 1024`, and
`dl/src/wasm.rs` then does `size.div_ceil(self.page_size())`. The chunk overhead
is added **before** the page rounding, and both steps round up. talc's bug was a
floor: `(size + CHUNK_UNIT + PAGE_SIZE - 1) / PAGE_SIZE` computes a page count
that excludes the tag the chunk also needs. There is no `n*65536 - 16` for
dlmalloc to get wrong.

### Neither one grows the top for a realloc

This is the gap, and it is in both.

**dlmalloc.** `try_realloc_chunk` has a branch for exactly the right case and
then declines it:

```rust
} else if next == self.top {
    // extend into top
    if oldsize + self.topsize <= nb {
        return ptr::null_mut();   // caller falls back to malloc + memcpy + free
    }
```

`sys_alloc` is never called from any realloc path. When the chunk being grown is
the topmost one and the top gap is too small, dlmalloc copies, even though the
top is the end of linear memory and `memory.grow` would extend it with no copy
at all, through the same contiguity `sys_alloc` already relies on.

**talc.** `try_grow_in_place` only ever extends into an existing adjacent gap
(`old_tag.is_above_free()`), and `S::acquire` is called from exactly one place in
the crate: the `loop` inside `Talc::allocate`. So on wasm the source whose entire
job is extending the heap is never consulted during a grow, and `realloc` falls
through to `allocate` plus `copy_from_nonoverlapping` plus `deallocate`.

Measured on a buffer doubling from 64 KiB, which is the shape of an inflate
output and of every `Vec` that grows:

| to 16 MiB, run alone | doublings that moved | copied |
|---|---:|---:|
| dlmalloc (`std` `System`) | 7 of 8 | 16,256 KiB |
| talc `WasmGrowAndExtend` | 8 of 8 | 16,320 KiB |
| talc `WasmGrowAndClaim` | 8 of 8 | 16,320 KiB |

**A doubling buffer is copied about once in full to reach its size, and on wasm
it does not have to be.** dlmalloc is marginally the better of the two here, not
worse: it caught one in-place grow by absorbing an adjacent free chunk where talc
caught none. That is the opposite of what reading `try_realloc_chunk` first
suggested, which is why it is measured rather than argued.

What a fix would need, and what it would cost:

- **dlmalloc**: a capability on the `Allocator` trait, say `fn
  grows_contiguously(&self) -> bool { false }`, true only in `wasm.rs`; and in
  the `next == self.top` branch, call `sys_alloc` before giving up when that is
  set. The default keeps every non-wasm target byte-identical.
- **talc**: let `try_grow_in_place` call `S::acquire` when the allocation's end
  is the heap end, which is a state `Talc` already tracks through
  `TRACK_HEAP_END` and `heap_end_to_gap_base`.

Neither is free. Both make a `realloc` able to grow the heap where before it
would reuse a lower free chunk, so a heap with a large hole below the top could
commit a page it currently avoids. On this bridge's shape it looks like a double
win rather than a trade, because the fallback already commits a second region the
size of the whole buffer: dlmalloc committed 517 pages, about 33 MiB, to end up
holding 16 MiB. But "looks like" is not measured, and it is the general dlmalloc
test suite that would have to say so, not this workload.

### `allocates_zeros` is inert

`dl/src/wasm.rs` reports `allocates_zeros() = true`, and the guarantee holds:
pages read straight off `memory.grow` are zero, asserted in
`memory_grow_hands_back_zeroed_pages`. But the only consumer is

```rust
pub unsafe fn calloc_must_clear(&self, ptr: *mut u8) -> bool {
    !self.system_allocator.allocates_zeros() || !Chunk::mmapped(Chunk::from_mem(ptr))
}
```

and `Chunk::mmapped` is `head & INUSE == 0`, a state **nothing in dlmalloc-rs
ever produces**: the crate has no `mmap_alloc`, only consumers of the bit. So the
right-hand side is always true, `calloc_must_clear` is always true, and
`alloc_zeroed` always memsets, including over pages that are already zero. Every
`vec![0u8; n]` served from fresh linear memory pays a redundant `n`-byte write.

The same dead bit makes `Allocator::remap` unreachable on every target, so
`dl/src/wasm.rs`'s `// TODO: I think this can be implemented near the end?` on
`remap` cannot buy anything: `mmap_resize` is its only caller and it sits behind
`if Chunk::mmapped(p)`. Recording that here is the cheap part of this section,
the same way this repository's own `--converge` note exists so the next person
finds the number before spending the week.

Exploiting the zeroing is not free either: it needs per-chunk provenance, since a
chunk that was written and freed is not zero. That is a bit of state per chunk,
which is exactly the metadata cost talc's 5.0.4 fix was criticised for adding.

Only the platform half is testable from here, and the test asserts just that.
Whether a given `alloc()` happens to land on still-virgin pages is not:
`GlobalAlloc::alloc` returns uninitialized storage, and reading it to find out
would be undefined however the bytes look. The redundancy above is established by
reading `calloc_must_clear`, not by probing the heap.

### None of this changes the recommendation

Both gaps are shared. Fixing either one upstream would help dlmalloc and talc
about equally, and neither is a reason to switch this bridge's allocator today.
They are written down because the measurement pointed at them and because the
next person to reopen this question should start here rather than at the
changelog.

## The exchange rate

The repository has priced this trade once already, at a rate this can be read
against. `--one-caller-inline-max-function-size 2000` costs ~1–2% on each
inbound message decode and buys **18.41 MiB** of a consumer's private memory
after connect, for 158 bytes of code
(`docs/wasm-compile-zone-peak.md`). That is the shape of a deal this repository
takes: a per-message percentage for MiB per process.

Bytes and committed pages are the same currency here. A wire byte costs 1.052
bytes of `Private_Dirty` in every process
(`docs/wasm-artifact-private-memory.md`); a committed linear-memory page is
written by the guest, so it is private and dirty too, at 1.0. So the ruler is:

> An allocator change earns its place if, per process, it returns more bytes of
> committed memory plus artifact than it costs, and it may spend up to ~2% on
> the per-message path to do it — the rate the inlining cap already established.

Where talc lands on that ruler:

| | talc-extend against dlmalloc |
|---|---|
| artifact | **−6,895 B**, worth 7,254 B of `Private_Dirty` per process |
| committed linear memory | **0 B** on all three workloads, to the page |
| total returned per process | ~7 KiB |
| per-message CPU | **+5.0%** at a 16-byte crossing (16/16), +1.9% at 1 KiB (14/16), nothing measurable once a message does real work |

7 KiB against the 18.41 MiB the last trade bought is a factor of 2,600. It is
0.1% of what the module already costs a process, and the harness that measures
per-process memory cannot see it at all. The CPU side is under the 2% ceiling on
any workload with real work in it, but the ceiling was for buying MiB, and there
are no MiB here.

The ruler does not have to be strict to answer this. Even reading the CPU cost
as zero everywhere except the pure-allocation crossing, the return is 7 KiB, and
7 KiB is not a reason to take on a `#[global_allocator]`, a dependency, and a
repeat of a class of bug that has already cost this bridge one production
incident.

## Recommendation

**Stay on dlmalloc.** `Cargo.toml` is unchanged and talc is not a dependency.

What that costs, explicitly, since the other path is real: 6,895 bytes of
artifact, worth about 7 KiB of `Private_Dirty` in every process that imports
this package, which is 0.1% of the 6.58 MiB the module already costs. That is
the whole bill, and it is a bill this repository pays elsewhere without
noticing: two builds of identical sources here differ in 49,486 bytes of the
same-sized code section.

If the answer is ever to change, one of these has to move:

- **A workload where the allocator is a real share of the time.** `churn` says
  talc is 5.0% slower when allocation is most of the call; the same table says
  allocation is not most of any call this bridge actually makes. A profile that
  crosses the boundary far more often per message, with far smaller payloads,
  would reweight that.
- **Committed memory diverging.** They are identical to the page today. A
  long-lived client with a fragmented Rust heap is the case that could separate
  them, and it is exactly the case this harness cannot reach.
- **talc getting faster on this codegen profile.** The regression is on
  `opt-level = "z"` with this wasm-opt pass list, which is not what upstream
  benchmarks against.

## What was not measured

- **A real session.** There is no mock server in this environment, so nothing
  here goes through `connect()`, a store, or a live history sync. Every export
  used is a free function over bytes that copies its result out and frees, so
  the bridge's long-lived Rust heap never exists. **That is where fragmentation
  would show, and it is the single largest hole in this run.** The
  committed-memory columns are per-crossing and per-blob, not per-session.
- **`memory-profiling` builds.** `src/memory_profile.rs` installs a counting
  `#[global_allocator]`, and a crate declares exactly one, so talc and the
  counters cannot both be it. The arrangement that works is the counters
  wrapping talc rather than `System`, which `talc-arms.patch` carries and which
  compiles; it was never measured, because a build with counters on is not the
  build being decided about.
- **Combinations of the size features.** `disable-grow-in-place` and
  `disable-realloc-in-place` were measured one at a time. Upstream says the
  first has no effect once the second is on, and both are already disqualified
  by 26.57 MiB on `retention`, so the pair was not built.
- **`WasmGrowAndClaim` with the arena, or any non-`WasmBinning` binning.**
  Neither is a configuration this bridge would ship.
- **A second machine, or a second node.** Every number here is one session on
  one 4-core linux x64 box under node v22.22.2. The control arm bounds the
  within-session noise; it says nothing about between-machine transfer.
- **The 5.0.4 metadata regression on its own.** It is folded into every talc
  row rather than isolated, because there is no 5.0.3-with-the-fixes to compare
  against.
