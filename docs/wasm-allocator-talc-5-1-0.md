# Whether talc 5.1.0 is worth taking back

This bridge ran talc as its `#[global_allocator]` and dropped it in June 2026,
after two bugs in 5.0.3 broke it in production. Both are fixed, and 5.1.0
changed the default source the fix left in place, so the question was worth
reopening.

**It is not worth taking back, and the reason is not the bugs.** The bugs are
gone: `benches/talc-repro/` runs them red on 5.0.3 and green on 5.1.0. What
stops it is that talc costs **4.6% on an allocation-dominated crossing**
(14 of 15 rounds, against a control arm that is a coin flip) and returns
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
rotated every round, 15 rounds for the fast workloads and 13 for the heavy ones.
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

Peak and final, medians of 13 rounds. Every arm reported the same value in every
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
  instead of extending into the gap above it. They buy 90 and 168 bytes.

`after peak` is 0.00 everywhere: no arm asked the host for a page after the peak
had been reached and released, so nothing here separates the allocators on reuse
either. That is the fragmentation question, and on this workload it has no
answer because no allocator failed it.

### CPU, per crossing

15 rounds. `paired` is the median of the per-round ratio against `dlmalloc`;
`slower` is the rounds the arm lost.

**`callOnly` — `getWasmMemoryBytes()`, no allocation at all:**

| arm | ns/op | paired | slower |
|---|---:|---:|---:|
| `dlmalloc` | 6.7 ±0.5 | | |
| `control` | 6.9 ±0.5 | +3.6% | 9/15 |
| `talc-extend` | 6.9 ±0.2 | +2.2% | 9/15 |
| `talc-claim` | 6.8 ±0.2 | +0.9% | 8/15 |
| `talc-arena` | 6.8 ±0.2 | +1.2% | 10/15 |
| `talc-extend-nogrow` | 6.8 ±0.2 | −0.0% | 7/15 |
| `talc-extend-norealloc` | 6.8 ±0.4 | +0.8% | 8/15 |

Every arm inside a control that is itself +3.6% and 9/15. A crossing that does
not allocate does not care which allocator is installed, which is the check that
says the next table is measuring the allocator and not the boundary.

**`churn` — `md5` of 16 bytes: the same crossing, plus `__wbindgen_malloc`,
the result allocation and `__wbindgen_free`:**

| arm | ns/op | paired | slower |
|---|---:|---:|---:|
| `dlmalloc` | 316.0 ±9.4 | | |
| `control` | 314.7 ±7.4 | −0.9% | 7/15 |
| `talc-extend` | 328.9 ±6.2 | **+4.6%** | **14/15** |
| `talc-claim` | 326.9 ±6.6 | +3.5% | 14/15 |
| `talc-arena` | 329.6 ±5.0 | +4.4% | 14/15 |
| `talc-extend-nogrow` | 328.5 ±5.7 | +4.2% | 14/15 |
| `talc-extend-norealloc` | 329.7 ±6.6 | +4.3% | 14/15 |

**This is the result.** The control lands on a coin flip, 7 of 15 and −0.9%, so
the harness has no bias of its own; against it, every talc arm is 3.5–4.6%
slower and loses 14 rounds of 15. The old comment this bridge carried said talc
was "~2x faster than dlmalloc"; on this profile, on this workload, it is not
faster at all.

**`boundary` — `md5` of 1 KiB: the same again, with real work behind it:**

| arm | ns/op | paired | slower |
|---|---:|---:|---:|
| `dlmalloc` | 1954.6 ±19.3 | | |
| `control` | 1952.8 ±17.8 | −0.5% | 6/15 |
| `talc-extend` | 1981.9 ±26.0 | +1.4% | 13/15 |
| `talc-claim` | 1972.5 ±43.5 | +0.9% | 10/15 |
| `talc-arena` | 1958.8 ±28.8 | +0.6% | 10/15 |
| `talc-extend-nogrow` | 1971.4 ±45.7 | +0.9% | 11/15 |
| `talc-extend-norealloc` | 1976.3 ±31.5 | +1.1% | 14/15 |

The same regression, diluted: 4.6% at 16 bytes becomes 1.4% at 1 KiB, because
the fixed allocator cost is now a smaller share of a call that also hashes a
kilobyte. The direction survives (13 of 15), the magnitude does not.

### CPU, per message and on allocation-heavy load

13 rounds.

| workload | arm | ns/op | paired | slower |
|---|---|---:|---:|---:|
| `ratchet` | `dlmalloc` | 174,993.5 ±2,541.2 | | |
| | `control` | 177,438.1 ±2,641.2 | +1.2% | 10/13 |
| | `talc-extend` | 177,200.4 ±3,019.5 | +1.3% | 9/13 |
| | `talc-claim` | 175,187.8 ±1,921.2 | −0.3% | 6/13 |
| | `talc-arena` | 178,125.9 ±2,991.1 | +1.5% | 10/13 |
| `inflate` | `dlmalloc` | 3,147,259 ±163,411 | | |
| | `control` | 3,064,824 ±116,682 | −0.0% | 6/13 |
| | `talc-extend` | 3,144,523 ±134,148 | −0.1% | 6/13 |
| | `talc-claim` | 3,069,809 ±97,040 | −1.5% | 5/13 |
| | `talc-arena` | 3,065,342 ±126,319 | +0.2% | 7/13 |
| `historySync` | `dlmalloc` | 8,224,506 ±182,068 | | |
| | `control` | 8,038,112 ±176,707 | −1.7% | 3/13 |
| | `talc-extend` | 8,136,721 ±163,298 | −1.1% | 2/13 |
| | `talc-claim` | 8,431,947 ±140,734 | **+1.9%** | **12/13** |
| | `talc-arena` | 8,136,907 ±161,852 | −1.9% | 4/13 |
| `retention` | `dlmalloc` | 4,199,643 ±55,817 | | |
| | `control` | 4,238,977 ±66,001 | +0.8% | 10/13 |
| | `talc-extend` | 4,271,490 ±50,486 | +1.8% | 9/13 |
| | `talc-claim` | 4,194,967 ±64,449 | +0.3% | 8/13 |
| | `talc-arena` | 4,276,222 ±72,748 | +1.3% | 10/13 |

`ratchet` is 175 µs of curve arithmetic per message and the allocator is
invisible in it: `talc-extend` at +1.3% and 9 of 13 is `control` at +1.2% and
10 of 13. `inflate` and `retention` say the same in a wider band.

The one row that clears its control is `talc-claim` on `historySync`: +1.9% and
12 of 13, against a control of −1.7% and 3 of 13. That is grow-and-claim paying
for the fragmentation upstream's issue #51 describes, on the workload where a
buffer grows by doubling. It does not show up in the committed-memory table
because the 64 MiB inflate ceiling caps the buffer before the 10x can
accumulate, and it does not change the recommendation, because grow-and-claim is
not the arm on offer. It is the clearest confirmation in this run that the
5.1.0 default change was the right one for this shape of load.

The `disable-*` arms are omitted from this table because they are already
disqualified by the committed-memory table above.

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
| per-message CPU | **+4.6%** at a 16-byte crossing (14/15), +1.4% at 1 KiB (13/15), nothing measurable once a message does real work |

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
  talc is 4.6% slower when allocation is most of the call; the same table says
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
