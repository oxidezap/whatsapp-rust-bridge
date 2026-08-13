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
number below was taken on this machine, 4 cores / 16 GB, against `6f8d348`
(v0.11.0), with a locally built release wasm (`whatsapp_rust_bridge_bg.wasm`,
5,993,200 B).

### The one caveat that governs everything else

V8 grows its heap in steps far larger than what is being measured. Several arms
sit on a step boundary and their samples split into two clusters ~2.5 MiB apart;
a median over such an arm is decided by how many runs landed in each. Two
demonstrations, both from the tables below:

- On v26 the isolated 657-codec arm reports `min 11424, max 14240`. An earlier
  25-repetition run of the same artifact produced a median in the upper cluster
  and the "instantiation costs 2.7 MiB" reading that goes with it. A rebuilt,
  re-run set produced a median in the lower cluster and the opposite reading.
- Running v26 with `--predictable` (deterministic, single-threaded GC — samples
  land within 30 KiB of each other) **flips the sign** of the text-removal
  result, from −1.74 MiB to +1.70 MiB.

So `Private_Dirty` differences of this size are page-commit consequences, not
the quantity itself. Where the two disagree, the retained-memory accounting
(post-GC `heapUsed` plus `external`, which does not depend on GC configuration)
is the one to believe. It is reported alongside.

## Correcting the pre-analysis

| claim | measured | verdict |
|---|---|---|
| 658 message codecs | **657** | `grep -c 'MessageFns<'` counts the `interface MessageFns<T>` declaration too. 657 codecs, 657 interfaces, 657 `createBase*`, 212 enums, one-to-one |
| codec alone rebuilds to 1.014 MB | **1,014,386 B** | exact |
| published bundle 1.06 MB | **1,087,404 B** (1.037 MiB) | `bun build ts/index.ts --minify --target node` |
| codec is 93 % of the bundle | **81.9 %** (890,139 B) | see below |
| wasm-bindgen glue 172 KB unminified | **176,556 B** = 172.4 KiB | exact |
| ~7.9 MiB of private RSS to evaluate the bundle | **8.6 MiB** (v22) / **8.1 MiB** (v26) | confirmed |
| ~18 MiB left for JS after discounting the wasm module | **no** | see the decomposition |

**Why 81.9 % and not 93 %.** Rebuilding `ts/generated/whatsapp.ts` as its own
entry point keeps all 657 export names alive at every use site, because they are
the bundle's public surface. Inside `dist/index.js` the codec is an internal
module and the minifier renames those identifiers. Measuring by difference —
build `ts/index.ts` as it stands, then again with each codec body replaced by an
empty object under the same export name — gives 1,087,404 − 197,265 = **890,139 B**.
Still the dominant term, and the prompt's headline ("the JavaScript cost of this
package is protobuf, not the bridge") survives as a statement about *bytes*. It
does not survive as a statement about memory.

### Where the 19.7 MiB of an import actually goes

Private_Dirty delta per stage, 5 repetitions each, one process per stage:

| stage | v22.22.2 | v26.5.0 |
|---|---|---|
| `readFileSync` of the 5.99 MB wasm | +5.74 MiB | +5.73 MiB |
| … then `new WebAssembly.Module(bytes)` | +13.62 MiB | +14.69 MiB |
| … then dropping the Buffer and collecting | +13.64 MiB | +14.66 MiB |
| the same bundle with the wasm bootstrap removed | +8.58 MiB | +8.07 MiB |
| importing the library as published | +19.68 MiB | +17.36 MiB |

Two things fall out. Freeing the wasm Buffer **does not return the pages** — the
"it is collected later" line in the pre-analysis is not true of private memory
here. And the JS half of the import is **~8.1–8.6 MiB**, which is the original
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

| arm | codecs | bundle KiB | v22 PrivDirty | Δ | v26 PrivDirty | Δ |
|---|---|---|---|---|---|---|
| baseline | 0 | 12.9 | 7524 | 0 | 9748 | 0 |
| | 25 | 35.8 | 7900 | +376 | 9984 | +236 |
| | 50 | 57.9 | 8000 | +476 | 10212 | +464 |
| | 100 | 107.4 | 7924 | +400 | 10176 | +428 |
| | 200 | 205.8 | 8756 | +1232 | 10760 | +1012 |
| | 300 | 295.5 | 8784 | +1260 | 10836 | +1088 |
| ping-pong closure | 374 | 560.3 | 8540 | +1016 | 11012 | +1264 |
| | 400 | 392.7 | 8852 | +1328 | 12384 | +2636 |
| | 450 | 444.3 | 10536 | +3012 | 10956 | +1208 |
| | 500 | 497.0 | 10884 | +3360 | 11076 | +1328 |
| registry closure | 514 | 733.0 | 11272 | +3748 | 11060 | +1312 |
| | 550 | 561.7 | 11180 | +3656 | 11172 | +1424 |
| | 646 | 858.7 | 12432 | +4908 | 11424 | +1676 |
| **all, eager** | **657** | **903.1** | **12624** | **+5100** | **11472** | **+1724** |
| **all, lazy (control)** | **657** | **942.8** | **11476** | **+3952** | **11396** | **+1648** |
| 657 as a string literal | 0 | 916.0 | 9612 | +2088 | 11760 | +2012 |
| 657 read from disk | 0 | 13.0 | 8508 | +984 | 10652 | +904 |

The curve is not linear in codec count — the v22 column steps ~1.7 MiB between
400 and 450 codecs, the v26 column has an arm (400) sitting on a boundary. That
is the quantization, not a property of the codecs.

The last two rows separate *bytes* from *code*. `read from disk` holds one copy
of the same minified text as a plain string and costs ~0.9 MiB, so the bytes
alone are about a megabyte; `string literal` holds two (the module source V8
keeps alive, plus the heap string) and costs about twice that.

**The control is the `lazy` row.** Same 657 codecs, same source, every object
literal moved behind a memoised factory so nothing is constructed until it is
used — verified by round-tripping `Message` and a nested `WebMessageInfo`
through it. Deferring every construction saves **1.12 MiB on v22 and 0.07 MiB on
v26**, out of the 5.10 / 1.72 MiB the codecs cost. Removing the text saves all
of it.

### In situ: the same changes inside the real library

`bun run bench:codec-memory:in-situ` — 15 repetitions, medians, KiB, versus
`stock`. `+touch` means the process then ran the traffic a ping-pong client
generates: handshake, client payload, device identity, a `Message` encode and
decode, a `WebMessageInfo` round trip, `encodeProto`/`decodeProto`.

| arm | v22 Δ | v26 Δ | what it changes |
|---|---|---|---|
| stock +touch | +212 | +180 | the ping-pong traffic itself |
| **textcut** | **−2428** | **−1736** | 657 codec bodies gone; names and namespace work identical |
| cut (374 kept) | −908 | −576 | the largest cut a client could take |
| cut +touch | −776 | −356 | |
| **lazycodecs** | **−756** | **+184** | codec objects deferred, `proto` assembled eagerly |
| lazycodecs +touch | −680 | +428 | |
| lazyns | −2076 | −2316 | codecs eager, `proto` assembled on first read |
| lazyns +touch | −540 | −664 | |
| lazyboth | −3884 | −3300 | both — the ceiling on deferring anything |
| **lazyboth +touch** | **−1436** | **−32** | the ceiling, after the client uses six types |
| textcut-lazyns | −4388 | −3984 | the floor |

Retained memory for the two headline arms, which does not move with GC
configuration:

| | v22 heapUsed | v26 heapUsed | v26 external |
|---|---|---|---|
| stock | 9378 | 7744 | 3028 |
| textcut | 7578 | 8060 | 1967 |

v22 charges the codec text to the JS heap (−1.76 MiB) and v26 to external
memory (−1.04 MiB, −0.73 MiB net of a slightly larger heap). Either way the text
is **0.7–1.8 MiB of retained memory**, and the `Private_Dirty` deltas above are
that number amplified by page commits.

### The same arms under a deterministic GC

`NODE_FLAGS=--predictable`, 4 repetitions — samples land within 30 KiB of each
other, so the spread is gone and only the quantization regime has changed:

| arm | v22 Δ | v26 Δ |
|---|---|---|
| stock +touch | +182 | **+4084** |
| textcut | −2568 | **+1702** |
| cut | −1044 | −572 |
| lazycodecs | −780 | +180 |
| lazyns | −1848 | −2162 |
| lazyns +touch | −328 | **+3214** |
| lazyboth +touch | −1342 | **+2218** |
| textcut-lazyns | −4226 | −500 |

v22 reproduces the default configuration arm for arm. v26 does not: three arms
change sign, including the ping-pong traffic on stock, which alone commits 4 MiB
here and 0.18 MiB in the default configuration. Nothing about the codecs changed
between the two tables — only where V8 decided to grow.

## Which mechanism pays

**The text.** Three independent readings agree:

1. Isolated, the lazy control keeps 77 % (v22) / 96 % (v26) of the eager cost.
2. In situ, deferring codec construction *alone* (`lazycodecs`) is worth −0.74 MiB
   on v22 and **nothing** on v26 — because `proto-namespace.ts` reads every
   export while assembling the namespace, so the getters all fire during import
   anyway.
3. Deferring the namespace as well (`lazyboth`) looks like a 3.3–3.9 MiB win at
   import and collapses to **−1.40 MiB (v22) / −0.03 MiB (v26)** once the client
   touches six types. That is the enum-laziness result again: the page commits
   move to a later step rather than disappearing.

Removing the text is the only change whose value does not depend on whether
anyone uses the library.

## Is there a cut? Not one worth taking

The most a Baileys-compatible client can drop is 283 of 657 codecs, and that is
worth **−0.89 MiB (v22) / −0.56 MiB (v26)** of private memory — before the
constraint that actually decides it:

**`proto` is a public namespace over all 657 types.** `encodeProto("X", …)`
resolves any name in the schema, and consumers depend on that. Nothing that
removes a codec from the text can be transparent — the removed name is gone from
`proto` and from the `.d.ts`. A cut therefore has exactly one honest shape: **the
consumer declares which message types it uses, and the codec is generated for
that set.** That is an API change, not an optimization, and it is stated as one
here rather than dressed up as transparent. Its ceiling is the `textcut` row
(−1.7 to −2.4 MiB, and +1.7 MiB under `--predictable` on v26); its realistic
value is the `cut` row, under 1 MiB.

Against a ~61 MiB floor for a WhatsApp client in Node, and ~19 MiB between this
bridge and the leanest Node client, under 1 MiB for a breaking change to the
public surface is not the lever this investigation was looking for.

**What was not done, and why.** The one transparent option is a lazy `proto`
namespace with self-replacing getters — the `lazyns` arm, worth −0.53 MiB (v22)
/ −0.65 MiB (v26) after realistic use, with codec text and types untouched. It
is left unimplemented: half a megabyte, measured in the same regime where a GC
flag flips a larger result's sign, does not justify changing the shape of the
package's most-depended-on surface. The measurement is here so that decision can
be revisited with a number rather than an intuition.

## Not covered

The wasm `code` section, which is a separate constant of the same process and
has its own investigation. The wasm module compile step (+7.9 to +9.0 MiB) and
the un-returned Buffer pages (+5.7 MiB) are both larger than everything measured
here, and neither is protobuf.
