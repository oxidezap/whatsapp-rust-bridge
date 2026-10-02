import { describe, expect, test } from "bun:test";
import { createWhatsAppClient, initWasmEngine } from "@oxidezap/whatsapp-rust-bridge";
import { loadVoip } from "@oxidezap/whatsapp-rust-bridge/voip";
import { initVoipSync } from "@oxidezap/whatsapp-rust-bridge/voip/host";
import { readFileSync } from "node:fs";
import { createHttp } from "./helpers.js";

// OZVP v1: magic | major | minor | opcode | flags LE | length LE | body.
const u32 = (n: number) => Uint8Array.of(n, n >>> 8, n >>> 16, n >>> 24);
const u64 = (n: number) => Uint8Array.of(...u32(n), 0, 0, 0, 0);
const concat = (...parts: Uint8Array[]) => Uint8Array.from(parts.flatMap((p) => [...p]));
const bytes = (data: Uint8Array) => concat(u32(data.length), data);
const text = (value: string) => bytes(new TextEncoder().encode(value));
const frame = (opcode: number, body: Uint8Array) =>
  concat(Uint8Array.of(79, 90, 86, 80, 1, 0, opcode, 0, 0), u32(body.length), body);
const session = concat(u32(1), u64(1));
const reserve = (callId: string) => frame(2, concat(session, text(callId), Uint8Array.of(0)));

// A validly framed open with a deliberately invalid self LID. Setup fails
// after the synchronous ack and pushes EVENT(MEDIA_SETUP_FAILED) to its owner.
const format = Uint8Array.of(1, 0, ...u32(16_000), ...u32(16_000), 1,
  ...u32(960), ...u32(16_000), ...u32(960), 120);
const openParams = concat(
  Uint8Array.of(0), text("not-a-lid"), text("peer@s.whatsapp.net"), u32(1),
  Uint8Array.of(0), bytes(format), bytes(new Uint8Array(16)),
  bytes(new Uint8Array(8)), bytes(new Uint8Array(32)), text("127.0.0.1"),
  u32(3478), bytes(new Uint8Array(8)), u32(4),
  Uint8Array.of(1, 0, 0, 0, 0, 0), u32(0), Uint8Array.of(0),
);
const beginOpen = frame(3, concat(session, bytes(openParams)));
const isAck = (reply: Uint8Array, opcode: number) =>
  reply[6] === opcode && reply[7] === 1; // RESPONSE without ERROR

const noRelay = { async connect() { throw Error("no relay in offline test"); } };

async function offlineClient(voipBackend: ReturnType<typeof loadVoip>["voipBackend"]) {
  return createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} }, createHttp(),
    null, null, null, null, null, null, null, { voipBackend },
  );
}

describe("two clients in one JS isolate", () => {
  test("host-supplied bytes isolate the same handle across engines", async () => {
    const wasm = readFileSync(new URL("../dist/whatsapp_rust_voip_bg.wasm", import.meta.url));
    const first = initVoipSync(wasm, noRelay).voipBackend;
    const second = initVoipSync(wasm, noRelay).voipBackend;
    expect(isAck(await first.sendFrame(reserve("HOST-A")), 2)).toBe(true);
    expect(isAck(await second.sendFrame(reserve("HOST-B")), 2)).toBe(true);
  });

  test("each loader owns handle 1 and pushes setup failure only to its own handler", async () => {
    initWasmEngine();
    const first = loadVoip(noRelay).voipBackend;
    const second = loadVoip(noRelay).voipBackend;
    const pushesA: Uint8Array[] = [];
    const pushesB: Uint8Array[] = [];
    let releaseA!: () => void;
    let releaseB!: () => void;
    const pushedA = new Promise<void>((resolve) => { releaseA = resolve; });
    const pushedB = new Promise<void>((resolve) => { releaseB = resolve; });
    first.setPushHandler((value) => { pushesA.push(value); releaseA(); });
    second.setPushHandler((value) => { pushesB.push(value); releaseB(); });

    expect(isAck(await first.sendFrame(reserve("CALL-A")), 2)).toBe(true);
    expect(isAck(await second.sendFrame(reserve("CALL-B")), 2)).toBe(true);
    expect(isAck(await first.sendFrame(beginOpen), 3)).toBe(true);
    expect(isAck(await second.sendFrame(beginOpen), 3)).toBe(true);
    await Promise.all([pushedA, pushedB]);
    expect(pushesA).toHaveLength(1);
    expect(pushesB).toHaveLength(1);
    expect(pushesA[0]![6]).toBe(9); // EVENT, not a response
    expect(pushesB[0]![6]).toBe(9);

    // The public client constructor installs two independent core handlers
    // after the direct wire probe. Both clients remain operational offline.
    const clientA = await offlineClient(first);
    const clientB = await offlineClient(second);
    try {
      for (const client of [clientA, clientB]) {
        await expect(client.acceptCallPcm("UNKNOWN")).rejects.toMatchObject({
          kind: "invalid-argument", field: "callId",
        });
      }
    } finally {
      clientA.free();
      clientB.free();
    }
  });
});
