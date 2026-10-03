# Private projection sharing: measured costs

PR #139 keeps both event serializer policies and their order: normal conversion
(including its genuine large-integer fallback), then the presence-preserving
proto overlay, then the unchanged key-cache/Reflect tail and event envelope.
Two private borrowed forwarding interfaces select the existing operations before
invoking them. They add indirect calls, not a serializer, allocations, clones,
public exports, or new field/default policies.

The normal default producer, with unchanged optimizer flags and ceilings, gives
an 84,288-byte largest WASM body versus 111,694 at `12ece04`. The supported
selected package after preserving bindgen declaration formatting and the bounded
performance selection is 8,298,722 bytes against 8,300,000 (1,278 bytes of room).
This is tight headroom, not a changed budget. These are the normal default
artifacts measured on 2026-10-02 after the review fixes, not the failed `12ece04`
artifact. All 15 runtime/declaration outputs are byte-identical to published
`f5d1e739e1f2dfa031d89f69da609a79821888f7`; only the README grew 21 bytes to
identify the bridge repository. That preceding head's canonical package was
8,298,701 bytes and its exact-head CI passed.
Selected WASM SHA-256:
`34e6d630f038d44f23f43ae1cf076845d98d725bdd0695fe805e874c596cc9bb`.
The canonical 17-file size includes the actual manifest/README and all 15 fresh
default outputs. Provenance edits in this non-published document do not alter those bytes.
Unsafe declaration aliases and ineffective event-splitting/Map-tail experiments
are not adopted.

The generated JS codec separately shares unknown-field framing, ordered scalar
projection and create completion. Original fresh base construction and public
methods remain; one private flat key array replaces per-run key arrays. Only
protobuf declaration trivia is compacted. Bindgen text is retained because four
existing generated-output consumers depended on its formatting; their assertions
were not changed.

## Bounded performance selection

The retained media input follows `Message` → `Message_ImageMessage` → `ContextInfo`
→ `DisappearingMode`, plus its quoted `Message`. `Message` already had no shared
scalar runs. Restoring the original scalar statements for these exact identities
and `ClientPayload` leaves 486 shared runs / 2,056 fields, with framing, create,
base construction and the native forwarding operations unchanged. This is a
producer selection, not a new codec or a general consumer-specific hot list.

One counterfactual increased the actual package by 1,894 bytes and kept both
ceilings green. Nine rotating fresh-process rounds reused the same inputs and
harness against the original, preserved candidate and narrowed candidate:

| operation | original ns/op | preserved ns/op | selected ns/op |
|---|---:|---:|---:|
| `Message.fromPartial`, conversation | 1,272 | 1,335 | 1,310 |
| `Message.fromPartial`, media/context | 3,398 | 7,924 | 3,312 |
| `ClientPayload.fromPartial` | 185 | 1,553 | 196 |
| media decode | 2,455 | 1,988 | 2,372 |
| unknown-field decode | 294 | 282 | 283 |

The measured expensive paths recovered; `ClientPayload` remained 5.9% / 11 ns
above the original in this sample. Other still-shared projections were not
benchmarked. Different sample magnitudes are retained, not spliced across rounds
to manufacture a speedup or a neutrality claim.

The selected package's 15 alternating import/touch rounds retained +57/+53 KiB
of heap versus the original. Private-dirty medians were +4,816 KiB cold and
−308 KiB after touch. The cold increase remains negative evidence; neither probe
initializes WASM, connects, or establishes a consumer's overall memory cost.
Its WASM is byte-identical to the preserved candidate, so the eager V8 results
below also apply to that same selected WASM artifact.

## Original candidate tradeoffs, retained as negative evidence

Local Node 26.5/Bun 1.4.2 results are not CI's Node 24/Bun 1.3.14. Matching the
actual built host modules in nine alternating fresh-process rounds, with 100,000
warmup and 150,000 measured operations per case, produced these median costs:

| operation | `12ece04` ns/op | candidate ns/op |
|---|---:|---:|
| exposed `Message.fromPartial`, conversation | 879 | 1,000 |
| exposed `Message.fromPartial`, media/context | 1,973 | 6,292 |
| exposed `ClientPayload.fromPartial` | 109 | 1,240 |
| media decode | 1,735 | 1,719 |
| unknown-field decode | 231 | 243 |

The original candidate's scalar copier dynamic lookup/set loop was slower than
specialized property access. In particular, the measured `ClientPayload.fromPartial` path is **11.36×**
as costly, an additional 1.13 microseconds per operation. This is not an
encode-throughput, server, or connected-runtime neutrality claim. Getter/setter
order, first-error identity, partial effects, ownership and representation are
separate executable compatibility controls, not a performance exemption.

Fifteen fresh-process import/touch probes showed candidate retained heap deltas
59/54 KiB above baseline; private-dirty medians were 4,984/272 KiB higher. The
prior per-run-array proposal also increased private-dirty by 668/716 KiB. Those
increases are retained as negative evidence, not averaged away across proposals.

The existing eager, sequential V8 WASM probe (15 samples per artifact) showed
private-memory medians 61.508 → 62.051 MiB and compile-time medians
162.8 → 220 ms. Its zone-peak statistic and eager whole-module footprint are
comparators, not connected RSS or a throughput test. Earlier source-sharing
samples also included a +0.298 MiB private-memory increase. None proves absence
of a connected-runtime regression; authenticated WhatsApp activity was not run.

Raw samples, source/artifact hashes, failed counterfactuals, and full validation
outputs are retained in the task's numbered evidence archive. Configured PR reviews and CI assess these tradeoffs with the package/body
savings; there is no separate manual cost gate. Green functional/size gates do
not turn these observations into a claim of runtime neutrality.
