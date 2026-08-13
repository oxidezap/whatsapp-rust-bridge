# What the 657 generated codecs cost a process

`ts/generated/whatsapp.ts` is 2.45 MB of source and 890 KB of the published
bundle. The question this answers is whether that text is also memory — whether
a process that imports the bridge and touches six message types is paying for
the other 651 — and if so, which mechanism charges: **the code being present**,
or **the code being run**.

Short answer: two designs pay and they are the same order of magnitude.
Generating the codec for only the types a consumer declares is worth **−3.89 /
−2.27 MiB** and is an API change. Deferring construction **per type** is worth
**−2.08 / −1.07 MiB** after realistic use, and the only difference a consumer
can observe is a property descriptor. Every other shape of either idea is worth
nothing: deferring construction alone, deferring the namespace as one unit, or
removing codec bodies while keeping their names. The recommendation is the lazy
design. The measurement is in `benches/codec-memory/`.

## What was measured, and how

`Private_Dirty` from `/proc/self/smaps_rollup`, never `Rss`: on a machine with
other processes the file-backed half of `Rss` drifts tens of MiB without private
memory moving, and a clean page stops being private the moment a second process
maps the same file. Two full `global.gc()` passes before each reading. One
process per sample, arms interleaved round-robin so machine drift lands on all
of them, medians over the repetition counts stated per table.

Two node versions, because they disagree: **v22.22.2** and **v26.5.0**. Every
number below was taken on this machine, 4 cores / 16 GB, against `d5b6b38`
(v0.11.0 plus #57 and #58), with a locally built release wasm (`whatsapp_rust_bridge_bg.wasm`,
5,993,200 B).

### The one caveat that governs everything else

V8 grows its heap in steps far larger than what is being measured. Several arms
sit on a step boundary and their samples split into two clusters ~2.5 MiB apart;
a median over such an arm is decided by how many runs landed in each. Two
demonstrations, both from the tables below:

- On v26 the isolated 657-codec arm reports `min 2116, max 4860` over 25
  repetitions. An earlier run of the same artifact put its median in the upper
  cluster and produced the "instantiation costs 2.7 MiB" reading that goes with
  it; a rebuilt, re-run set put it in the lower cluster and produced the
  opposite one.
- Running v26 with `--predictable` (deterministic, single-threaded GC — samples
  land within 30 KiB of each other) **flips the sign** of the body-only removal,
  from −1.54 MiB to +1.64 MiB — while leaving both candidate designs where they
  were.

So `Private_Dirty` differences of this size are page-commit consequences, not
the quantity itself. Where the two disagree, the retained-memory accounting
(post-GC `heapUsed` plus `external`, which does not depend on GC configuration)
is the one to believe. It is reported alongside.

## Correcting the pre-analysis

| claim | measured | verdict |
|---|---|---|
| 658 message codecs | **657** | `grep -c 'MessageFns<'` counts the `interface MessageFns<T>` declaration too. 657 codecs, 657 interfaces, 657 `createBase*`, 212 enums, one-to-one |
| codec alone rebuilds to 1.014 MB | **1,014,386 B** | exact |
| published bundle 1.06 MB | **1,088,484 B** (1.038 MiB) | `bun build ts/index.ts --minify --target node` |
| codec is 93 % of the bundle | **81.8 %** (890,176 B) | see below |
| wasm-bindgen glue 172 KB unminified | **176,556 B** = 172.4 KiB | exact |
| ~7.9 MiB of private RSS to evaluate the bundle | **8.6 MiB** (v22) / **7.9 MiB** (v26) | confirmed |
| ~18 MiB left for JS after discounting the wasm module | **no** | see the decomposition |

**Why 81.8 % and not 93 %.** Rebuilding `ts/generated/whatsapp.ts` as its own
entry point keeps all 657 export names alive at every use site, because they are
the bundle's public surface. Inside `dist/index.js` the codec is an internal
module and the minifier renames those identifiers. Measuring by difference —
build `ts/index.ts` as it stands, then again with each codec body replaced by an
empty object under the same export name — gives 1,088,484 − 198,308 = **890,176 B**.
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
reached by no other codec             74
closure of Message                   335
closure of WebMessageInfo            364
closure of HistorySync               387
closure of ClientPayload               8
closure of the ping-pong roots       385
closure of the proto.ts registry     525
outside that closure                 132
```

A closure is transitive over the `Foo.decode(...)` / `Foo.encode(...)` calls one
codec makes into another — what has to exist for that codec to run at all.

The number that decides the shape of any cut is **335**. `Message` embeds nearly
half the schema, so decoding one inbound message can reach 335 codecs, and a
client cannot know which branch arrives. Add `WebMessageInfo`, the pairing
records and the sender-key messages and it is **385 of 657** — the largest cut a
Baileys-compatible client could take while still decoding ordinary traffic.

The 132 codecs outside even the hand-written `proto.ts` registry exist because
ts-proto emits everything in `whatsapp.proto`. They are still **public**: the
`proto` namespace resolves any type by name and Baileys-compatible consumers
depend on that.

## The experiment and its control

### Isolated: the codec layer on its own

`bun run bench:codec-memory` — 25 repetitions per arm, medians, KiB.

| arm | codecs | bundle KiB | v22 Δ | vs 0 | v26 Δ | vs 0 |
|---|---|---|---|---|---|---|
| baseline | 0 | 12.9 | 84 | 0 | 428 | 0 |
| | 25 | 35.8 | 464 | +380 | 668 | +240 |
| | 50 | 56.2 | 552 | +468 | 900 | +472 |
| | 100 | 105.7 | 648 | +564 | 852 | +424 |
| | 200 | 203.9 | 1312 | +1228 | 1432 | +1004 |
| | 300 | 291.9 | 1352 | +1268 | 1508 | +1080 |
| ping-pong closure | 385 | 566.9 | 1120 | +1036 | 1676 | +1248 |
| | 400 | 390.1 | 1412 | +1328 | 1236 | +808 |
| | 450 | 441.5 | 3080 | +2996 | 1656 | +1228 |
| | 500 | 495.9 | 3416 | +3332 | 1776 | +1348 |
| registry closure | 525 | 739.7 | 3908 | +3824 | 1752 | +1324 |
| | 550 | 559.2 | 3768 | +3684 | 1916 | +1488 |
| | 646 | 858.7 | 4964 | +4880 | 2100 | +1672 |
| **all, eager** | **657** | **903.1** | **5180** | **+5096** | **2172** | **+1744** |
| **all, lazy (control)** | **657** | **936.4** | **4028** | **+3944** | **2068** | **+1640** |
| 657 as a string literal | 0 | 903.2 | 2136 | +2052 | 2420 | +1992 |
| 657 read from disk | 0 | 13.0 | 1052 | +968 | 1320 | +892 |

Δ is each process's own before/after difference rather than its absolute
reading: two fresh node processes need not start from the same page-commit
state, and a difference in where they started would otherwise land on the codec
count. The absolute readings are in the harness output alongside.

The curve is not linear in codec count — the v22 column steps 1.63 MiB between
400 and 450 codecs, and the v26 column goes *down* from 300 to 400. Those are
quantization, not a property of the codecs.

The last two rows separate *bytes* from *code*. `read from disk` holds one copy
of the same minified text as a plain string and costs ~0.9 MiB, so the bytes
alone are about a megabyte; `string literal` holds two (the module source V8
keeps alive, plus the heap string) and costs about twice that.

**The control is the `lazy` row.** Same 657 codecs, same source, every object
literal moved behind a memoised factory so nothing is constructed until it is
used — verified by round-tripping `Message` and a nested `WebMessageInfo`
through it. Deferring every construction saves **1.13 MiB on v22 and 0.10 MiB on
v26**, out of the 4.98 / 1.70 MiB the codecs cost. Removing the text saves all
of it. This arm defers construction and nothing else; the in-situ section adds
the designs that also touch the namespace, and those are different numbers.

### In situ: the same changes inside the real library

`bun run bench:codec-memory:in-situ` — 15 repetitions, medians, KiB, versus
`stock`. `+touch` means the process then ran the traffic a ping-pong client
generates: handshake, client payload, device identity, a `Message` encode and
decode, a `WebMessageInfo` round trip, `encodeProto`/`decodeProto`.

Each `+touch` arm is measured against `stock +touch`, not against an untouched
import: the traffic itself costs 0.10–0.30 MiB, and charging that to the arm
would flatter every one of them.

| arm | v22 Δ | v26 Δ | what it changes |
|---|---|---|---|
| **textcut** | **−2532** | **−1552** | 657 codec bodies gone; export names and namespace work identical |
| cut (385 kept, rest stubbed) +touch | −1120 | −336 | codec bodies gone for the removed types, names kept |
| **cut-real (385 kept) +touch** | **−3980** | **−2324** | the removed types and their enums are not there at all |
| lazycodecs +touch | −852 | +288 | codec objects deferred, `proto` assembled eagerly |
| lazyns +touch | −924 | −712 | codecs eager, the whole tree on the first read of `proto` |
| lazyboth +touch | −1716 | −180 | both, whole-tree |
| lazyns-pertype +touch | +72 | +28 | codecs eager, one lazy getter per type |
| **lazyboth-pertype +touch** | **−2132** | **−1096** | both, per type — the shape that could ship |
| textcut-lazyns | −4376 | −3532 | the floor |

`cut` and `cut-real` are the same 385 types kept, and the difference between
them is the whole cost of a name existing: `cut` replaces the other 272 codec
bodies with stubs under the same export names, so `proto-namespace.ts` still
builds 657 wrappers and 657 paths; `cut-real` does not export them, so it does
not. `cut-real` also drops the enums nested under the removed messages and the ones
only they referenced — leaving all 212 in rebuilds `proto.HistorySync` out of
its enums alone, which is a namespace node the cut is supposed to have removed.
The stubbed arm is what "remove the bodies" is worth (−1.09 / −0.33 MiB); the
real one is what the proposed API change is worth (−3.89 / −2.27 MiB).

Three of these are about *how* the deferral is written, and the difference
between them is the whole story:

- `lazycodecs` defers each codec object but leaves `proto-namespace.ts`
  assembling eagerly, and that assembly reads every export. Every getter fires
  during import anyway, so it buys nothing on v26 and 0.82 MiB of noise on v22.
- `lazyboth` defers the namespace as one unit, behind a Proxy. Enormous at
  import, and then the first read of any property builds all 657 — which is why
  `+touch` leaves it at −0.12 MiB on v26 and worse than stock on retained memory
  on both (+0.35 / +0.27 MiB).
- `lazyboth-pertype` gives each type its own getter, so touching six types
  builds six wrappers and the codecs their decodes reach. This one does not
  collapse: **−2.09 MiB (v22) / −0.96 MiB (v26) after the client has used the
  library.** It needs both halves — `lazyns-pertype`, per-type getters over
  eager codecs, is worth nothing, because the codec objects are built either
  way and the tree of 657 getters costs about what the wrappers it defers do.

Retained memory — post-GC `heapUsed` plus `external`, summed per sample and
then medianed, because node 22 charges the codec text to the heap and node 26 to
external memory and the two move in opposite directions:

| arm | v22 retained | vs base | v26 retained | vs base |
|---|---|---|---|---|
| stock | 12564 | 0 | 10765 | 0 |
| stock +touch | 12687 | 0 | 10888 | 0 |
| **textcut** | **10773** | **−1791** | **10030** | **−735** |
| cut +touch | 12033 | −654 | 11293 | +405 |
| **cut-real +touch** | **11038** | **−1649** | **10271** | **−617** |
| lazyboth +touch | 13043 | +356 | 11169 | +281 |
| **lazyboth-pertype +touch** | **12174** | **−513** | **10277** | **−611** |
| textcut-lazyns | 10080 | −2484 | 9333 | −1432 |

The text is **0.72–1.75 MiB of retained memory**. The per-type lazy design
retains **0.50 (v22) / 0.60 (v26) MiB** less than stock after the same traffic;
the real cut retains 1.61 / 0.60 — three times as much on v22 and **the same
amount on v26**. The whole-tree lazy arm retains *more* than stock, which is the
clearest statement of what deferring at the wrong granularity does.

### The same arms under a deterministic GC

`NODE_FLAGS=--predictable`, 4 repetitions — samples land within 30 KiB of each
other, so the spread is gone and only the quantization regime has changed:

| arm | v22 Δ | v26 Δ |
|---|---|---|
| textcut | −2580 | **+1660** |
| cut +touch | −1120 | −388 |
| **cut-real +touch** | **−3760** | **−436** |
| lazycodecs +touch | −714 | +224 |
| lazyns +touch | −524 | −786 |
| lazyboth +touch | −1450 | −172 |
| lazyns-pertype +touch | +348 | −4 |
| **lazyboth-pertype +touch** | **−1960** | **−876** |
| textcut-lazyns | −4250 | **−218** |

v22 reproduces the default configuration arm for arm. v26 does not: `textcut`
flips from −1.54 to **+1.64 MiB**, and `textcut-lazyns` from −3.39 to −0.18.
Nothing about the codecs changed between the two tables — only where V8 decided
to grow.

One arm is unmoved in all four configurations: `lazyboth-pertype +touch`, at
−1.91 to −2.08 MiB on v22 and −0.86 to −1.07 on v26. `cut-real` is not: it is
−3.67 to −3.89 MiB on v22 and −2.27 in the default configuration on v26, but
only −0.43 there under `--predictable`. Its *retained* memory is stable (−1.61 /
−0.60 in both), so the cut's advantage is real and its size on node 26 is not
something to quote to two digits.

## Which mechanism pays

Both, and they are the same order of magnitude — but each only in one specific
shape, and every other shape of the same idea is worth nothing.

**Removing the types outright** is the largest single lever: **−3.89 MiB (v22) /
−2.27 MiB (v26)** for a client that keeps the 385 it can reach, and 1.61 / 0.60
MiB of retained memory. Note what has to go with the bodies for that number: the
export names and the enums under them, and therefore the 272 namespace wrappers
and paths built over them. Removing only the bodies and keeping the names
(`cut`) is worth a quarter of it, and `textcut` — every body gone, every name
kept — is the arm whose sign flips under `--predictable` on v26.

**Deferring the work** is worth **−2.08 MiB (v22) / −1.07 MiB (v26)** after a
client has used six types — the one arm that holds its value in all four
configurations — and it costs nothing at the API. But only per type. Three designs, three answers:

1. Isolated, deferring construction alone keeps 78 % (v22) / 94 % (v26) of the
   eager cost. That is the enum result: the text is still there and V8 still
   parses it.
2. In situ, deferring construction alone (`lazycodecs`) buys nothing on v26 — it
   is 0.35 MiB *worse* — because the eager namespace reads every export while
   assembling.
3. Deferring the namespace as one unit (`lazyboth`) looks like −3.8 MiB at
   import and comes back to −0.12 MiB on v26 once the first property is read,
   because that read builds all 657. Per type, it does not come back.

So the distinction the original hypothesis drew — presence versus execution — is
real, and both sides of it pay. What decides how much is **granularity**: a
deferral the client cashes in wholesale is worth nothing, and a cut that removes
bodies but keeps names is worth a third of one that removes the names too.

## The two designs, and which to take

### The transparent one: a per-type lazy namespace

Generated codecs built on first use, `proto` a plain object whose types
materialize one getter at a time. Measured as `lazyboth-pertype`: **−2.08 /
−1.07 MiB after realistic use**, −0.50 / −0.60 MiB of retained memory, stable
under both GC configurations on both node versions, and **no API change**.

`bun run bench:codec-memory:equivalence` is the reason to believe it is the same
library. Against the stock bundle it checks:

- the same 8,133 namespace paths, none missing and none extra;
- **all 657 codecs round-tripping identically** through `encode`, `decode` and
  `fromPartial` — each driven with one empty instance of every message field it
  declares, 857 nested fields, which is every cross-codec call the rewrite
  touched;
- the registry's non-generated spellings (`AdvSignedDeviceIdentity`), the
  `ADVSignedKeyIndexList` alias, and the synthesized-unknown-child behaviour of
  the four forward-compatible carriers;
- a type read for the first time *after* `Object.freeze(proto)`, which a getter
  that insists on writing itself back would fail, and the same object handed out
  twice from a frozen namespace, which an unmemoized forward-compatible wrapper
  would fail;
- 250 of 270 top-level types still unmaterialized after import, 247 after a
  ping-pong exchange.

Types do not change: nothing is removed from the schema, the `.d.ts` is
untouched, and the getters are enumerable and configurable, so `Object.keys`,
`in`, spread and `JSON.stringify` behave as they do today. First access writes
the value back as a plain property — on the namespace and in the codec module
both — so nothing pays a factory call twice.

One difference is observable, and it is inherent rather than a defect to fix:
until a type is first read, `Object.getOwnPropertyDescriptor(proto, "Message")`
returns an accessor where the eager namespace returns a writable data
descriptor. Reading, calling, enumerating, spreading and freezing all behave
identically; a consumer that inspects descriptors would see the difference. That
is the whole of what "transparent" means here.

Five details any implementation has to get right, all five found by getting them
wrong first: walking to a parent with `cursor[segment] ??= {}` *reads* the
parent, which materializes every type that has children (106 of 270 in the first
attempt); merging a child namespace with `Object.assign` reads the children, so
descriptors have to be copied instead; a rewrite that redirects `X.decode(` to a
factory has to allow for the name and the call sitting on separate lines, which
is how prettier emits the long ones — 11 cross-codec edges hide there; a
getter that cannot write itself back, because the consumer froze the namespace,
has to return the value anyway; and the wrapper it returns has to be memoized,
or a frozen namespace hands out a new object per read.

This is not implemented here. It changes the shape `scripts/gen-ts-proto.ts`
emits and the way `ts/proto-namespace.ts` assembles the package's most
depended-on surface, and that belongs in its own change with its own tests
rather than riding along with a measurement.

### The bigger one: a cut, which is an API change

`cut-real` is worth about 1.9× the lazy design on private memory and three times
as much retained memory on node 22 — though on node 26 the two retain the same
amount, and the cut's private-memory advantage there does not survive
`--predictable`. It is also not transparent, and cannot be made so:

**`proto` is a public namespace over all 657 types.** `encodeProto("X", …)`
resolves any name in the schema, and consumers depend on that. The `cut-real`
arm gets its number precisely *because* the removed names are gone from the
module — from `proto`, and from the `.d.ts` with it. A cut therefore has exactly
one honest shape: **the consumer declares which message types it uses, and the
codec is generated for that set.** That is an API change, not an optimization,
and it is stated as one here rather than dressed up as transparent.

It also does not stack cleanly with the lazy design — `textcut-lazyns` is the
floor at −4.34 / −3.47 MiB, but that arm keeps no working codec at all, so it
bounds the two together rather than pricing them.

**Recommendation: take the lazy design.** It is half the cut's private memory,
the same retained memory on node 26, and the only arm whose value holds in every
configuration measured — for a change no consumer can observe except by reading
a property descriptor. The cut is worth reopening only if someone is willing to
ship a codec generated per consumer, and against a ~61 MiB floor for a WhatsApp
client in Node, another megabyte on node 22 does not obviously buy that.

## Running it

```
bun run bench:codec-memory              # the isolated sweep, 25 reps
bun run build:wasm                      # in-situ needs pkg/
bun run bench:codec-memory:in-situ      # the ten library arms, 15 reps
bun run bench:codec-memory:equivalence  # is the lazy arm the same library?
bun run benches/codec-memory/slice.ts   # the reachability counts
```

`REPS`, `NODE_BIN` and `NODE_FLAGS` override the defaults; the tables above are
the defaults on both node versions, and the deterministic table adds
`NODE_FLAGS=--predictable`. `slice.ts` also self-checks that a slice keeping
every codec reproduces the generated file byte for byte — every count in this
document rests on that parse.

## Not covered

The wasm `code` section, which is a separate constant of the same process and
has its own investigation. The wasm module compile step (+7.9 to +8.3 MiB) and
the un-returned Buffer pages (+5.7 MiB) are both larger than everything measured
here, and neither is protobuf.
