# Host-loaded entrypoint: one approach for all runtimes

## Question

Is there ONE approach supported on all runtimes (Cloudflare Workers, Durable
Objects, workerd, Deno, other WASM-compatible hosts), or does workerd
genuinely need its own entrypoint?

## Answer

One entrypoint (`./host`, built from `ts/host.ts`) covers all of them.
workerd needs no separate path. The evidence is below; each claim was run,
not read off docs.

## What the entrypoint is

Same bridge as the default entrypoint, minus the `node:fs` read. The host
supplies the wasm as bytes or a compiled `WebAssembly.Module` and calls
wasm-bindgen's `initSync` itself — there is no wrapper, so no second
initializer name to drift:

```ts
// Cloudflare Workers / workerd: static wasm import compiles to a Module
import wasmModule from "@oxidezap/whatsapp-rust-bridge/wasm";
import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
initSync({ module: wasmModule });

// Deno: bytes (raw import, still behind --unstable-raw-imports — or readFile)
import wasmBytes from "./whatsapp_rust_bridge_bg.wasm" with { type: "bytes" };
import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
initSync({ module: wasmBytes });

// Node/Bun without the filesystem read
import { readFileSync } from "node:fs";
import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
initSync({ module: readFileSync("./whatsapp_rust_bridge_bg.wasm") });
```

`initSync` accepts either a compiled `WebAssembly.Module` or raw bytes
(`SyncInitInput`), and the host idioms produce exactly those —
workerd/wrangler static imports compile to a `WebAssembly.Module`,
Deno/raw-import and bundler `?module` styles produce bytes. A second call in
the same isolate is a no-op returning the existing instance.

## Layout

One surface, two thin shells, one shared chunk:

```text
ts/surface.ts   the whole bridge API (single source of truth)
ts/index.ts     default: reads the wasm itself, re-exports ./surface
ts/host.ts      host-loaded: re-exports ./surface + initSync

dist/bridge.js  the one implementation copy (~1.2 MB)
dist/index.js   ~1.6 KB shell: node:fs read + initSync, re-exports ./bridge.js
dist/host.js    ~1.5 KB shell: re-exports ./bridge.js + initSync
dist/*.d.ts     declarations; surface.d.ts holds the API
```

The `./wasm` export subpath (`dist/whatsapp_rust_bridge_bg.wasm`) exists so a
host can statically import the asset through the package map instead of a
relative path that resolves in the consumer's source dir (where no such file
exists after install). Under workerd/wrangler the static import compiles to
the `WebAssembly.Module` `initSync` takes.

`dist/bridge.js` is internal (not in `exports`) but both shells import it as
`./bridge.js`, and `scripts/build-shared-entrypoints.ts` keeps it that way:
it builds both entries with `--splitting`, renames the content-hashed chunk
to the stable `bridge.js`, and fails if either shell outgrows 8 KB (a shell
carrying its own bridge copy is the ~1.2 MB duplication this guards, not a
budget line to raise).

## Evidence

All probes ran against workerd 2026-09-19 (npm `workerd`), wrangler 4.135.0
(`wrangler dev`, i.e. miniflare + workerd), Deno 2.9.7, Node v22, Bun 1.4.2,
with the repo's dev-built 46 MB wasm unless noted.

1. **workerd forbids runtime wasm compilation; static imports arrive
   pre-compiled.** `new WebAssembly.Module(bytes)` and `await
   WebAssembly.compile(bytes)` both throw `CompileError: Wasm code generation
   disallowed by embedder` at global scope and inside `fetch()`
   (`WebAssembly.instantiate(bytes)` fails the same way;
   `instantiateStreaming` does not exist). A `.wasm` module listed in the
   worker config and imported (`import mod from "./x.wasm"`) arrives as
   `[object WebAssembly.Module]` — compiled by the embedder, outside the ban.
   So `initSync({ module })` with the static import is the *only* init shape
   workerd accepts, and it works: full `initSync` of the real 46 MB module
   returned `ok` in workerd.

2. **wrangler agrees.** Under `wrangler dev`, `import mod from "./tiny.wasm"`
   yields `[object WebAssembly.Module]`, and `new WebAssembly.Instance(mod,
   {})` instantiates it (answer 42). No wrangler config beyond the default is
   involved — static `.wasm` imports are a modules-format builtin.

3. **Deno takes the bytes side of the same union.** `new
   WebAssembly.Module(bytes)` works; `{ type: "bytes" }` raw imports
   (behind `--unstable-raw-imports`) yield a `Uint8Array` that instantiates
   via `await WebAssembly.instantiate(bytes)`. Plain `import "./x.wasm"`
   without the flag is a load error. End-to-end: `initSync({ module: readFileSync(...) })`
   + `initWasmEngine` + `createWhatsAppClient` over a no-op transport, then a
   client call rejecting `not-connected` — same typed error as Node.

4. **End-to-end in workerd.** The actual `dist/host.js` + `dist/bridge.js`
   with a static `WebAssembly.Module`, `initSync` at top level, codec +
   client inside `fetch()`:
   `{"initSync":"ok-top-level","codec":"ok","generic":"ok",
   "callSettled":"rejected:not-connected:not connected"}` — no
   `nodejs_compat` flag. The pure-JS codec path (`proto` encode/decode,
   `encodeProto`/`decodeProto`, `BinaryReader`, invalid-UTF8 counting with its
   report) also passes in workerd with no compat flags, because nothing in the
   shared chunk imports a `node:` builtin: `proto-reader.ts` decodes with the
   realm-shared `TextDecoder` over a view instead of `node:buffer`
   (`node:fs` is not importable in workerd at all, with or without
   `nodejs_compat` — `node:path`/`node:crypto`/`node:buffer` are, `node:fs`
   is not — which is why the default entrypoint can never run there).

5. **The one platform constraint is documented, not branched.** Client
   creation at workerd global scope panics (`could not initialize ThreadRng:
   Web Crypto API is unavailable` — `crypto.getRandomValues` throws
   `Disallowed operation called within global scope` outside a request
   context). Creating the client inside `fetch()` works. This is a call-site
   rule stated in the `ts/host.ts` doc comment, not a code path: the same
   module runs in both places.

## UTF-8 without `node:buffer`

`proto-reader.ts` used `Buffer` for two things: `toString("utf8", start, end)`
per string field, and `Buffer.from(text)` + `compare` for the
invalid-UTF8-counting re-encode check. Both now use Web Platform primitives
(realm-shared `TextDecoder`/`TextEncoder` over `subarray` views), measured
before switching because the decode path is hot:

- decode, 500 mixed messages x 60 rounds: Node `Buffer` 22.9 ms vs shared
  `TextDecoder`+`subarray` 25.1 ms (1.10x); Bun `Buffer` 21.1 ms vs shared
  `TextDecoder` 13.3 ms (0.63x, i.e. faster).
- re-encode check, 200k rounds: Node `Buffer.from` 51.6 ms vs
  `TextEncoder.encode` 207.5 ms (4.02x slower); Bun `Buffer.from` 79.9 ms vs
  `TextEncoder.encode` 20.7 ms (0.26x). The re-encode path only runs when a
  decoded string contains U+FFFD (peer-sent invalid UTF-8), so its Node cost
  lands off the hot path; the per-field decode is the one that matters and it
  is at parity.

No behavior change: substitution-with-count semantics are identical, and the
full proto suite (roundtrip, reader, int64-range, quoted-long,
numeric-input, message-merge, field-boundaries, packed-repeated, camel-long)
passes unchanged.

## What was deliberately not done

- No `./cloudflare` or `./workerd` sub-export: there is no workerd-only
  behavior to name. The `./host` name describes the axis that differs from
  the default entrypoint (host-supplied wasm, no filesystem), and it is not
  edge-specific — Node, Bun, Deno, browsers and future embedders use it too.
- No `initEdge` wrapper: it added a name without behavior (a pass-through to
  `initSync`), and its bare-input forwarding tripped wasm-bindgen's
  deprecated positional overload. The host entrypoint re-exports `initSync`
  itself.
- No size-budget raise: the two entries share one `dist/bridge.js` chunk, so
  the release package grows by ~3 KB of shells, not ~1.2 MB of duplicate
  bundle. The budget line does not move.
- No `node:` external in the portable build: `tests/host-entrypoint.test.ts`
  asserts neither shell nor the shared chunk imports any `node:` builtin, so
  a future `node:fs` introduction into that graph breaks CI instead of a
  workerd deploy.
- The default `./dist/index.js` path keeps its `node:fs` read and behavior;
  only additive `dist/host.js` + `dist/bridge.js` + declarations ship beside it.
