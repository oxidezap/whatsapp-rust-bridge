import { expect, test } from "bun:test";
import { createLocalRelay } from "./fixtures/voip-local-relay.mjs";

test("queued fixture packets cannot cross a replaced connection", async () => {
  const relay = createLocalRelay(() => {});
  const oldPackets: Uint8Array[] = [], replacementPackets: Uint8Array[] = [];
  const old = await relay.transport(0).connect({}, { onPacket: (p: Uint8Array) => oldPackets.push(p) });
  old.send(Uint8Array.of(0, 3));
  old.close();
  const replacement = await relay.transport(0).connect({}, { onPacket: (p: Uint8Array) => replacementPackets.push(p) });
  old.close(); // A late close must not clear the replacement slot.
  await Promise.resolve();
  expect(oldPackets).toHaveLength(0);
  expect(replacementPackets).toHaveLength(0);
  expect(relay.activeConnections).toBe(1);
  replacement.send(Uint8Array.of(0, 3));
  await Promise.resolve();
  expect(replacementPackets).toHaveLength(1);
  replacement.close();
  expect(relay.activeConnections).toBe(0);
});

test("queued fixture RTP cannot arrive on a replacement peer", async () => {
  const relay = createLocalRelay(() => {});
  const packets: Uint8Array[] = [];
  const sender = await relay.transport(0).connect({}, { onPacket() {} });
  const receiver = await relay.transport(1).connect({}, { onPacket() {} });
  sender.send(Uint8Array.of(0x80, 120));
  receiver.close();
  const replacement = await relay.transport(1).connect({}, { onPacket: (p: Uint8Array) => packets.push(p) });
  await Promise.resolve();
  expect(packets).toHaveLength(0);
  sender.send(Uint8Array.of(0x80, 120));
  await Promise.resolve();
  expect(packets).toHaveLength(1);
  sender.close();
  replacement.close();
});
