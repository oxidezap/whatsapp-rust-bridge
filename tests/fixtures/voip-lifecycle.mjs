// Exercise the published subpath in a separate Node process. No test runner
// timers belong here: after CLOSE, an owned media engine must let Node exit.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { loadVoip, MlowAudioDecoder, packetizeOpusForMlow, depacketizeOpusFromMlow } from "@oxidezap/whatsapp-rust-bridge/voip";
import { initVoipSync } from "@oxidezap/whatsapp-rust-bridge/voip/host";

const u32 = (n) => Uint8Array.of(n, n >>> 8, n >>> 16, n >>> 24);
const u64 = (n) => Uint8Array.of(...u32(n), 0, 0, 0, 0);
const concat = (...parts) => Uint8Array.from(parts.flatMap((p) => [...p]));
const bytes = (data) => concat(u32(data.length), data);
const text = (value) => bytes(new TextEncoder().encode(value));
const frame = (opcode, body) =>
  concat(Uint8Array.of(79, 90, 86, 80, 1, 0, opcode, 0, 0), u32(body.length), body);
const session = concat(u32(1), u64(1));
const reserve = frame(2, concat(session, text("LIFECYCLE"), Uint8Array.of(1)));
const format = Uint8Array.of(1, 0, ...u32(16_000), ...u32(16_000), 1,
  ...u32(960), ...u32(16_000), ...u32(960), 120);
const params = concat(
  Uint8Array.of(1), text("111111111111111:0@lid"), text("222222222222222:0@lid"),
  u32(0x57410001), Uint8Array.of(0), bytes(format),
  bytes(new Uint8Array(16).fill(0xab)), bytes(new Uint8Array(8).fill(0xcd)),
  bytes(Uint8Array.from({ length: 32 }, (_, i) => i)),
  text("203.0.113.7"), u32(3478), bytes(new TextEncoder().encode("relay-key")),
  u32(4), Uint8Array.of(1, 0, 0, 0, 0, 0), u32(0), Uint8Array.of(0),
);
const begin = frame(3, concat(session, bytes(params)));
const close = frame(8, concat(session, Uint8Array.of(1, 0)));
const transport = {
  async connect() {
    return { send() {}, async reconnect() {}, close() {} };
  },
};
const wasm = readFileSync(new URL(import.meta.resolve("@oxidezap/whatsapp-rust-bridge/voip/wasm")));
const backend = (process.env.VOIP_ENTRY === "host" ? initVoipSync(wasm, transport) : loadVoip(transport)).voipBackend;
// In host mode, initialization must reach codec helpers exported by ./voip.
const original = new Uint8Array([0xbb, 3, 1, 2]);
assert.deepEqual(depacketizeOpusFromMlow(packetizeOpusForMlow(original)), original);
const decoder = new MlowAudioDecoder();
assert.equal(decoder.decode(new Uint8Array(), undefined).length, 960);
decoder.free();
let resolveOpen;
const opened = new Promise((resolve) => { resolveOpen = resolve; });
backend.setPushHandler((payload) => {
  if (payload[6] === 4) resolveOpen(); // OPEN, not an early BEGIN_OPEN ack
  if (payload[6] === 9) throw Error("media setup failed before OPEN");
});
assert.equal((await backend.sendFrame(reserve))[7], 1);
// Shared codec state must not turn independent backends into one session table.
const independent = initVoipSync(wasm, transport).voipBackend;
assert.notEqual(independent, backend);
assert.equal((await independent.sendFrame(reserve))[7], 1);
assert.equal((await independent.sendFrame(close))[7], 1);
assert.equal((await backend.sendFrame(begin))[7], 1);
await opened;
assert.equal((await backend.sendFrame(close))[7], 1);
console.log("opened and closed");
