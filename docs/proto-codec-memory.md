# What the 657 generated codecs cost a process

`ts/generated/whatsapp.ts` is 2.45 MB of source and 890 KB of the published
bundle. The question this answers is whether that text is also memory — whether
a process that imports the bridge and touches six message types is paying for
the other 651 — and if so, which mechanism charges: **the code being present**,
or **the code being run**.

Short answer: two designs pay and they are the same order of magnitude.
Generating the codec for only the types a consumer declares is worth **−3.89
MiB (v22)** and **−1.61 MiB of retained memory**, and is an API change.
Deferring construction **per type** is worth **−1.91 MiB (v22) / −0.93 (v26)**
after realistic use, and the seven differences a consumer can observe — a property
descriptor, the order `Object.keys` returns, and five corners of writing to or
redefining a property — are declared below.

Those two are the only *shippable* shapes measured. Two others save real memory
but cannot be shipped in any form: `textcut` and `cut` leave the removed types
exported and throwing, which is worse for a consumer than removing them. The
rest save nothing at all — deferring construction alone, deferring the namespace
as one unit, and per-type getters over eager codecs are each within noise of
stock once the client has used the library. The recommendation is the lazy
design. The measurement is in `benches/codec-memory/`.

## What was measured, and how

`Private_Dirty` from `/proc/self/smaps_rollup`, never `Rss`: on a machine with
other processes the file-backed half of `Rss` drifts tens of MiB without private
memory moving, and a clean page stops being private the moment a second process
maps the same file. Two full `global.gc()` passes before each reading. One
process per sample, arms interleaved under a fresh seeded permutation each
repetition so machine drift lands on all of them, medians over the repetition
counts stated per table. The permutations form a Latin square — over a
complete cycle of `arms` repetitions every arm occupies every slot once and
every arm's mean sweep position is identical — because a fixed order leaves
every arm at one position, and independent shuffles still let an arm favour a
slot over five or fifteen samples. An incomplete cycle spreads its rotations
around the cycle rather than taking them consecutively, which holds the
15-of-18 sweep's mean positions to 8.00–9.00 against an ideal 8.50 instead of
7.00–10.00. Seeded, so a run reproduces.

Two node versions, because they disagree: **v22.22.2** and **v26.5.0**. Every
number below was taken on this machine, 4 cores / 16 GB, against `d5b6b38`
(v0.11.0 plus #57 and #58), with a locally built release wasm (`whatsapp_rust_bridge_bg.wasm`,
5,993,200 B). Every artifact was built with **bun 1.3.11**, whose optimizer
decides the bytes being compared — both runners print it in their header for
that reason.

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
- Running v26 with `--predictable` — samples then land within 30 KiB of each
  other — **flips the sign** of the body-only removal, from −1.54 MiB to
  +1.64 MiB, while leaving both candidate designs where they were.
- Re-running the in-situ table on v26 with nothing changed put `cut +touch` at
  −0.33 MiB, then **+3.20**, then −0.39 across three runs, and `cut-real +touch`
  at −2.27, −0.45 and −0.41. The same three runs reproduced every v22 arm within
  ~200 KiB and reproduced the retained-memory column byte for byte on both
  versions.

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
| ~7.9 MiB of private RSS to evaluate the bundle | **11.4 MiB** (v22) / **8.0 MiB** (v26) | confirmed on v26, half again as large on v22 |
| ~18 MiB left for JS after discounting the wasm module | **no** | see the decomposition |

**Why 81.8 % and not 93 %.** Rebuilding `ts/generated/whatsapp.ts` as its own
entry point keeps all 657 export names alive at every use site, because they are
the bundle's public surface. Inside `dist/index.js` the codec is an internal
module and the minifier renames those identifiers. Measuring by difference —
build `ts/index.ts` as it stands, then again with every codec replaced by
`export const X: any = {}` under the same name — gives 1,088,484 − 198,308 =
**890,176 B**. `bench:codec-memory:in-situ` builds that second bundle and prints
the three numbers in its header. It is a *byte* control and is never measured
for memory; the `textcut` arm below keeps four throwing methods per codec
instead, because `{}` would stop `proto-namespace.ts` recognising a codec and
would remove the namespace's own work along with the text.
Still the dominant term, and the prompt's headline ("the JavaScript cost of this
package is protobuf, not the bridge") survives as a statement about *bytes*. It
does not survive as a statement about memory.

### Where the ~19.4 MiB of an import actually goes

`bun run bench:codec-memory:import-stages` — Private_Dirty delta per stage, 5
repetitions, one process per stage, each stage measured on its own rather than
by subtraction:

| stage | v22.22.2 | v26.5.0 |
|---|---|---|
| `readFileSync` of the 5,993,200 B wasm | +5.73 MiB | +5.72 MiB |
| … then `new WebAssembly.Module(bytes)` | +13.63 MiB | +13.90 MiB |
| … then dropping the Buffer and collecting | +13.61 MiB | +14.03 MiB |
| the same bundle with the wasm bootstrap removed | +11.43 MiB | +7.96 MiB |
| importing the library as published | +19.35 MiB | +16.48 MiB |

Three things fall out. Freeing the wasm Buffer **does not return the pages** —
13.63 → 13.61 MiB on v22, and on v26 the reading goes up rather than down. The
"it is collected later" line in the pre-analysis is not true of private memory
here.

The JS half of the import is **7.96 MiB on v26**, which is the original ~7.9 MiB
attribution almost exactly, and **11.43 MiB on v22**, which is half again as
much. Neither is ~18 MiB.

And **the stages do not add up**, which is the actual answer to where 18 MiB
came from: 11.43 + 13.61 = 25.04 against a 19.35 MiB total, because each stage
carries per-process costs the others also carry and the two allocation paths
share pages. A decomposition by subtracting one measured stage from the total is
therefore not valid, and that subtraction — total minus the `WebAssembly.Module`
step, leaving the un-returned Buffer pages and the instantiation on the JS side
of the ledger — is exactly how the ~18 MiB figure was produced.

An earlier, uncommitted version of this probe put the v22 JS-only stage at
8.55 MiB. The committed one says 11.43, tightly (11636–11724 KiB over five
runs), and the committed one is what this table now reports.

## Which codecs are reachable

`bun run benches/codec-memory/slice.ts`:

```text
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
| **textcut** | **−2404** | **−1556** | 657 codec bodies gone; export names and namespace work identical |
| cut (385 kept, rest stubbed) +touch | −1068 | −404 | codec bodies gone for the removed types, names kept |
| **cut-real (385 kept) +touch** | **−3980** | **−420** | the removed types and their enums are not there at all |
| lazycodecs +touch | −812 | +320 | codec objects deferred, `proto` assembled eagerly |
| lazyns +touch | −904 | −676 | codecs eager, the whole tree on the first read of `proto` |
| lazyboth +touch | −1660 | −92 | both, whole-tree |
| lazyns-pertype +touch | −52 | −24 | codecs eager, one lazy getter per type |
| **lazyboth-pertype +touch** | **−1960** | **−952** | both, per type — the shape that could ship |
| textcut-lazyns | −4308 | −3628 | the floor |

**The v26 column of this table does not reproduce between whole runs, and the
v22 column does.** Across three runs of the identical artifacts `cut +touch`
read −336, **+3276** and −404 KiB, and `cut-real +touch` read −2324, −464 and
−420, while every v22 arm came back within ~200 KiB of its previous reading. The
v26 samples say why: `stock` spans 16820–21180 KiB across fifteen repetitions,
so its median is decided by how many landed in each cluster. The retained-memory
column below does not have this property — it came back **byte for byte
identical in all three runs on both versions** — which is why the recommendation
rests on it and on v22, and why no v26 private-memory figure here is quoted to
two digits.

`cut` and `cut-real` are the same 385 types kept, and the difference between
them is the whole cost of a name existing: `cut` replaces the other 272 codec
bodies with stubs under the same export names, so `proto-namespace.ts` still
builds 657 wrappers and 657 paths; `cut-real` does not export them, so it does
not. `cut-real` also drops the enums nested under the removed messages and the ones
only they referenced — leaving all 212 in rebuilds `proto.HistorySync` out of
its enums alone, which is a namespace node the cut is supposed to have removed.
The stubbed arm is what "remove the bodies" is worth (−1.05 MiB on v22); the
real one is what the proposed API change is worth (−3.89 MiB on v22, −1.61 MiB
retained on v22 and −0.60 on v26).

Three of these are about *how* the deferral is written, and the difference
between them is the whole story:

- `lazycodecs` defers each codec object but leaves `proto-namespace.ts`
  assembling eagerly, and that assembly reads every export. Every getter fires
  during import anyway, so it buys nothing on v26 and 0.81 MiB of noise on v22.
- `lazyboth` defers the namespace as one unit, behind a Proxy. Enormous at
  import, and then the first read of any property builds all 657 — which is why
  `+touch` leaves it at −0.18 MiB on v26 and worse than stock on retained memory
  on both (+0.36 / +0.28 MiB).
- `lazyboth-pertype` gives each type its own getter, so touching six types
  builds six wrappers and the codecs their decodes reach. This one does not
  collapse: **−1.91 MiB (v22) / −0.93 MiB (v26) after the client has used the
  library.** It needs both halves — `lazyns-pertype`, per-type getters over
  eager codecs, is worth nothing, because the codec objects are built either
  way and the tree of 657 getters costs about what the wrappers it defers do.

Retained memory — post-GC `heapUsed` plus `external`, each as a per-process
delta across the import for the same reason `Private_Dirty` is, summed per
sample and then medianed, because node 22 charges the codec text to the heap and
node 26 to external memory and the two move in opposite directions:

| arm | v22 retained | vs base | v26 retained | vs base |
|---|---|---|---|---|
| stock | 6992 | 0 | 4629 | 0 |
| stock +touch | 7115 | 0 | 4751 | 0 |
| **textcut** | **5201** | **−1791** | **3894** | **−735** |
| cut +touch | 6461 | −654 | 5157 | +406 |
| **cut-real +touch** | **5466** | **−1649** | **4135** | **−616** |
| lazyboth +touch | 7481 | +366 | 5042 | +291 |
| **lazyboth-pertype +touch** | **6729** | **−386** | **4233** | **−518** |
| textcut-lazyns | 4508 | −2484 | 3196 | −1433 |

The text is **0.72–1.75 MiB of retained memory**. The per-type lazy design
retains **0.38 (v22) / 0.51 (v26) MiB** less than stock after the same traffic;
the real cut retains 1.61 / 0.60 — four times as much on v22 and **a fifth more
on v26**. The whole-tree lazy arm retains *more* than stock, which is the
clearest statement of what deferring at the wrong granularity does.

### The same arms under V8's predictable mode

`NODE_FLAGS=--predictable`, 4 repetitions — samples land within 30 KiB of each
other, so the spread is gone:

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
Nothing about the codecs changed between the two tables.

**What `--predictable` changes is not only the GC**, and the first version of
this document said it was. V8 documents the flag as "enable predictable mode"
and carries `--predictable-gc-schedule`, `--single-threaded` and
`--single-threaded-gc` as separate flags, so re-running the arms under the two
that name the GC says which half moved — 4 repetitions,
`NODE_FLAGS="--predictable-gc-schedule --single-threaded-gc"`:

| arm | v22 Δ | v26 Δ |
|---|---|---|
| textcut | −2476 | −5754 |
| **cut-real +touch** | **−4144** | **−4364** |
| lazyns +touch | −22 | +214 |
| lazyboth +touch | −1082 | −3650 |
| **lazyboth-pertype +touch** | **−1978** | **−2214** |
| textcut-lazyns | −4584 | −5136 |

Neither flip reproduces. `textcut` stays negative on v26, and `cut-real` returns
to −4.26 MiB there instead of the −0.43 it showed under `--predictable`. So the
v26 reversals belong to predictable mode as a whole — which also serializes
background compilation, the work a megabyte of lazily-compiled function bodies
generates — and not to GC scheduling. This document no longer attributes them to
where V8 grew its heap; what they establish is that on v26 the arms defined by
*text presence* are configuration-dependent, and that is claim enough to prefer
an arm that is not. These samples are also the loosest taken: on v26 `stock`
spans 16856–21080 KiB over the four, so read the sign, not the magnitude.

One arm is unmoved in all six configurations: `lazyboth-pertype +touch`, at
−1.91 to −2.08 MiB on v22 and −0.86 to −2.16 on v26. `cut-real` is not: −3.67 to
−4.05 MiB on v22, and on v26 anywhere from −0.45 to −4.26 depending on the flags
and on which run — its widest single spread comes from re-running the default
configuration, not from changing flags. Its *retained* memory is stable across
all of them (−1.61 / −0.60), so the cut's advantage is real and its size on node
26 is not something to quote at all.

## Which mechanism pays

Both, and they are the same order of magnitude — but each only in one specific
shape, and every other shape of the same idea is worth nothing.

**Removing the types outright** is the largest single lever: **−3.89 MiB on
v22** for a client that keeps the 385 it can reach, and 1.61 / 0.60 MiB of
retained memory. On v26 its private-memory figure is not reproducible enough to
quote — the retained one is. Note what has to go with the bodies for that number: the
export names and the enums under them, and therefore the 272 namespace wrappers
and paths built over them. Removing only the bodies and keeping the names
(`cut`) is worth a quarter of it, and `textcut` — every body gone, every name
kept — is the arm whose sign flips under `--predictable` on v26.

**Deferring the work** is worth **−1.91 MiB (v22) / −0.93 MiB (v26)** after a
client has used six types — the one arm that holds its value in all six
configurations — and it costs nothing at the API. But only per type. Three designs, three answers:

1. Isolated, deferring construction alone keeps 78 % (v22) / 94 % (v26) of the
   eager cost. That is the enum result: the text is still there and V8 still
   parses it.
2. In situ, deferring construction alone (`lazycodecs`) buys nothing on v26 — it
   is 0.25 MiB *worse* — because the eager namespace reads every export while
   assembling.
3. Deferring the namespace as one unit (`lazyboth`) looks like −3.2 MiB at
   import and comes back to −0.18 MiB on v26 once the first property is read,
   because that read builds all 657. Per type, it does not come back.

So the distinction the original hypothesis drew — presence versus execution — is
real, and both sides of it pay. What decides how much is **granularity**: a
deferral the client cashes in wholesale is worth nothing, and a cut that removes
bodies but keeps names is worth a third of one that removes the names too.

## The two designs, and which to take

### The transparent one: a per-type lazy namespace

Generated codecs built on first use, `proto` a plain object whose types
materialize one getter at a time. Measured as `lazyboth-pertype`: **−1.91 /
−0.93 MiB after realistic use**, −0.38 / −0.51 MiB of retained memory, stable
under all three flag configurations on both node versions and across repeat
runs, and **no API change**.

`bun run bench:codec-memory:equivalence` is the reason to believe it is the same
library. Against the stock bundle it checks:

- the same 8,133 namespace paths, none missing and none extra;
- **all 657 codecs round-tripping identically** through `encode`, `decode` and
  `fromPartial` — each driven with one empty instance of every message field it
  declares, 858 nested fields, which is every cross-codec call the rewrite
  touched;
- all five of the registry's non-generated spellings — `AdvSignedDeviceIdentity`,
  `AdvSignedKeyIndexList`, `AdvDeviceIdentity`, `AdvSignedDeviceIdentityHmac`
  and `LidMigrationMappingSyncPayload`, none of which the all-codec sweep
  reaches, since that one drives generated names — and the
  synthesized-unknown-child behaviour of the four forward-compatible carriers;
- that `ADVKeyIndexList` and `ADVSignedKeyIndexList` are both present and stay
  distinct, which is what `HISTORICAL_ALIASES` does today: it installs the alias
  only when the generated module lacks that export, and the module has it, so
  the alias never fires. An earlier version of this suite asserted the target
  alone, which only rechecked one of the 657;
- a type read for the first time *after* `Object.freeze(proto)`, the same object
  handed out twice from a frozen namespace, and assignment after
  `Object.seal(proto)` and after `Object.freeze(proto)` — a getter that insists
  on writing itself back fails the first, an unmemoized forward-compatible
  wrapper the second, a setter that insists on redefining a sealed accessor the
  third, and one that silently accepts a write a frozen data property would
  have rejected the fourth. The same write from non-strict code is reported
  rather than asserted, because there the two genuinely differ;
- 250 of 270 top-level types still unmaterialized after import and 244 after
  the same six-type workload the `+touch` arms run — both counts asserted
  absolutely, since a rewrite that materialized wrappers at import would move
  them together and pass a delta check.

Types do not change: nothing is removed from the schema, the `.d.ts` is
untouched, and the getters are enumerable and configurable, so `Object.keys`,
`in`, spread and `JSON.stringify` all see the same set of keys they see today —
in a different order, which is the second declared difference below. First
access writes the value back as a plain property — on the namespace and in the codec module
both — so nothing pays a factory call twice.

Seven differences are observable, and all seven are inherent to an accessor
rather than defects to fix:

- until a type is first read, `Object.getOwnPropertyDescriptor(proto, "Message")`
  returns an accessor where the eager namespace returns a writable data
  descriptor — an accessor is how a deferral is expressed, and a Proxy trapping
  the descriptor would have to materialize the property to answer honestly;
- `Object.keys(proto)` comes out in a different order. Today's order is the
  bundler's, over 869 individual exports of the generated module — bun emits
  them descending, so the namespace starts at `mentionMentionType` rather than
  at the first declaration. Any design that moves the codecs into one bag loses
  that, whatever order it builds in.
- assigning to a type that has never been read, from **non-strict code**, after
  `Object.freeze(proto)`: a frozen data property ignores the write silently,
  and the setter throws `TypeError` instead. A setter cannot see its caller's
  strictness, so it has to pick one behaviour for both, and throwing is the one
  that matches every strict caller — which is all ESM and all compiled
  TypeScript. Only a Proxy could defer the choice to the caller, by returning
  `false` from a `set` trap; that is a different design, unmeasured here, and it
  puts a trap on every property read of the hottest object in the package;
- `Object.defineProperty(proto, "Message", { value })` after `Object.seal(proto)`
  and before the type is read: a sealed data property is still writable, so
  stock takes the new value, while a sealed accessor is non-configurable and
  cannot become a data property, so the call throws `Cannot redefine property`.
  After a *freeze* both refuse, so this is the seal case only;
- the same call on an **unsealed** namespace, before the type is read.
  Redefining an existing data property leaves omitted attributes as they were,
  so stock stays `writable: true`; converting an accessor to a data property
  defaults them to `false`, so the next assignment throws where stock accepts
  it. `defineProperty` never reaches the accessor, so no getter or setter can
  compensate — again only a Proxy could;
- `Reflect.set(proto, "Message", value)` after `Object.freeze(proto)`: a frozen
  data property's `[[Set]]` returns `false` and leaves the caller to decide,
  while a setter runs and this one throws. Same root as the sloppy-mode case —
  an accessor cannot hand the decision back — and it is the reason both are
  listed rather than merged;
- `Reflect.set` through a **non-extensible** `Object.create(proto)` overlay,
  before the type is read: stock cannot create the own property on the receiver
  and reports `false`, while the setter tries to define it and throws. Third
  face of the same root, and the reason the receiver fix in the setter closes
  the ordinary overlay case but not this one.

Not on the list, because it was a defect and is fixed: a write through an
overlay — `Object.create(proto).Message = x` — reaches the setter with the
overlay as its receiver, and a setter that ignored that would have rewritten the
shared namespace for every holder instead of making an own property on the
overlay. The setter checks its receiver, and the harness checks the setter.

Everything else behaves identically: reading, calling, enumerating, spreading,
`JSON.stringify`, freezing, assigning after a seal, and refusing a strict-mode
assignment after a freeze. That is the whole of what "transparent" means here,
and `bench:codec-memory:equivalence` prints the key-order, sloppy-mode, both
`Reflect.set` and both redefinition divergences on every run rather than hiding
them in a set comparison.

Six details any implementation has to get right, all six found by getting them
wrong first: walking to a parent with `cursor[segment] ??= {}` *reads* the
parent, which materializes every type that has children (106 of 270 in the first
attempt); merging a child namespace with `Object.assign` reads the children, so
descriptors have to be copied instead; a rewrite that redirects `X.decode(` to a
factory has to allow for the name and the call sitting on separate lines, which
is how prettier emits the long ones — 11 cross-codec edges hide there; a
getter that cannot write itself back, because the consumer froze or sealed the
namespace, has to hold the value in its closure rather than throw; its setter
has to accept a write after a seal and refuse one after a freeze, which is what
the data property it replaces does; and the wrapper it returns has to be
memoized, or a frozen namespace hands out a new object per read.

This is not implemented here. It changes the shape `scripts/gen-ts-proto.ts`
emits and the way `ts/proto-namespace.ts` assembles the package's most
depended-on surface, and that belongs in its own change with its own tests
rather than riding along with a measurement.

### The bigger one: a cut, which is an API change

`cut-real` is worth about 2× the lazy design on private memory and four times as
much retained memory on node 22 — though on node 26 it retains only a fifth more
(0.60 against 0.51 MiB), and its private-memory advantage there reads anywhere
from −0.45 to −4.26 MiB depending on the V8 flags and on which run. It is also
not transparent, and cannot be made so:

**`proto` is a public namespace over all 657 types.** `encodeProto("X", …)`
resolves any name in the schema, and consumers depend on that. The `cut-real`
arm gets its number precisely *because* the removed names are gone from the
module — from `proto`, and from the `.d.ts` with it. A cut therefore has exactly
one honest shape: **the consumer declares which message types it uses, and the
codec is generated for that set.** That is an API change, not an optimization,
and it is stated as one here rather than dressed up as transparent.

It also does not stack cleanly with the lazy design — `textcut-lazyns` is the
floor at −4.21 / −3.54 MiB, but that arm keeps no working codec at all, so it
bounds the two together rather than pricing them.

**Recommendation: take the lazy design.** It is half the cut's private memory
on v22, within a fifth of its retained memory on node 26, and the only arm whose value holds in every
configuration measured — for a change a consumer can only observe through the
seven corners declared above. The
cut is worth reopening only if someone is willing to
ship a codec generated per consumer, and against a ~61 MiB floor for a WhatsApp
client in Node, another megabyte on node 22 does not obviously buy that.

## Running it

```sh
bun run bench:codec-memory              # the isolated sweep, 25 reps
bun run build:wasm                      # in-situ needs pkg/
bun run bench:codec-memory:in-situ      # the ten library arms, 15 reps
bun run bench:codec-memory:equivalence  # is the lazy arm the same library?
bun run bench:codec-memory:import-stages # where an import's memory goes, 5 reps
bun run benches/codec-memory/slice.ts   # the reachability counts
```

`REPS`, `NODE_BIN` and `NODE_FLAGS` override the defaults; the tables above are
the defaults on both node versions, plus `NODE_FLAGS=--predictable` and
`NODE_FLAGS="--predictable-gc-schedule --single-threaded-gc"`. `slice.ts` also self-checks that a slice keeping
every codec reproduces the generated file byte for byte — every count in this
document rests on that parse.

## Not covered

The wasm `code` section, which is a separate constant of the same process and
has its own investigation. The wasm module compile step (+7.9 to +8.3 MiB) and
the un-returned Buffer pages (+5.7 MiB) are both larger than everything measured
here, and neither is protobuf.
