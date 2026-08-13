# What compiling this wasm costs, and what the three biggest functions have to do with it

A consumer (`oxidezap/baileyrs`) reaches ~92 MiB of private memory right after
its first connection, and the connect step alone accounts for ~37 MiB of it.
That step is not per-session state — a sweep from 1 to 8 sessions in one process
put the marginal cost at ~1.3 MiB per additional session — it is V8 compiling
this module for the first time, and it is charged to whichever call happens to
touch the code first.

The report that prompted this said three functions held about half of V8's
compile-zone peak, named two of them as ours, and proposed splitting them in
Rust. Two of the three names hold up. The proposal does not: **none of those
functions exists in the Rust source.** wasm-opt builds them, and it would
rebuild them out of whatever we split.

Everything below is reproducible with `scripts/wasm-fn-sizes.mjs` and
`scripts/wasm-zone-peak.mjs`.

## The artifact

Measurements are against the published `0.11.0` `dist/whatsapp_rust_bridge_bg.wasm`
(5,512,857 bytes, 11,688 defined functions, median body 70 bytes). A local
build of the same commit — whatsapp-rust `8cea605`, wasm-bindgen `0.2.126`,
binaryen `117`, the `Cargo.toml` release flags verbatim — reproduces its code
section exactly: same 11,688 functions, same 4,979,477 body bytes, and the same
three outliers at the same indices. The comparisons here are therefore against
the bytes a consumer actually loads.

| index | body | share of code |
|---|---|---|
| `#2102` | 177,110 B | 3.56 % |
| `#183` | 132,324 B | 2.66 % |
| `#3305` | 69,931 B | 1.40 % |

## Who they are

The shipped module carries no name section (`strip = true`, plus wasm-opt's
`--strip-debug`). Building the same commit with `strip = "none"`, running
`wasm-bindgen --keep-debug`, and running the release wasm-opt flags with `-g`
and without `--strip-debug` produces a module with identical body sizes and a
name for each:

| index | body | name |
|---|---|---|
| `#2102` | 177,110 B | `<waproto::whatsapp::Message as buffa::message::Message>::merge_to_limit::<&[u8]>` |
| `#183` | 132,324 B | `<whatsapp_rust_bridge::wasm_client::JsEventHandler>::new::{closure#0}` |
| `#3305` | 69,931 B | `<whatsapp_rust::handlers::notification::NotificationHandler as StanzaHandler>::handle::{closure#0}` |

So `#183` is ours — the `spawn_local` future in `wasm_client.rs` that runs
`run_event_consumer`, with `dispatch_event_to_js` and `event_to_js` folded into
it. `#3305` is the core's notification handler, as reported. `#2102` is **not**
ours: it is the core's generated protobuf decoder for the top-level `Message`.
Nothing in this repository can shrink it.

## Why splitting them in Rust cannot work

Before wasm-opt the module has 27,651 functions, a median body of 74 bytes, and
a largest body of 44,844. None of the three exists. They are assembled by
wasm-opt, whose `--one-caller-inline-max-function-size` defaults to `-1`:
*every* function with exactly one reference is inlined into its caller, at any
size. Rust emits long chains of single-use functions — serde monomorphisations,
generated per-field proto decoders, async poll fns — and each chain collapses
into one body.

Counting each function's transitive single-reference closure in the named
pre-wasm-opt module predicts the result almost exactly:

| surviving root | own | + single-reference callees | closure | post-wasm-opt |
|---|---|---|---|---|
| `waproto::…::Message::merge_to_limit` | 240 B | 346 fns | 186,419 B | 177,110 B |
| `JsEventHandler::new::{closure#0}` | 6,501 B | 522 fns | 144,320 B | 132,334 B |
| `NotificationHandler::handle::{closure#0}` | 142 B | 70 fns | 71,896 B | 69,941 B |

Our `event_to_js` is one of those 522: 9,224 bytes of its own, dragging 488
single-use serde monomorphisations behind it, all of it folded into the
consumer future above it. Splitting it into N smaller functions gives each part
exactly one caller, which is precisely the condition for inlining it straight
back. The split would cost real CPU — a suspension point per boundary inside an
async state machine — and buy nothing, because the wasm the host compiles would
be the same shape. Capping the inliner, by contrast, does split it: under
`-ocimfs=200` (below) `event_to_js` survives as its own 59,507-byte function.

## Measuring the zone peak

`v8.getHeapStatistics().peak_malloced_memory` tracks it. Two configurations,
both of which `scripts/wasm-zone-peak.mjs` runs:

- `--serial` (`--wasm-num-compilation-tasks=1`): the peak is set by the single
  most expensive function. **Bit-identical across 15 runs**, every artifact
  measured, so a pair of runs is enough to compare two builds.
- `--parallel`: V8's own scheduling, so the peak depends on which functions
  happen to compile together. Every run differs, and the median of 15 moves
  between sessions on this machine (17.8–20.2 MiB for the same artifact), so
  read it for the size of a gap and not for its third decimal.

Both compile the whole module eagerly, which a real connect does not — a
connect compiles roughly a fifth of it. This is a comparator between artifacts,
not a prediction of a host's RSS.

What the number tracks is per-function compilation work, and not one tier's
zones. Compiling every function costs ~7.9 MiB where loading the same module
lazily costs 0.5, and stubbing every body takes it to 0.28 — but `--liftoff-only`
leaves it where it was, to three decimals, in both configurations. Whatever the
optimising tier spends is not what this counts. Read a drop as "the module got
cheaper to compile", not as a claim about which compiler paid, and do not read
the V8 zone names from the original report into these figures.

## What the three functions are actually worth

Replacing a function body with `unreachable` leaves the module valid and the
index space intact, so V8 has the same module with nothing to compile for that
function. Against the published artifact, 15 runs per row:

| module | zone peak, serial | zone peak, parallel (median) | private after (median) |
|---|---|---|---|
| shipped | 7.894 MiB | 17.788 MiB | 52.883 MiB |
| minus `#2102` | 7.343 MiB | 13.338 MiB | 48.492 MiB |
| minus all three | 4.877 MiB | 10.426 MiB | 47.094 MiB |
| every body stubbed | 0.282 MiB | 0.382 MiB | 14.695 MiB |

The three hold 3.02 of 7.89 MiB serialised (38 %) and 7.36 of 17.79 MiB in
parallel (41 %). The reported 16.4 of 31.3 MiB (52 %) is the same finding on a
different V8 and a different compile window; the attribution reproduces, the
absolute numbers do not transfer.

## The one lever this repository has

Not a source split — the wasm-opt flag that builds the functions in the first
place. `-ocimfs=N` caps single-caller inlining at `N`:

| build | functions | code bytes | largest body | zone peak (parallel, 25 runs) | private (median) |
|---|---|---|---|---|---|
| current | 11,688 | 4,979,477 | 177,110 B | 20.159 MiB | 52.797 MiB |
| `-ocimfs=2000` | 11,752 | 4,979,634 (+157) | 87,456 B | 9.786 MiB | 48.117 MiB |
| `-ocimfs=200` | 12,846 | 4,964,183 (−15,294) | 59,509 B | 4.369 MiB | 48.191 MiB |

The private-memory gain saturates at `-ocimfs=2000`: cutting the zone peak by a
further 5.4 MiB buys nothing more, which is the warning about zone memory being
allocator memory playing out — most of it is already free when the window ends.
Repeating the current and `-ocimfs=200` rows across three sessions put the gap
at 4.0–4.8 MiB every time.

Compile wall time is not a reason to prefer either. One session had `-ocimfs=200`
18 % slower; two later ones had it 5 % faster (89.4 / 89.9 ms against 94.3 /
95.8). The spread between sessions is larger than the difference within one, so
there is no effect here worth quoting.

### What it costs per message

It is not free, and the cost lands on the hot path. `#2102` is the decoder every
inbound message goes through, and capping inlining is exactly what stops it
being one body. Measured by building the module with a temporary
`#[wasm_bindgen]` export over `waproto::whatsapp::Message::decode_from_slice`
(present in all three arms, so the arms stay comparable), decoding a 437-byte
`ExtendedTextMessage` with a context info and a device-list metadata block,
15,000 decodes per sample, 11 samples per round, 14 rounds interleaved across
the arms:

| build | best ns/decode | p10 | vs current |
|---|---|---|---|
| current | 3,509.9 | 3,558.3 | — |
| `-ocimfs=2000` | 3,602.6 | 3,621.6 | +2.6 % |
| `-ocimfs=200` | 3,656.8 | 3,659.4 | +4.2 % |

The medians are noise-dominated on this machine and do not separate the arms;
the minimum and the p10 both order them monotonically with the inlining cap,
which is the direction less inlining should push. Read it as a few percent, not
as a precise figure.

## Where this leaves it

The flag is **not** changed here. The trade is ~4.3 MiB of private memory at
first compile against ~3 % on every message decode, and it is measured on a
whole-module eager compile rather than on the connect window the ~37 MiB came
from. That is the caller's call to make, not this repository's, and the sibling
investigation into the `code` section's total size touches the same artifact —
landing both blind would leave neither measurable.

To make the call on better evidence, run `-ocimfs=2000` through the consumer's
own connect harness and compare private memory there; the CPU side needs a
message-rate benchmark against a real stanza stream, which nothing in this
repository can stand up (there is no mock server in CI).

What did not get measured: the connect window itself, any artifact other than a
whole-module eager compile, and `#3305`, which belongs to the core.
