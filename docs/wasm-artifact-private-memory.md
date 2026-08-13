# What the wasm artifact costs in private memory

A process that imports this package compiles a 5.24 MiB wasm module before it
does anything else, and pays for it in memory that is private to that process.
This is what that costs, measured, and what part of it the package can give
back.

Everything below is reproducible from `benches/wasm-module-rss/`. Numbers are
node v26.5.0 on linux x64, medians of 8 repetitions, artifacts measured
round-robin rather than one at a time.

## The metric is `Private_Dirty`

Not RSS: its file-backed half moves tens of MiB with whatever else is resident
on the machine while the process's own memory does not change.

Not USS either, quite. USS is `Private_Clean + Private_Dirty`, and the clean
half is private only until a second process maps the same pages. `pair.mjs`
measures that directly — one child compiles the artifact and reads its own
numbers before a second child exists, then both read again with both resident:

```
                 solo      paired
Private_Dirty   21.59 →    21.70 MiB
Private_Clean    0.82 →     0.07 MiB
Shared_Clean    40.18 →    40.99 MiB
```

The 0.75 MiB that moves out of `Private_Clean` is node's own mapped pages, and
it is the same 0.75 MiB for every artifact in the table below — the module
contributes nothing to it. `ts/index.ts` reads the wasm with `readFileSync`
into a heap buffer rather than mapping it, and V8 then keeps its own copy. So
**the module's cost is entirely `Private_Dirty`, and every process in a fleet
pays all of it.**

## The relationship

Seven artifacts, all of them real builds of this crate — feature sets that
change what code exists, and `wasm-opt` levels that change how much room the
same code takes:

| artifact | code MiB | file MiB | functions | compile ΔPriv_Dirty | retained ΔPriv_Dirty |
|---|---|---|---|---|---|
| `minimal` (no default features) | 3.80 | 4.22 | 9 164 | 5.47 ±0.07 | **5.49 ±0.07** |
| `default`, `wasm-opt -Oz` | 4.65 | 5.14 | 12 713 | 6.67 ±0.07 | 6.69 ±0.07 |
| **`default` (what ships)** | 4.75 | 5.24 | 11 684 | 6.70 ±0.04 | **6.72 ±0.04** |
| `default`, `wasm-opt -O2` | 4.78 | 5.27 | 12 434 | 6.75 ±0.04 | 6.77 ±0.04 |
| `default + image` | 5.19 | 5.77 | 12 572 | 7.35 ±0.06 | 7.37 ±0.06 |
| `default`, `wasm-opt -O0` | 5.20 | 5.72 | 25 939 | 8.28 ±0.02 | 8.30 ±0.03 |
| `default + audio,image,sticker` | 5.61 | 6.30 | 13 482 | 7.97 ±0.05 | 7.99 ±0.05 |

`compile` is the step's own cost, `retained` is what is still there after the
wire bytes are dropped and the heap settles — they agree to within 0.03 MiB, so
the module keeps everything the compile committed.

Two artifacts in that table have nearly the same code section and differ by
1 MiB of private memory (`-O0` at 5.20 MiB and `+image` at 5.19 MiB), because
`-O0` has twice the functions. Fitting both terms accounts for all of it:

```
retained Private_Dirty = 1.052 × wire bytes + 78.2 B × functions + 0.35 MiB
R² = 0.9993, residuals ≤ 0.044 MiB across all seven
```

Read plainly: **V8 keeps a private copy of the whole artifact, byte for byte,
plus about 78 bytes of bookkeeping per declared function.** For the shipped
default that predicts 5.24×1.052 + 11 684×78.2 B + 0.35 = 6.73 MiB, against
6.72 measured.

So the finding holds, and it is not the "V8 moved the commit somewhere else"
outcome that the transient-allocation work ran into: the coefficient on wire
bytes is 1.05, not 0. A byte removed from the artifact is a byte of
`Private_Dirty` removed from every process.

### Instantiating too

`ts/index.ts` does not stop at compiling — it `initSync`s, so the shipped path
is compile *and* instantiate. `--mode=instantiate` adds an instance over stub
imports synthesised from the module's own import list:

| artifact | retained Private_Dirty, compile | + instance |
|---|---|---|
| `minimal` | 5.46 ±0.06 | 6.28 ±0.07 |
| `default` | 6.71 ±0.07 | 7.71 ±0.05 |
| `default + audio,image,sticker` | 7.99 ±0.05 | 9.25 ±0.07 |

The instance adds about 1 MiB — the initial linear-memory pages and the data
section written into them — and it scales with the artifact too, so the cut is
worth slightly more here (1.43 MiB rather than 1.23) than compiling alone.

### The lever

The lever is the *file*, not the code section specifically. The code section is
91% of the file and is where any real cut has to come from, but a byte of data
section costs the same — fitting against the code section instead gives a slope
of 1.3 MiB/MiB, which is the same relationship read through the constant ratio
between the two.

## What is in the code section

`attribute.mjs` maps each function body in the code section to its owning crate
through the wasm `name` section. Measured on a `--profile profiling` build
(same codegen settings, LTO off so a body still carries the name of the
function it came from) before `wasm-opt`, which merges bodies and makes
per-function names unreliable:

| share | crate |
|---|---|
| 17.5% | `waproto` — the generated codec for the whole WhatsApp schema |
| 17.1% | `whatsapp-rust` |
| 10.6% | `core` |
| 10.6% | `wacore` |
| 8.1% | `js_sys` — almost all of it `future_to_promise` monomorphizations |
| 5.5% | **`whatsapp-rust-bridge`** |
| 5.2% | `alloc` |
| 4.1% | `wacore-libsignal` |
| 3.2% | `hashbrown` |
| 3.0% | `serde_json` |
| 2.5% | `serde-wasm-bindgen` |
| 1.9% | `wacore-binary` |

By module, the concentrations are `whatsapp_rust::client::Client` (9.6%),
`js_sys::futures::future_to_promise` (7.3%), `waproto::whatsapp::message`
(5.7%) and `core::ptr::drop_in_place` (5.2%). The largest single bodies are
`handlers::notification::handle_notification_impl` at 40.8 KiB,
`Client::handle_retry_receipt` at 30.5 KiB and
`Client::process_classified_message` at 26.0 KiB — the core's, not this
repository's.

**This repository owns 5.5% of its own artifact.** Half of the rest is the core
and its schema, and none of that has an upstream feature to turn off: history
sync, app state and media download are reached from `connect()`, not from an
exported method, so no amount of gating on this side removes them.

## What the features are worth

Turning a bridge domain off does more than delete its exported methods: the
core paths only those methods reach stop being reachable and go with them. The
maximum available cut is `--no-default-features`, which drops every optional
domain at once:

| | code MiB | file MiB | functions | retained Private_Dirty |
|---|---|---|---|---|
| default | 4.75 | 5.24 | 11 684 | 6.72 MiB |
| no default features | 3.80 | 4.22 | 9 164 | 5.49 MiB |
| | −0.95 | −1.02 | −2 520 | **−1.23 MiB per process** |

That is 18% of what the module costs, for a client that keeps connection and
messaging and gives up everything else.

Per feature, each measured as `default` minus that one, 8 repetitions each.
This is its own run, so its `default` and `minimal` rows are re-measurements of
the two above and land 0.01–0.03 MiB apart:

| feature off | code MiB | file MiB | functions | retained Private_Dirty | saved |
|---|---|---|---|---|---|
| — (`default`) | 4.75 | 5.24 | 11 684 | 6.71 ±0.07 | — |
| `client-signal` | 4.45 | 4.93 | 11 013 | 6.39 ±0.06 | 0.32 MiB |
| `client-groups` | 4.61 | 5.09 | 11 267 | 6.55 ±0.07 | 0.16 MiB |
| `client-newsletter` | 4.68 | 5.16 | 11 435 | 6.60 ±0.05 | 0.11 MiB |
| `legacy-session` | 4.68 | 5.17 | 11 614 | 6.62 ±0.04 | 0.09 MiB |
| `client-chat-actions` | 4.69 | 5.17 | 11 449 | 6.62 ±0.04 | 0.09 MiB |
| `client-contacts` | 4.67 | 5.16 | 11 392 | 6.63 ±0.06 | 0.08 MiB |
| `client-media` | 4.69 | 5.18 | 11 519 | 6.65 ±0.03 | 0.06 MiB |
| `client-business` | 4.67 | 5.16 | 11 529 | 6.66 ±0.08 | 0.05 MiB |

Those eight sum to 0.96 MiB and together are worth 1.25 MiB, because a shared
core path only dies once its last caller is gone. Dropping one domain is worth
little; the value is in dropping several.

`measure.mjs` fits its two-term model to whatever it is given, and over these
ten artifacts alone it reports 0.80 bytes per wire byte and 184 B per function
— the two columns move together across a 0.3 MiB span, so nothing here can
separate them. Take the coefficients from the wide-range table above, where an
artifact with the same code section and twice the functions pins the second
term.

`client-media` is the clearest case of that. Turning it off removes seven
exported methods and 0.06 MiB — the core downloads history-sync blobs through
the same path, and `connect()` keeps it alive whatever the bridge exports.

### The three features named in the original hypothesis are already off

`audio`, `image` and `sticker` are **not** in the published artifact — `default`
has never included them, and `bun run build` passes no `--features`. Turning
them on is what the last two rows of the first table measure: `image` alone
costs +0.65 MiB of `Private_Dirty`, all three cost +1.27 MiB. There is nothing
to reclaim there; the cost was never being paid.

`memory-profiling` is likewise off by default.

## Building a smaller artifact

The features are additive and `default` carries all of them, so a consumer
subtracts:

```
cargo build --release --target wasm32-unknown-unknown \
  --no-default-features --features client-media,client-groups
```

`bun run build` builds `default` and is unchanged. Rebuilt across the feature
change, the default artifact keeps every section size and its function count to
the byte, and the emitted `.js` and `.d.ts` are identical files. It is not
*byte*-identical, but neither are two builds of the same unchanged source: this
crate does not build reproducibly here, and two `default` builds of identical
sources differ in 49 486 bytes of the same-sized code section.

This is a from-source path. The npm package ships one artifact, and picking
features means building the crate — there is no way to select them from a
`package.json`.

The `.d.ts` shrinks with the artifact: each domain's declarations come from the
same `#[wasm_bindgen]` block as its exports, so a gated-out domain leaves no
declaration behind. That is the failure mode worth checking — a declaration
that stayed while the type it names went would reach a consumer as a silent
`any`, the way `tests/published-dts.test.ts` describes — so the
`--no-default-features` `.d.ts` was compiled the same way that test compiles
the default one, with `skipLibCheck: false`: no errors. What remains is typed
exactly as it is today.

`getEnabledFeatures()` is deliberately not extended to report the new ones.
Adding fields to it would change the default build's types, which is exactly
what a feature nobody turns on must not do — and a gated-out domain is already
visible both in the declarations and at runtime, as an absent method.
