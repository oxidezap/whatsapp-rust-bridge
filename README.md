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
Loading the normal entrypoint does **not** load `voip.wasm`. A Node/Bun host
loads it explicitly and supplies its relay transport (ICE/DTLS/SCTP over a
pre-negotiated DataChannel) before constructing the client:

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

A host without `node:fs` imports `@oxidezap/whatsapp-rust-bridge/voip/wasm`
and calls `initVoipSync(wasm, relayTransport)` from `./voip/host` instead of
`loadVoip`. Install its returned `voipBackend` at client creation the same
way. Neither engine loader invents a relay connection: the host supplies one
implementing `VoipRelayTransport` (see `ts/voip-relay-transport.ts`).
