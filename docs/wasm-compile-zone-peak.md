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

What does move them is the wasm-opt flag that assembles them, and it is now
set: the release pass list in `Cargo.toml` carries
`--one-caller-inline-max-function-size 2000`. "The lever, swept" below is the
sweep that picked 2000, and `check:wasm-shape` is what fails if the flag ever
goes missing.

**Two windows, two numbers, and they are four times apart.** Everything this
repository can measure is a *whole-module eager compile*, and that says the cap
is worth ~4.2 MiB of private memory. The consumer's own connect harness, which
compiles roughly a fifth of the module, measured **18.41 MiB of USS**. Neither
number is wrong; "What the eager number is, and is not" says which to quote
where. Do not carry the 4.2 into a connect-window argument, and do not carry
the 18.41 into a comparison between two artifacts here.

Everything measured *here* is reproducible with `scripts/wasm-fn-sizes.mjs` and
`scripts/wasm-zone-peak.mjs`.

## The artifact

The naming and attribution below are against the published `0.11.0`
`dist/whatsapp_rust_bridge_bg.wasm` (5,512,857 bytes, 11,688 defined functions,
median body 70 bytes). A local build of that commit — whatsapp-rust `8cea605`,
wasm-bindgen `0.2.126`, binaryen `117`, the `Cargo.toml` release flags verbatim
— reproduces its code section exactly: same 11,688 functions, same 4,979,477
body bytes, and the same three outliers at the same indices. That is what ties
the indices to names.

The sweep further down is against the same toolchain at whatsapp-rust
`7c971c82`, one commit later. Uncapped, that build has 11,733 functions and
4,990,667 body bytes — 45 functions and 11,190 bytes more than `0.11.0` — with
the same 177,110-byte largest body and the same 7.894 MiB serialised zone peak,
to three decimals. The function *indices* shift by a few; the shape does not.

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
not a prediction of a host's RSS, and the gap between the two is not a rounding
difference: see "What the eager number is, and is not".

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

## The lever, swept

Not a source split — the wasm-opt flag that builds the functions in the first
place. `-ocimfs=N` caps single-caller inlining at `N`, and it is now set to
`2000` in the release pass list.

One cargo build and one `wasm-bindgen` run feed every arm, so the arms differ by
that flag and nothing else. Serial zone peak over 3 runs (it repeats bit for
bit); parallel zone peak and private memory are medians of 15:

| `-ocimfs` | functions | code bytes | largest body | zone, serial | zone, parallel | private |
|---|---|---|---|---|---|---|
| *(none, `-1`)* | 11,733 | 4,990,667 | 177,110 B | 7.894 MiB | 18.882 MiB | 52.555 MiB |
| 8000 | 11,745 | +81 | 121,900 B | 4.956 MiB | 13.463 MiB | 50.164 MiB |
| 4000 | 11,764 | +143 | 101,933 B | 3.716 MiB | 10.474 MiB | 47.938 MiB |
| **2000** | **11,797** | **+158** | **87,456 B** | **3.528 MiB** | **9.411 MiB** | **48.383 MiB** |
| 1000 | 11,870 | +858 | 87,456 B | 3.237 MiB | 7.630 MiB | 47.965 MiB |
| 500 | 12,112 | −3,009 | 79,420 B | 2.987 MiB | 5.807 MiB | 47.906 MiB |
| 200 | 12,892 | −15,797 | 59,509 B | 1.904 MiB | 4.276 MiB | 48.586 MiB |

Three things in that table decided the value, and none of them is the zone peak,
which keeps falling all the way down and stops being worth anything long before
it stops falling:

- **Private memory saturates between 4000 and 2000.** 52.6 MiB uncapped, 50.2 at
  8000, then 47.9–48.6 for every cap from 4000 down — a 0.7 MiB band that the
  run-to-run spread covers. Cutting the zone peak from 9.4 to 4.3 MiB buys
  nothing, which is zone memory being allocator memory: most of it is already
  free when the window ends.
- **1000 is 2000 with extra steps.** Identical largest body, 73 more functions,
  700 more bytes. Below it, 500 and 200 do keep shrinking the largest body, and
  buy no memory for it.
- **8000 is half a lever.** It leaves the largest body at 121,900 B and collects
  about half the private-memory gain.

So the memory side puts the knee at 4000–2000, and the per-message side (below)
cannot separate any two arms in that range. What breaks the tie is that 2000 is
the value the consumer's connect harness actually measured — 18.41 MiB of USS,
9 rounds out of 9. Picking 4000 for a difference this sweep cannot resolve
would trade a measured result for an inferred one.

Compile wall time is not a reason to prefer any of them. Medians across the
arms sit between 87 and 101 ms in parallel mode with no ordering by cap, and the
spread between sessions is larger than the spread across the table.

### What it costs per message

It is not free, and the cost lands on the hot path. `#2102` is the decoder every
inbound message goes through, and capping inlining is exactly what stops it
being one body.

Same method as the original: each arm is built with a temporary
`#[wasm_bindgen]` export over `waproto::whatsapp::Message::decode_from_slice`
— present in every arm, so the arms stay comparable, and reverted before the
commit — decoding a 437-byte `ExtendedTextMessage` with a context info and a
device-list metadata block, 15,000 decodes per sample, 11 samples per round.

Two things were added to it, because the first pass here could not tell a real
difference from the harness's own:

- **A control arm.** The uncapped module runs twice under two names. Whatever
  separates those two is the floor, and nothing below it is a finding.
- **Rotation.** The arms run in a different order every round. With a fixed
  order, the control arm in the second slot came out 1.0 % behind the identical
  module in the first — a position effect that reads exactly like a small
  regression.

21 rounds, rotated, comparing each arm to the uncapped one round by round
(minimum of the round's 11 samples):

| `-ocimfs` | best ns/decode | paired median | rounds slower |
|---|---|---|---|
| *(none)* | 2,894.8 | — | — |
| 8000 | 2,926.7 | +1.3 % | 14/21 |
| 4000 | 2,940.7 | +1.3 % | 14/21 |
| 2000 | 2,984.9 | +2.3 % | 15/21 |
| 1000 | 3,004.9 | +1.2 % | 13/21 |
| 500 | 2,966.0 | +1.0 % | 13/21 |
| 200 | 2,928.8 | +1.6 % | 13/21 |
| *control: uncapped, twice* | 2,927.2 vs 2,926.8 | +0.6 % | 11/21 |

**Read this as a cost of roughly 1–2 % that the sweep cannot attribute to a
particular cap.** Every capped arm is slower than uncapped more often than not,
and every one of them is close enough to the control that the ordering between
them is not evidence. The earlier figures — +2.6 % at 2000, +4.2 % at 200,
monotonic in the cap — came from an unrotated harness whose control had not been
run; the direction survives, the monotonicity does not, and the magnitude at
2000 is at most what was reported and probably less.

What this still does not measure: a saturated client, or any message larger than
a short text. The decode penalty is per inbound message and the memory saving is
once per process, so a profile that decodes far more per connect trades
differently than the one below.

## What the eager number is, and is not

The tables above are a **whole-module eager compile**: every function, all at
once, in a process that does nothing else. On that comparator, `-ocimfs=2000` is
worth ~4.2 MiB of private memory. It is the right number for comparing two
artifacts in this repository, and it is the wrong number to quote to a consumer.

A real connect compiles roughly a fifth of the module — and it is the expensive
fifth, since the functions a connect touches are the ones this flag splits. The
consumer's own harness (`wabench`'s `pingpong`, driving `oxidezap/baileyrs` over
this bridge; 9 rounds with the arms interleaved, 30,000 messages at 1000 msg/s,
every source frozen in a detached worktree, USS sampled in the client process)
measured, for the cap alone and over a core already at `7c971c82`:

| metric | effect of `-ocimfs=2000` | sign | p |
|---|---|---|---|
| **USS after connect** | **−18.41 MiB (−20.7 %)** | 9/9 | 0.004 |
| peak USS | −17.19 MiB (−15.6 %) | 9/9 | 0.004 |

Reconfirmed at 4000 msg/s: −21.5 MiB across 3/3 pairs.

**That is ~4× the eager number, and the difference is not error in either.** The
eager compile pays for 11,733 functions and then reports what survives the
window; a connect pays for the few hundred that a handshake and a first message
reach, which is where the split bodies are concentrated. Quote 4.2 MiB when
comparing two builds here. Quote 18.41 MiB when talking about what a consumer
pays, and say which harness produced it — nothing in this repository can
reproduce it, because there is no mock server in CI.

## What holds it

`check:wasm-shape` fails the build when the largest function body goes over
100,000 bytes, and CI runs it beside `check:size`.

The two gates do not overlap. `check:size` watches the package's total bytes,
and the total is blind to this: the artifact that costs a consumer 18 MiB more
is **157 bytes smaller** than the one that does not. What separates them is the
distribution, and the distribution hangs off one line in a wasm-opt flag list —
a line that could be dropped while rebasing, or lost to a wasm-pack upgrade that
reorders the metadata, with nothing else to notice. Same exports, same types,
same behaviour, same size, no test to fail.

The gate is on the largest body rather than on the zone peak because the peak is
a V8 statistic: it would have to be re-baselined on every Node bump, which is a
gate that gets deleted the first time it fails for the wrong reason. The largest
body is a property of our own bytes, it is what sets the serialised peak, and it
moves with the same flag. `measure:zone-peak` still prices the peak itself when
a human wants the number.

## Where this leaves it

The flag is set to 2000, and the sweep above is why rather than the value being
inherited. What the consumer gets for it is 18.41 MiB of USS after connect
(9/9 rounds); what it pays is on the order of 1–2 % per inbound message decode
and 158 bytes of code.

What did not get measured, and would change the trade if it went the other way:

- **A saturated or chattier client.** The connect measurement runs a client that
  is ~97 % idle, exchanging short text messages. The memory saving is paid once
  per process and the decode cost is paid per message, so a profile with large
  or frequent inbound messages moves the balance towards the cost.
- **The CPU cost at a rate this harness can resolve.** 21 rotated rounds put
  every arm within 1–2 % of uncapped and within ~1 % of a same-artifact control.
  A cleaner machine, or a benchmark against a real stanza stream, could separate
  what this one could not.
- **Any cap between 2000 and 4000.** The knee is somewhere in there; the sweep
  brackets it rather than locating it, and the end-to-end evidence sits at 2000.
- **`#3305`**, which belongs to the core.
