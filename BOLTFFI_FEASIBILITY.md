# BoltFFI feasibility for the WASM bridge

Measured against `boltffi` at `b9272fb7` (workspace version `0.29.3`, the version
published on crates.io) and this repository at `v0.7.0`.

Every claim below was produced by generating and running code, not by reading
documentation. Where the BoltFFI source and its public docs disagree, the source
is what is reported here.

## Verdict

BoltFFI can carry the bridge's **free-standing utility functions**. It cannot
carry the **client**, because the client's inbound boundary needs a callback
shape the TypeScript target cannot render, and another shape that renders but
aborts the process at runtime.

This is a limit in BoltFFI's TypeScript renderer, not a property of this
repository. Both gaps are narrow and identified to the line.

## Corrections to the brief

The task brief carried several figures that do not survive checking. They are
listed because the scope argument depends on them.

| claim | measured |
|---|---|
| `boltffi` HEAD `30028e23` | no such object; HEAD is `b9272fb7` |
| 171 exported `js_name` | 204 exported names (plus 8 `extern` *imports*, which are not exports) |
| 166 async + 13 sync = 171 | the two figures do not sum to the total they are given for |
| `wire_batch.rs` keeps `unsafe` zero-copy access | `wire_batch.rs` contains **no** `unsafe` |
| `AGENTS.md` warns about `transmute` and lifetimes there | `AGENTS.md` never mentions `transmute`, zero-copy, or lifetimes; `transmute` appears **nowhere** in `src/` |
| ~19 callback-free exports | 10 are moved here; the rest of the pure surface is listed under *Deferred* |

Confirmed as stated: there are **zero** `async fn (&mut self, …)` methods, so the
renderer's `asynchronous mutable receiver` limit is never reached.

The real `unsafe` in this repository is `Uint8Array::view` over linear memory in
`src/js_crypto.rs` (19 occurrences), the `wasm_send_sync!` macro in `src/lib.rs`,
and the counting allocator in `src/memory_profile.rs`. None of it moved in this
change.

## 1. Callbacks (Rust → JS)

The bridge does not merely export; it receives storage, transport, crypto, HTTP,
time and cache implementations *from* JavaScript and calls back into them.
`JsBackend` alone is built on three mandatory JS functions — `get`, `set`,
`delete` — with optional batch/enumeration handles probed at runtime.

BoltFFI models this with trait export (`#[export]` on a trait). Async trait
methods, structured errors, records, `Option`, and byte buffers all generate.
Two shapes do not.

### Gap A — a fallible callback with no success value

`Result<(), E>` from a JS-implemented callback fails to render, **sync and
async alike**:

| shape | result |
|---|---|
| `async fn op(..) -> Result<(), E>` | skipped — `callback async fallible success` |
| `fn op(..) -> Result<(), E>` | skipped — `callback fallible success` |

`ReturnPlan::Void` is handled in both *infallible* paths
(`callback.rs:370`, `callback.rs:551`) and in **neither** fallible-success match
(`callback.rs:322`, `callback.rs:508`). Adding a `Void` arm to those two matches
is, on inspection, the whole fix.

This is exactly `set(store, key, value) -> Promise<void>` and
`delete(store, key) -> Promise<void>` — two of the three mandatory storage
callbacks.

Worse than the failure itself: `boltffi generate` **exits 0** and prints the
skipped declaration as a table row. A build that greps for a non-zero status
sees success and silently ships a binding with the trait missing.

### Gap B — a fallible callback returning bytes aborts at runtime

This one generates cleanly, with no skip reported, and then traps:

| shape | generates | runs |
|---|---|---|
| `async fn f(..) -> Vec<u8>` (infallible) | yes | **yes** |
| `async fn f(..) -> Result<String, E>` | yes | **yes** |
| `async fn f(..) -> Result<Vec<u8>, E>` | yes | **`RuntimeError: unreachable`** |
| `async fn f(..) -> Result<Option<Vec<u8>>, E>` | yes | **`RuntimeError: unreachable`** |

The last row is `get(store, key) -> Promise<Uint8Array | null>` — the third
mandatory storage callback.

`String` and `Vec<u8>` were verified in the *same* build, so the difference is
the payload type and not the harness. The abort arrives as a wasm trap raised on
the callback-completion path, outside the promise chain, so `try`/`catch` around
the `await` does not catch it and the Node process dies. That is strictly worse
than a rejection and is incompatible with this repository's error contract,
which requires every failure to cross as a `BridgeError` the caller can inspect.

### What this costs

A backend that cannot receive storage cannot construct a client. The client
surface — the large majority of the 204 exported names — is therefore out of
reach until Gap A and Gap B are closed upstream. No workaround was applied on
this side; a JS host could be made to return a sentinel instead of `void`, and
bytes could be smuggled through a record, but both change the host-facing
contract, and the brief asks for the limit to be reported rather than papered
over.

## 2. Async on the way out

Fully supported. An exported `async fn` crosses as a real `Promise`, and a
typed error arrives as a generated `Error` subclass:

```ts
export declare function probeAsyncGet(
  s: AsyncStoreProbe, store: string, key: string,
): Promise<Uint8Array | null>;
export declare class MathErrorException extends Error { }
```

## 3. Types this repository actually uses

| type | crosses | as |
|---|---|---|
| `Vec<u8>` in / out | yes | `Uint8Array` |
| `Option<Vec<u8>>` return | yes | `Uint8Array \| null` |
| `Option<T>` scalar | yes | `T \| null` |
| `Result<T, E>` with a data-carrying error enum | yes | throws a typed `Error` subclass |
| struct with `String` + `Vec<u8>` fields | yes | `interface` |
| `Vec<String>`, `Vec<Record>` | yes | arrays |
| `f64`, `u32`, `bool` | yes | `number` / `boolean` |
| `Result<(), E>` from a callback | **no** | see Gap A |
| `Result<Vec<u8>, E>` from a callback | **no** | see Gap B |

Error enums accept tuple and struct payloads with explicit `#[repr(i32)]`
discriminants, so a message-carrying error is expressible.

`Tsify`/`serde` do not participate. BoltFFI derives its TypeScript from
`#[data]`/`#[export]`, so a type crossing both backends is declared once per
backend over one shared core definition — which is how the two record
declarations in this change are arranged.

## 4. Initialization and lifecycle

`boltffi pack wasm` emits a Node entrypoint that instantiates the module
synchronously from a sibling `_bg.wasm`, so importing the package yields a ready
module with no top-level `await` and no init call. This does **not** collide with
`initWasmEngine` / `createWhatsAppClient`: those belong to the wasm-bindgen
artifact, which is untouched and remains the package default.

## 5. Cost

Numbers are in the pull request body, which reports them alongside the
wasm-bindgen figures they should be read against.

## 6. Environment limits worth recording

- `wasm-opt` is off for the BoltFFI artifact. BoltFFI derives its flag set from
  the module's wasm features, and a bulk-memory module gets
  `--enable-bulk-memory-opt`, which binaryen 108 (the newest available here)
  rejects outright. CI needs binaryen ≥ 119 before it can be turned on.
- `boltffi`'s default features (`uuid`, `url`) do not compile for
  `wasm32-unknown-unknown` — `errno` fails with *"The target OS is unknown or
  none"*. `default-features = false` is required, not merely tidy.
- The published docs lag the source, as the repository owner warned. The
  `unsupported(...)` reasons are only discoverable by reading
  `boltffi_backend/src/target/typescript/`.

## Upstream summary

| # | limit | file | fix |
|---|---|---|---|
| A | fallible callback returning `()` is skipped (async) | `boltffi_backend/src/target/typescript/render/callback.rs:508` | add a `ReturnPlan::Void` arm |
| A | fallible callback returning `()` is skipped (sync) | `boltffi_backend/src/target/typescript/render/callback.rs:322` | add a `ReturnPlan::Void` arm |
| B | fallible callback returning bytes traps at runtime | encoder/decoder disagreement on the fallible callback-return path | make the TypeScript writer and the Rust reader agree for byte payloads |
| C | skipped declarations do not fail the command | `boltffi generate` | a `--deny-skipped` (or non-zero exit) so CI cannot ship a silently truncated binding |

C is the one worth fixing first: A and B are recoverable once seen, but a build
that reports success while dropping a trait is how a gap reaches production.
