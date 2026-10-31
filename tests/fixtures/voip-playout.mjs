// Installed production engine regression: local opaque relay, no network or client WASM.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { initVoipSync } from "@oxidezap/whatsapp-rust-bridge/voip/host";
const u32 = (n) => Uint8Array.of(n, n >>> 8, n >>> 16, n >>> 24);
const u64 = (n) => Uint8Array.of(...u32(n), 0, 0, 0, 0);
const concat = (...parts) => Uint8Array.from(parts.flatMap((p) => [...p]));
const bytes = (data) => concat(u32(data.length), data);
const text = (value) => bytes(new TextEncoder().encode(value));
const frame = (opcode, body) =>
  concat(Uint8Array.of(79, 90, 86, 80, 1, 1, opcode, 0, 0), u32(body.length), body);
const session = concat(u32(1), u64(1));
const reserve = frame(2, concat(session, text("LIFECYCLE"), Uint8Array.of(1)));
const format = Uint8Array.of(1, 0, ...u32(16_000), ...u32(16_000), 1,
  ...u32(960), ...u32(16_000), ...u32(960), 120);

const wasm = readFileSync(new URL(import.meta.resolve("@oxidezap/whatsapp-rust-bridge/voip/wasm")));
const endpoints = [], sentAt = [[], []], receivedAt = [[], []], videos = [0, 0];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const engines = [0, 1].map(index => initVoipSync(wasm, {
  async connect(_params, events) {
    endpoints[index] = events;
    return {
      send(packet) {
        const copy = packet.slice();
        if (copy[0] === 0 && copy[1] === 3) {
          const response = copy.slice(0, 20);
          response[0] = 1; response[1] = 3; response[2] = 0; response[3] = 0;
          queueMicrotask(() => events.onPacket(response));
        } else if ((copy[0] & 0xc0) === 0x80) {
          if ((copy[1] & 0x7f) === 120) sentAt[index].push(performance.now());
          queueMicrotask(() => endpoints[1-index]?.onPacket(copy));
        }
      },
      reconnect() {}, close() {},
    };
  },
}).voipBackend);
const opened = engines.map((engine, index) => new Promise(resolve => engine.setPushHandler(packet => {
  if (packet[6] === 4) resolve();
  if (packet[6] === 0x11) {
    assert.ok(packet.length > 33, "PCM samples cross the ABI");
    receivedAt[index].push(performance.now());
  }
  if (packet[6] === 0x15) videos[index]++;
})));
const params = index => concat(
  Uint8Array.of(index), text(index ? "222222222222222:0@lid" : "111111111111111:0@lid"),
  text(index ? "111111111111111:0@lid" : "222222222222222:0@lid"),
  u32(0x57410001 + index), Uint8Array.of(0), bytes(format),
  bytes(new Uint8Array(16).fill(0xab)), bytes(new Uint8Array(8).fill(0xcd)),
  bytes(Uint8Array.from({length:32}, (_, i) => i)), text("203.0.113.7"), u32(3478),
  bytes(new TextEncoder().encode("relay-key")), u32(4),
  Uint8Array.of(1, 1, 0, 0, 1, 0), u32(0), Uint8Array.of(0),
);
const close = frame(8, concat(session, Uint8Array.of(1, 0)));
try {
  for (const [index, engine] of engines.entries()) {
    assert.equal((await engine.sendFrame(reserve))[7], 1);
    assert.equal((await engine.sendFrame(frame(3, concat(session, bytes(params(index))))))[7], 1);
  }
  await Promise.all(opened);
  await sleep(25);
  const pcm = new Uint8Array(1920);
  const view = new DataView(pcm.buffer);
  for (let i=0; i<960; i++) view.setInt16(i*2, Math.round(6000*Math.sin(i*Math.PI/20)), true);
  const idr = Uint8Array.of(0,0,0,1,0x65,0x88,0x84,0x21,0xa0);
  for (let seq=0; seq<12; seq++) {
    for (const engine of engines) {
      await engine.sendFrame(frame(0x10, concat(session, u32(seq), bytes(pcm))));
      await engine.sendFrame(frame(0x14, concat(session, u32(seq), bytes(concat(bytes(idr), Uint8Array.of(0,0))))));
    }
    // Queue a burst as well as live input: decoded playout must keep its cadence.
    if (seq >= 5) await sleep(60);
  }
  await sleep(200);
  for (let index=0; index<2; index++) {
    assert.ok(receivedAt[index].length >= 5, `endpoint ${index}: actual PCM delivery (${receivedAt[index].length})`);
    assert.ok(receivedAt[index].at(-1)-receivedAt[index][0] >= 100, "playout advances over real time");
    assert.ok(sentAt[index].at(-1)-sentAt[index][0] >= 100, "RTP transmission continues over elapsed time");
    assert.ok(receivedAt[index][4]-receivedAt[index][0] >= 25, "decoded playout follows its deadlines rather than draining in one turn");
    assert.ok(videos[index] > 0, `endpoint ${index}: video delivery`);
  }
} finally {
  for (const engine of engines) await engine.sendFrame(close);
}
console.log("duplex PCM, timed playout and video delivered");
