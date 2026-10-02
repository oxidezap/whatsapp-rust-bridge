# Private projection sharing: measured costs

PR #139 keeps both event serializer policies and their order: normal conversion
(including its genuine large-integer fallback), then the presence-preserving
proto overlay, then the unchanged key-cache/Reflect tail and event envelope.
Two private borrowed forwarding interfaces select the existing operations before
invoking them. They add indirect calls, not a serializer, allocations, clones,
public exports, or new field/default policies.

The normal default producer, with unchanged optimizer flags and ceilings, gives
an 84,288-byte largest WASM body versus 111,694 at `12ece04`. The supported
package after preserving bindgen declaration formatting is 8,296,807 bytes
against 8,300,000. This is tight headroom, not a changed budget. Unsafe declaration
aliases and ineffective event-splitting/Map-tail experiments are not adopted.

The generated JS codec separately shares unknown-field framing, ordered scalar
projection and create completion. Original fresh base construction and public
methods remain; one private flat key array replaces per-run key arrays. Only
protobuf declaration trivia is compacted. Bindgen text is retained because four
existing generated-output consumers depended on its formatting; their assertions
were not changed.

## Observed tradeoffs, not neutrality

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

The scalar copier's dynamic lookup/set loop is slower than specialized property
access. In particular, the measured `ClientPayload.fromPartial` path is **11.36×**
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
outputs are retained in the task's numbered evidence archive. These costs must
be reviewed with the package/body savings; green functional/size gates do not
turn them into a claim of runtime neutrality.
