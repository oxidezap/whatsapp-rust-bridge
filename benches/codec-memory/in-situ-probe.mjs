/**
 * argv: <bundle.js> [touch]
 *
 * `touch` runs the traffic a ping-pong client generates, so an arm that only
 * defers work is measured after the deferral has been cashed in rather than
 * before. Deferral that nobody collects on is the failure mode this exists to
 * catch.
 */

import { readFileSync } from "node:fs";

const priv = () => Number(/^Private_Dirty:\s+(\d+) kB$/m.exec(readFileSync("/proc/self/smaps_rollup", "utf8"))[1]);
const settle = () => {
  global.gc();
  global.gc();
};

const bundle = process.argv[2];
const touch = process.argv[3] === "touch";

settle();
const before = priv();

const mod = await import(bundle);
globalThis.__keep = mod;

if (touch) {
  const { proto, encodeProto, decodeProto } = mod;
  const kept = [];
  kept.push(proto.HandshakeMessage.encode({ clientHello: { ephemeral: new Uint8Array(32) } }).finish());
  kept.push(proto.ClientPayload.encode({ username: 5511999999999, passive: false }).finish());
  kept.push(proto.ADVSignedDeviceIdentity.encode({ details: new Uint8Array(8) }).finish());
  kept.push(proto.DeviceProps.encode({ os: "bridge" }).finish());
  const wire = proto.Message.encode({ extendedTextMessage: { text: "ping", contextInfo: { stanzaId: "a" } } }).finish();
  kept.push(proto.Message.decode(wire));
  const info = proto.WebMessageInfo.encode({
    key: { remoteJid: "1@s.whatsapp.net", id: "A", fromMe: false },
    message: { conversation: "pong" },
    messageTimestamp: 1,
  }).finish();
  kept.push(proto.WebMessageInfo.decode(info));
  kept.push(encodeProto("Message", { conversation: "pong" }));
  kept.push(decodeProto("Message", wire));
  globalThis.__touched = kept;
}

settle();
const after = priv();
const heap = process.memoryUsage();
console.log(
  JSON.stringify({
    bundle,
    touch,
    delta: after - before,
    heapUsed: Math.round(heap.heapUsed / 1024),
    external: Math.round(heap.external / 1024),
    heapTotal: Math.round(heap.heapTotal / 1024),
  }),
);
