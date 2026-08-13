# What the 657 generated codecs cost a process

`ts/generated/whatsapp.ts` is 2.45 MB of source and 890 KB of the published
bundle. The question this answers is whether that text is also memory — whether
a process that imports the bridge and touches six message types is paying for
the other 651 — and if so, which mechanism charges: **the code being present**,
or **the code being run**.

Short answer: the text is the mechanism, deferring the work is not, and the
whole thing is worth **0.7–1.8 MiB of retained memory** — about a tenth of what
importing the library costs, and reachable only by an API change. The
measurement is in `benches/codec-memory/`.

## What was measured, and how

`Private_Dirty` from `/proc/self/smaps_rollup`, never `Rss`: on a machine with
other processes the file-backed half of `Rss` drifts tens of MiB without private
memory moving, and a clean page stops being private the moment a second process
maps the same file. Two full `global.gc()` passes before each reading. One
process per sample, arms interleaved round-robin so machine drift lands on all
of them, medians over the repetition counts stated per table.

Two node versions, because they disagree: **v22.22.2** and **v26.5.0**. Every
number below was taken on this machine, 4 cores / 16 GB, against `c1c0fb0`
(v0.11.0 plus #57), with a locally built release wasm (`whatsapp_rust_bridge_bg.wasm`,
5,993,200 B).

### The one caveat that governs everything else

V8 grows its heap in steps far larger than what is being measured. Several arms
sit on a step boundary and their samples split into two clusters ~2.5 MiB apart;
a median over such an arm is decided by how many runs landed in each. Two
demonstrations, both from the tables below:

- On v26 the isolated 657-codec arm reports `min 2104, max 4880` over 25
  repetitions. An earlier run of the same artifact put its median in the upper
  cluster and produced the "instantiation costs 2.7 MiB" reading that goes with
  it; a rebuilt, re-run set put it in the lower cluster and produced the
  opposite one.
- Running v26 with `--predictable` (deterministic, single-threaded GC — samples
  land within 30 KiB of each other) **flips the sign** of the text-removal
  result, from −1.45 MiB to +1.64 MiB.

So `Private_Dirty` differences of this size are page-commit consequences, not
the quantity itself. Where the two disagree, the retained-memory accounting
(post-GC `heapUsed` plus `external`, which does not depend on GC configuration)
is the one to believe. It is reported alongside.

## Correcting the pre-analysis

| claim | measured | verdict |
|---|---|---|
| 658 message codecs | **657** | `grep -c 'MessageFns<'` counts the `interface MessageFns<T>` declaration too. 657 codecs, 657 interfaces, 657 `createBase*`, 212 enums, one-to-one |
| codec alone rebuilds to 1.014 MB | **1,014,386 B** | exact |
| published bundle 1.06 MB | **1,087,972 B** (1.038 MiB) | `bun build ts/index.ts --minify --target node` |
| codec is 93 % of the bundle | **81.8 %** (890,139 B) | see below |
| wasm-bindgen glue 172 KB unminified | **176,556 B** = 172.4 KiB | exact |
| ~7.9 MiB of private RSS to evaluate the bundle | **8.6 MiB** (v22) / **7.9 MiB** (v26) | confirmed |
| ~18 MiB left for JS after discounting the wasm module | **no** | see the decomposition |

**Why 81.8 % and not 93 %.** Rebuilding `ts/generated/whatsapp.ts` as its own
entry point keeps all 657 export names alive at every use site, because they are
the bundle's public surface. Inside `dist/index.js` the codec is an internal
module and the minifier renames those identifiers. Measuring by difference —
build `ts/index.ts` as it stands, then again with each codec body replaced by an
empty object under the same export name — gives 1,087,972 − 197,833 = **890,139 B**.
Still the dominant term, and the prompt's headline ("the JavaScript cost of this
package is protobuf, not the bridge") survives as a statement about *bytes*. It
does not survive as a statement about memory.

### Where the ~19.6 MiB of an import actually goes

Private_Dirty delta per stage, 5 repetitions each, one process per stage:

| stage | v22.22.2 | v26.5.0 |
|---|---|---|
| `readFileSync` of the 5.99 MB wasm | +5.74 MiB | +5.73 MiB |
| … then `new WebAssembly.Module(bytes)` | +13.60 MiB | +14.01 MiB |
| … then dropping the Buffer and collecting | +13.64 MiB | +14.00 MiB |
| the same bundle with the wasm bootstrap removed | +8.55 MiB | +7.91 MiB |
| importing the library as published | +19.59 MiB | +16.62 MiB |

Two things fall out. Freeing the wasm Buffer **does not return the pages** — the
"it is collected later" line in the pre-analysis is not true of private memory
here. And the JS half of the import is **7.9–8.6 MiB**, which is the original
~7.9 MiB attribution, not ~18 MiB. The 18 MiB figure comes from subtracting only
the `WebAssembly.Module` step from the total and leaving the un-returned Buffer
pages and the instantiation on the JS side of the ledger.

## Which codecs are reachable

`bun run benches/codec-memory/slice.ts`:

```
message codecs                       657
enums                                212
reached by no other codec             85
closure of Message                   324
closure of WebMessageInfo            353
closure of HistorySync               376
closure of ClientPayload               8
closure of the ping-pong roots       374
closure of the proto.ts registry     514
outside that closure                 143
```

A closure is transitive over the `Foo.decode(...)` / `Foo.encode(...)` calls one
codec makes into another — what has to exist for that codec to run at all.

The number that decides the shape of any cut is **324**. `Message` embeds nearly
half the schema, so decoding one inbound message can reach 324 codecs, and a
client cannot know which branch arrives. Add `WebMessageInfo`, the pairing
records and the sender-key messages and it is **374 of 657** — the largest cut a
Baileys-compatible client could take while still decoding ordinary traffic.

The 143 codecs outside even the hand-written `proto.ts` registry exist because
ts-proto emits everything in `whatsapp.proto`. They are still **public**: the
`proto` namespace resolves any type by name and Baileys-compatible consumers
depend on that.

## The experiment and its control

### Isolated: the codec layer on its own

`bun run bench:codec-memory` — 25 repetitions per arm, medians, KiB.

| arm | codecs | bundle KiB | v22 Δ | vs 0 | v26 Δ | vs 0 |
|---|---|---|---|---|---|---|
| baseline | 0 | 12.9 | 80 | 0 | 420 | 0 |
| | 25 | 35.8 | 464 | +384 | 668 | +248 |
| | 50 | 57.9 | 560 | +480 | 892 | +472 |
| | 100 | 107.4 | 484 | +404 | 856 | +436 |
| | 200 | 205.8 | 1320 | +1240 | 1440 | +1020 |
| | 300 | 295.5 | 1344 | +1264 | 1508 | +1088 |
| ping-pong closure | 374 | 560.3 | 1096 | +1016 | 1656 | +1236 |
| | 400 | 392.7 | 1416 | +1336 | 1544 | +1124 |
| | 450 | 444.3 | 3084 | +3004 | 1628 | +1208 |
| | 500 | 497.0 | 3432 | +3352 | 1756 | +1336 |
| registry closure | 514 | 733.0 | 3840 | +3760 | 1732 | +1312 |
| | 550 | 561.7 | 3744 | +3664 | 1876 | +1456 |
| | 646 | 858.7 | 4808 | +4728 | 2084 | +1664 |
| **all, eager** | **657** | **903.1** | **5168** | **+5088** | **2184** | **+1764** |
| **all, lazy (control)** | **657** | **942.8** | **4032** | **+3952** | **2080** | **+1660** |
| 657 as a string literal | 0 | 916.0 | 2164 | +2084 | 2444 | +2024 |
| 657 read from disk | 0 | 13.0 | 1064 | +984 | 1332 | +912 |

Δ is each process's own before/after difference rather than its absolute
reading: two fresh node processes need not start from the same page-commit
state, and a difference in where they started would otherwise land on the codec
count. The absolute readings are in the harness output alongside.

The curve is not linear in codec count — the v22 column steps 1.63 MiB between
400 and 450 codecs, and the v26 column climbs by a fifth of that over the same
range. Those are quantization, not a property of the codecs.

The last two rows separate *bytes* from *code*. `read from disk` holds one copy
of the same minified text as a plain string and costs ~0.9 MiB, so the bytes
alone are about a megabyte; `string literal` holds two (the module source V8
keeps alive, plus the heap string) and costs about twice that.

**The control is the `lazy` row.** Same 657 codecs, same source, every object
literal moved behind a memoised factory so nothing is constructed until it is
used — verified by round-tripping `Message` and a nested `WebMessageInfo`
through it. Deferring every construction saves **1.11 MiB on v22 and 0.10 MiB on
v26**, out of the 4.97 / 1.72 MiB the codecs cost. Removing the text saves all
of it.

### In situ: the same changes inside the real library

`bun run bench:codec-memory:in-situ` — 15 repetitions, medians, KiB, versus
`stock`. `+touch` means the process then ran the traffic a ping-pong client
generates: handshake, client payload, device identity, a `Message` encode and
decode, a `WebMessageInfo` round trip, `encodeProto`/`decodeProto`.

| arm | v22 Δ | v26 Δ | what it changes |
|---|---|---|---|
| stock +touch | +140 | +120 | the ping-pong traffic itself |
| **textcut** | **−2532** | **−1480** | 657 codec bodies gone; names and namespace work identical |
| cut (374 kept) | −980 | −548 | the largest cut a client could take |
| cut +touch | −852 | −312 | |
| **lazycodecs** | **−948** | **+188** | codec objects deferred, `proto` assembled eagerly |
| lazycodecs +touch | −800 | +424 | |
| lazyns | −2216 | −2340 | codecs eager, `proto` assembled on first read |
| lazyns +touch | −660 | −688 | |
| lazyboth | −3952 | −3264 | both — the ceiling on deferring anything |
| **lazyboth +touch** | **−1524** | **−28** | the ceiling, after the client uses six types |
| textcut-lazyns | −4368 | −3616 | the floor |

Retained memory for the two headline arms, which does not move with GC
configuration:

| | v22 heapUsed | v26 heapUsed | v26 external |
|---|---|---|---|
| stock | 9380 | 7745 | 3029 |
| textcut | 7580 | 8062 | 1967 |

v22 charges the codec text to the JS heap (−1.76 MiB) and v26 to external
memory (−1.04 MiB, −0.73 MiB net of a slightly larger heap). Either way the text
is **0.7–1.8 MiB of retained memory**, and the `Private_Dirty` deltas above are
that number amplified by page commits.

### The same arms under a deterministic GC

`NODE_FLAGS=--predictable`, 4 repetitions — samples land within 30 KiB of each
other, so the spread is gone and only the quantization regime has changed:

| arm | v22 Δ | v26 Δ |
|---|---|---|
| stock +touch | +150 | **+4104** |
| textcut | −2582 | **+1680** |
| cut | −1100 | −560 |
| lazycodecs | −806 | +208 |
| lazyns | −1872 | −2192 |
| lazyns +touch | −368 | **+3250** |
| lazyboth +touch | −1378 | **+2232** |
| textcut-lazyns | −4254 | −528 |

v22 reproduces the default configuration arm for arm. v26 does not: three arms
change sign, including the ping-pong traffic on stock, which alone commits 4 MiB
here and 0.12 MiB in the default configuration. Nothing about the codecs changed
between the two tables — only where V8 decided to grow.

## Which mechanism pays

**The text.** Three independent readings agree:

1. Isolated, the lazy control keeps 78 % (v22) / 94 % (v26) of the eager cost.
2. In situ, deferring codec construction *alone* (`lazycodecs`) is worth −0.93 MiB
   on v22 and **nothing** on v26 — because `proto-namespace.ts` reads every
   export while assembling the namespace, so the getters all fire during import
   anyway.
3. Deferring the namespace as well (`lazyboth`) looks like a 3.2–3.9 MiB win at
   import and collapses to **−1.49 MiB (v22) / −0.03 MiB (v26)** once the client
   touches six types. That is the enum-laziness result again: the page commits
   move to a later step rather than disappearing.

Removing the text is the only change whose value does not depend on whether
anyone uses the library.

## Is there a cut? Not one worth taking

The most a Baileys-compatible client can drop is 283 of 657 codecs, and that is
worth **−0.96 MiB (v22) / −0.54 MiB (v26)** of private memory — before the
constraint that actually decides it:

**`proto` is a public namespace over all 657 types.** `encodeProto("X", …)`
resolves any name in the schema, and consumers depend on that. Nothing that
removes a codec from the text can be transparent — the removed name is gone from
`proto` and from the `.d.ts`. A cut therefore has exactly one honest shape: **the
consumer declares which message types it uses, and the codec is generated for
that set.** That is an API change, not an optimization, and it is stated as one
here rather than dressed up as transparent. Its ceiling is the `textcut` row
(−1.4 to −2.5 MiB, and +1.6 MiB under `--predictable` on v26); its realistic
value is the `cut` row, under 1 MiB.

Against a ~61 MiB floor for a WhatsApp client in Node, and ~19 MiB between this
bridge and the leanest Node client, under 1 MiB for a breaking change to the
public surface is not the lever this investigation was looking for.

**What was not done, and why.** The one transparent option is a lazy `proto`
namespace with self-replacing getters — the `lazyns` arm, worth −0.64 MiB (v22)
/ −0.67 MiB (v26) after realistic use, with codec text and types untouched. It
is left unimplemented: half a megabyte, measured in the same regime where a GC
flag flips a larger result's sign, does not justify changing the shape of the
package's most-depended-on surface. The measurement is here so that decision can
be revisited with a number rather than an intuition.

## Not covered

The wasm `code` section, which is a separate constant of the same process and
has its own investigation. The wasm module compile step (+7.9 to +8.3 MiB) and
the un-returned Buffer pages (+5.7 MiB) are both larger than everything measured
here, and neither is protobuf.
