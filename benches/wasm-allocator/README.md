# What a global allocator costs this bridge

Prices one `#[global_allocator]` against another on this bridge's own load,
rather than on an allocator benchmark. `docs/wasm-allocator-talc-5-1-0.md` is
the run that produced it and the reading it supports.

Artifacts come from `benches/wasm-module-rss/build-variant.sh`, which builds the
release profile and the `Cargo.toml` wasm-opt pass list without going through
`wasm-pack`. `wasm-bindgen` and `wasm-opt` have to be on PATH, and the
`wasm-opt` has to be the **native** binaryen: the `binaryen` npm package ships a
wasm build that takes 20 minutes on this pass list where the native one takes 50
seconds.

## The arms

`Cargo.toml` carries no allocator feature: talc is not a dependency, and the
target default (dlmalloc, through `std`) is what ships. `talc-arms.patch` adds
the arms back for a measurement run and is reverted afterwards, which is what
keeps a feature nobody turns on out of the published manifest.

```sh
git apply benches/wasm-allocator/talc-arms.patch

export WASM_OPT=/path/to/native/wasm-opt
V=benches/wasm-module-rss/build-variant.sh
$V dlmalloc
$V talc-extend              --features alloc-talc
$V talc-claim               --features alloc-talc,alloc-talc-claim
$V talc-arena               --features alloc-talc,alloc-talc-arena
$V talc-extend-nogrow       --features alloc-talc,talc/disable-grow-in-place
$V talc-extend-norealloc    --features alloc-talc,talc/disable-realloc-in-place

# reverse the patch rather than checking the four files out: a worktree with
# its own edits to any of them keeps them
git apply -R benches/wasm-allocator/talc-arms.patch
# the builds above wrote a talc entry into the root Cargo.lock, which the patch
# does not carry; this is what prunes it back out
cargo metadata --format-version 1 >/dev/null
```

`git status` is clean after that, apart from the artifacts.

Copy one artifact under a second name before measuring:

```sh
# a subshell, so the commands under "Running" still resolve from the root
(cd benches/wasm-module-rss/artifacts &&
  for ext in wasm glue.js d.ts; do cp dlmalloc.$ext control.$ext; done)
```

`control` is the same bytes as `dlmalloc` under a different name. Whatever
separates those two rows is the harness's own floor, and nothing smaller than it
is a finding. Without it a 1% row reads like a result.

## Running

```sh
node benches/wasm-allocator/sizes.mjs dlmalloc talc-extend …
node benches/wasm-allocator/run.mjs --rounds=16 dlmalloc control talc-extend …
node --expose-gc benches/wasm-module-rss/measure.mjs --reps=9 \
  benches/wasm-module-rss/artifacts/*.wasm
# and again with the linear memory actually created, which is the only mode
# that can price an arm whose cost is its heap rather than its code
node --expose-gc benches/wasm-module-rss/measure.mjs --reps=9 --mode=instantiate \
  benches/wasm-module-rss/artifacts/{dlmalloc,talc-extend,talc-arena}.wasm
```

`run.mjs` puts each sample in its own process, measures the arms round-robin,
and rotates the order every round pair, reversing it on the odd round of each
pair. Rotation alone is not enough: it moves every arm together, so an arm two
launches after the base arm stays two launches after it in almost every round,
and the paired ratio then absorbs within-round drift rather than cancelling it.
Mirroring puts each arm as far before the base as it sat after it, which is why
the round count is rounded up to even.

Per-operation time is the fastest of seven batches rather than the mean of one:
a batch that lost the CPU reports the scheduler, and a mean cannot tell those
apart. The `paired` column compares each arm to the base arm **within** a round,
and `slower` counts the rounds it lost outright, which is the column to read
when the medians are close.

## The workloads

Two of them exist to separate the allocator from the work around it:

| workload | what it is | why |
|---|---|---|
| `callOnly` | `getWasmMemoryBytes()` | a crossing with no allocation at all: the floor under the next row |
| `churn` | `md5` of 16 bytes | the same crossing with `__wbindgen_malloc`/`free` on the way in and almost nothing else |
| `boundary` | `md5` of 1 KiB | the same again with real work behind it; the digest out is the JS engine's heap, not the allocator under test |
| `ratchet` | `calculateAgreement` + `calculateSignature` | the two curve operations every message pays |
| `inflate` | `inflateZlib` of a 4 MiB blob | the history-sync inflate, allocation-heavy |
| `historySync` | 256 KiB to 16 MiB blobs inflated in turn, with crossings between | the load that broke this bridge under talc 5.0.3 |
| `retention` | a 24 MiB peak, then a long tail of small work | whether the tail fits in what the peak committed |

Committed linear memory comes from `getWasmMemoryBytes()`, which is
`memory.buffer.byteLength`: the memory's **logical** size, not heap the
allocator handed out and not resident pages. On wasm32 it never shrinks, so
`peak` and `final` are both what the module holds at the end. For an arm that
grows on demand that tracks residency closely, because a page is added only when
something is about to write to it. For a big fixed arena it does not: the host
maps the whole span and never faults in what is not written. Price that case
with `--mode=instantiate` above, not with this column.

## What it cannot see

No client, no store, no session state: every export here is a free function
over bytes, and each one copies its result out and frees. The bridge's
long-lived Rust heap is reached through `connect()`, which needs a mock server
CI does not have, and that is exactly where fragmentation would show. Read these
numbers as the per-crossing and per-blob cost, not as a whole-session profile.
