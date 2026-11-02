# @oxidezap/whatsapp-rust-bridge

[![npm version](https://img.shields.io/npm/v/@oxidezap/whatsapp-rust-bridge)](https://www.npmjs.com/package/@oxidezap/whatsapp-rust-bridge)
[![npm downloads](https://img.shields.io/npm/dm/@oxidezap/whatsapp-rust-bridge)](https://www.npmjs.com/package/@oxidezap/whatsapp-rust-bridge)
[![pkg.pr.new](https://pkg.pr.new/badge/oxidezap/whatsapp-rust-bridge)](https://pkg.pr.new/~/oxidezap/whatsapp-rust-bridge)

High-performance WhatsApp utilities powered by Rust and WebAssembly.

## Features

| Feature                        | Status |
| ------------------------------ | ------ |
| Binary Protocol                | ✅     |
| Libsignal                      | ✅     |
| App State Sync                 | ✅     |
| Audio (waveform, duration)     | ✅     |
| Image (thumbnails, conversion) | ✅     |
| Sticker Metadata               | ✅     |

## Preview builds

Every commit CI builds and tests — on `main` and on every pull request — then
uploads that build to
[pkg.pr.new](https://github.com/stackblitz-labs/pkg.pr.new), installable from a
URL. Nothing is published to npm, so a fix can be tried before it reaches a
release:

```sh
# the head of a pull request, following it as new commits land
npm install https://pkg.pr.new/@oxidezap/whatsapp-rust-bridge@88

# one specific commit
npm install https://pkg.pr.new/@oxidezap/whatsapp-rust-bridge@7ef8db0
```

Every open pull request comments the URL for its own head, and
[pkg.pr.new/~/oxidezap/whatsapp-rust-bridge](https://pkg.pr.new/~/oxidezap/whatsapp-rust-bridge)
lists what is available. Two things leave a commit without one. A release
version bump is not built at all — CI skips a diff that cannot reach the build,
and that commit is on its way to npm anyway. And the upload does not fail the
run: pkg.pr.new is not this repository's to keep up, so a green commit with no
preview means it was unreachable, not that the build was bad.

The tarball is the one the run built and tested: the same `bun run build`
output, packed through the same `prepack` guard a real publish goes through.
What it is not is a release. Preview builds carry the version
`0.0.0-preview-<sha>`, which no range written for a real release can match — an
install is a deliberate pin, and it stays on that commit until you change it.
They are for trying a change, not for running one.

## Persistent storage

Persistent storage hosts must implement all six `JsStoreCallbacks` methods:
`get`, `set`, `delete`, `commitPatch`, `touchTcTokenSenderTimestamp`, and
`storeReceivedTcToken`. Missing or non-function callbacks reject client
construction with `kind: "invalid-argument"`, `field: "store"`, before storage I/O.
The built-in in-memory backend requires no JS callbacks.

`commitPatch` must atomically persist the version and MAC changes, including
the self-index when requested. Token callbacks must merge against the latest
row, preserve the other timestamp/token fields, and share serialization with
every writer in the device scope. Timestamps cross as signed decimal i64 strings.
Use a transaction, CAS, or a lock shared by all relevant adapters/processes;
separate writes, a non-transactional batch, or an adapter-local mutex are insufficient.
Resolve only at the host's persistence boundary and propagate post-commit errors.
The generated callback declarations describe the existing namespaces and byte layouts.

## Host-loaded runtimes

On Node and Bun the default entrypoint finds and loads the wasm itself:

```ts
import { createWhatsAppClient } from "@oxidezap/whatsapp-rust-bridge";
```

Hosts without filesystem access (Cloudflare Workers, workerd, Deno,
browsers) supply the wasm instead. Import the asset through the `./wasm`
subpath — a static import, which workerd/wrangler compile to a
`WebAssembly.Module` — and pass it to `initSync` from `./host`:

```ts
import wasm from "@oxidezap/whatsapp-rust-bridge/wasm";
import {
  initSync,
  createWhatsAppClient,
} from "@oxidezap/whatsapp-rust-bridge/host";

initSync({ module: wasm });
```

Call `initSync` once per isolate, and on workerd create clients inside a
request handler rather than at global scope (`crypto.getRandomValues` is
unavailable during global-scope evaluation).

## VoIP media (opt-in engine)

`acceptCall`/`dialCall` (encoded audio), `acceptCallPcm`/`dialCallPcm`
(decoded mono PCM), video push, call callbacks and stats are available on the
default client, but media requires the separate engine.
Loading the normal entrypoint does **not** load `voip.wasm`. Call `loadVoip`
(or `initVoipSync`) once **per client**: each call creates an independent WASM
instance, so two sockets cannot exchange session handles, relay transports or
media callbacks. A Node/Bun host supplies a relay transport (ICE/DTLS/SCTP over
a pre-negotiated DataChannel) before constructing each client:

```ts
import { createWhatsAppClient, initWasmEngine } from "@oxidezap/whatsapp-rust-bridge";
import { loadVoip, MlowAudioDecoder, depacketizeOpusFromMlow } from "@oxidezap/whatsapp-rust-bridge/voip";

initWasmEngine();
const { voipBackend } = loadVoip(relayTransport);
const client = await createWhatsAppClient(
  transport, httpClient,
  { onEvent(event) {}, onCallAudio(frame) {}, onCallPcm(frame) {}, onCallVideo(frame) {}, onCallEvent(event) {} },
  null, null, null, null, null, null, { voipBackend },
);
const call = await client.acceptCall(callId, "opus-mlow", false, offerHandle);
client.callPushAudio(call.callId, opusCeltPacket);
await call.terminate();
```

`offerHandle` comes from the incoming offer event; a `callId` alone also
works. For decoded audio, call `acceptCallPcm(callId)` or `dialCallPcm(peer)`, pass
one `Int16Array(960)` of mono 16 kHz PCM to `callPushPcm16(callId, samples)`,
and handle decoded `Int16Array` frames in `onCallPcm`; voip.wasm handles
MLOW encoding and decoding. Both PCM and encoded methods return `WasmCallHandle`
(with `callId`,
`mediaStats`, `waitEnded`, and video controls); `endCall(callId)` and
`getCallMediaStats(callId)` are available on the client. Video push accepts
encoded H.264 Annex-B access units, not raw pixels. The engine converts
outgoing CELT Opus to MLOW escape on an `"opus-mlow"` call; inbound MLOW can
be decoded with a stateful `MlowAudioDecoder` per call. The codec helpers are
exported only from `./voip`, not the core entrypoint.
`rejectVideo()` currently rejects with an explicit capability error: the pinned
core has no token-safe peer rejection operation. It does not stop local video
or consume the pending request. `acceptVideo()` and `stopVideo()` retain their
separate operations.
The supplied engine flushes final counters synchronously on local close; custom
plugins that defer CLOSE retain their last observed snapshot at synchronous teardown.

A host without `node:fs` imports `@oxidezap/whatsapp-rust-bridge/voip/wasm`
and calls `initVoipSync(wasm, relayTransport)` from `./voip/host` instead of
`loadVoip`. Install its returned `voipBackend` at client creation the same
way. Neither engine loader invents a relay connection: the host supplies one
implementing `VoipRelayTransport` (see `ts/voip-relay-transport.ts`).

### Calling over an existing host

`createCallControl` attaches the same direct-call control and media queues to a
host that already owns its WhatsApp connection and Signal repository. It does
not construct a `WasmWhatsAppClient`, open another connection, or copy sessions.
The host implements the exported `DirectCallHostCallbacks` contract.

```ts
import { createCallControl } from "@oxidezap/whatsapp-rust-bridge";

const control = await createCallControl(host, {
  onEvent(event) { handleCallSignaling(event); },
  onCallEvent(event) { handleCallMedia(event); },
  onCallPcm(frame) { consumePcm(frame); },
}, { voipBackend });
try {
  const call = await control.dialCallPcm(peer);
  control.callPushPcm16(call.callId, new Int16Array(960));
  await control.endCall(call.callId);
} finally {
  control.disconnect();
  control.free();
}
```

Feed raw incoming `<call>` nodes to `control.receiveNode(node)`. The host keeps
ownership of generic stanza ACKs; shared Rust control emits distinct call
receipts. `watchAck` must register synchronously before the offer is sent and
return a cancellable registration. Delegate encryption, decryption, token
storage and identity learning to the existing host's authoritative operations.
An identity-learning callback must reject when that host cannot safely apply
its mapping, privacy and session-migration policy.
`readToken` returns the stored token and timestamp; shared Rust applies its
default receiver-side expiry policy without changing the stored record. Token
issuance is best effort and does not gate media readiness; a host must supply
its own authoritative issuance/deduplication policy.

The standalone API currently supports 1:1 PCM audio. Incoming offers retained by
`receiveNode` can be answered with `acceptCallPcm(callId)`. It returns the same
`WasmCallHandle` as the client API; group and video operations require native
call control. `getCallMediaStats` retains the last eight ended calls.
`disconnect` cancels pending bridge operations and media without closing the
host's connection. It cannot undo host operations that already committed.
Errors from key preparation, decryption, token reads and node sends propagate
with operation-specific messages; the standalone boundary does not preserve
every host-specific error kind. Alias lookup failures refuse identity matching.

The constructor bounds the media-backend handshake to 20 seconds and installs
its required clocks; it does not require
`initWasmEngine` or a messaging client. Host-supplied WASM initialization and the
existing `./host` entrypoint remain available. The installed-package fixtures
exercise standalone control and real media WASM over a synthetic relay with
fixture-only encryption. They do not validate authenticated WhatsApp calls,
Signal interoperability, hardware audio or production network traversal.
