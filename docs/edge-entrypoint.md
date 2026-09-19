# Edge entrypoint: one approach for all runtimes

## Question

Is there ONE approach supported on all runtimes (Cloudflare Workers, Durable
Objects, workerd, Deno, other WASM-compatible hosts), or does workerd
genuinely need its own entrypoint?

## Answer

One entrypoint (`./edge`, built from `ts/edge.ts`) covers all of them.
workerd needs no separate path. The evidence is below; each claim was run,
not read off docs.

## What the entrypoint is

Same bridge as the default entrypoint, minus the `node:fs` read. The host
supplies the wasm as bytes or a compiled `WebAssembly.Module` and calls
`initEdge` once per isolate:

```ts
// Cloudflare Workers / workerd: static wasm import compiles to a Module
import wasmModule from "./whatsapp_rust_bridge_bg.wasm";
import { initEdge } from "@oxidezap/whatsapp-rust-bridge/edge";
initEdge(wasmModule);

// Deno: bytes (raw import, still behind --unstable-raw-imports — or readFile)
import wasmBytes from "./whatsapp_rust_bridge_bg.wasm" with { type: "bytes" };
import { initEdge } from "@oxidezap/whatsapp-rust-bridge/edge";
initEdge(wasmBytes);

// Node/Bun without the filesystem read
import { readFileSync } from "node:fs";
import { initEdge } from "@oxidezap/whatsapp-rust-bridge/edge";
initEdge(readFileSync("./whatsapp_rust_bridge_bg.wasm"));
```

`initEdge` passes its argument straight to wasm-bindgen's `initSync`, whose
declared input (`SyncInitInput = BufferSource | WebAssembly.Module`) is
exactly the union of what the host idioms produce. A second call in the same
isolate is a no-op returning the existing instance.

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
   without the flag is a load error. End-to-end: `initEdge(readFileSync(...))`
   + `initWasmEngine` + `createWhatsAppClient` over a no-op transport, then a
   client call rejecting `not-connected` — same typed error as Node.

4. **End-to-end in workerd.** `dist/edge.js` + static `WebAssembly.Module`,
   `initEdge` at top level, codec + client inside `fetch()`:
   `{"initEdge":"ok-top-level","codec":"ok","generic":"ok",
   "callSettled":"rejected:not-connected:not connected"}` — no
   `nodejs_compat` flag. The pure-JS codec path (`proto` encode/decode,
   `encodeProto`/`decodeProto`, `BinaryReader`, invalid-UTF8 counting with its
   report) also passes in workerd with no compat flags, because the edge
   bundle is built `--target browser`: `node:buffer` compiles to a bundled
   polyfill instead of an import workerd would reject (`node:fs` is not
   importable in workerd at all, with or without `nodejs_compat` —
   `node:path`/`node:crypto`/`node:buffer` are, `node:fs` is not — which is
   why the default entrypoint can never run there).

5. **The one platform constraint is documented, not branched.** Client
   creation at workerd global scope panics (`could not initialize ThreadRng:
   Web Crypto API is unavailable` — `crypto.getRandomValues` throws
   `Disallowed operation called within global scope` outside a request
   context). Creating the client inside `fetch()` works. This is a call-site
   rule stated in the `ts/edge.ts` doc comment, not a code path: the same
   module runs in both places.

## What was deliberately not done

- No `./workerd` or `./cloudflare` sub-export: there is no workerd-only
  behavior to name. The `./edge` name describes the axis that differs from
  the default entrypoint (host-supplied bytes, no filesystem).
- No async `init`: workerd bans async compilation too, so an async entry
  would accept shapes no supported host needs while suggesting workerd can
  compile at runtime. Sync `initSync` is the only primitive every host
  honors.
- No bundler magic inside the package: the entrypoint takes bytes/a Module
  rather than importing `./whatsapp_rust_bridge_bg.wasm` itself, because
  every bundler compiles that specifier differently (bun emits a path string
  by default; wrangler emits a Module) and the package cannot pick one.
- The default `./dist/index.js` path is byte-identical (sha256-pinned in
  review); only additive `dist/edge.js` + `dist/edge.d.ts` ship beside it.
