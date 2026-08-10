/**
 * A divergence report measured fifteen fields against a schema that is not the
 * one this bridge compiles. These tests pin what the consumed schema — the
 * `waproto` crate Cargo.lock resolves — actually declares, so a dependency bump
 * that introduces any of those fields fails here instead of quietly reopening
 * the question.
 *
 * Run: bun test tests/proto-schema-divergences.test.ts
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { initWasmEngine, encodeProto, decodeProto } from "../dist/index.js";

const SURFACE_FILE = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "ts",
  "generated",
  "whatsapp-surface.txt",
);

const SURFACE = readFileSync(SURFACE_FILE, "utf8")
  .split("\n")
  .filter((line) => line.length > 0)
  .map((line) => line.split("\t"));

const declares = (message: string, field: string): boolean =>
  SURFACE.some((columns) => columns[0] === message && columns[1] === field);

const numberOf = (message: string, field: string): number | undefined => {
  const columns = SURFACE.find((candidate) => candidate[0] === message && candidate[1] === field);
  return columns ? Number(columns[2]) : undefined;
};

const hex = (bytes: Uint8Array): string =>
  Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");

// Reported as "declared by the schema, never written by the encoder".
const REPORTED_UNWRITTEN: ReadonlyArray<readonly [string, string]> = [
  ["Message.AudioMessage", "mediaKeyDomain"],
  ["Message.DocumentMessage", "mediaKeyDomain"],
  ["Message.ImageMessage", "mediaKeyDomain"],
  ["Message.MMSThumbnailMetadata", "mediaKeyDomain"],
  ["Message.StickerMessage", "mediaKeyDomain"],
  ["Message.VideoMessage", "mediaKeyDomain"],
  ["Message.MessageHistoryMetadata", "oldestMessageTimestamp"],
  ["Message.PaymentExtendedMetadata", "messageParamsJson"],
  ["SyncActionValue", "businessBroadcastAssociationAction"],
  ["SyncActionValue.AgentAction", "deviceID"],
  ["SyncActionValue.ChatAssignmentAction", "deviceAgentID"],
];

// Reported as "the codec round-trips under a name the schema does not declare".
const REPORTED_RENAMES: ReadonlyArray<readonly [string, string, string]> = [
  ["SyncActionValue.AgentAction", "deviceID", "deviceId"],
  ["SyncActionValue.ChatAssignmentAction", "deviceAgentID", "deviceAgentId"],
  ["Message.MessageHistoryMetadata", "oldestMessageTimestamp", "oldestMessageTimestampInWindow"],
];

beforeAll(() => {
  initWasmEngine();
});

describe("reported divergences against the consumed schema", () => {
  test("the schema declares none of the eleven fields reported as unwritten", () => {
    const declared = REPORTED_UNWRITTEN.filter(([message, field]) => declares(message, field));
    expect(declared).toEqual([]);
  });

  test("the reported renames are the spellings the schema itself declares", () => {
    for (const [message, reported, actual] of REPORTED_RENAMES) {
      expect([message, reported, declares(message, reported)]).toEqual([message, reported, false]);
      expect([message, actual, declares(message, actual)]).toEqual([message, actual, true]);
    }
  });

  test("mediaKeyDomain is declared on MediaDomainInfo alone", () => {
    const carriers = SURFACE.filter((columns) => columns[1] === "mediaKeyDomain").map((columns) => columns[0]);
    expect(carriers).toEqual(["MediaDomainInfo"]);
    expect(hex(encodeProto("MediaDomainInfo", { mediaKeyDomain: 0 }))).toBe("0800");
  });

  test("an explicit zero on a schema-declared field survives the round-trip", () => {
    // The eleven-field report called these renamed; they are the declared
    // names. What matters is that zero is not mistaken for unset.
    expect(hex(encodeProto("SyncActionValue.AgentAction", { deviceId: 0 }))).toBe("1000");
    expect(decodeProto("SyncActionValue.AgentAction", new Uint8Array([0x10, 0x00])).deviceId).toBe(0);
    expect(decodeProto("SyncActionValue.AgentAction", new Uint8Array()).deviceId).toBeUndefined();

    expect(hex(encodeProto("SyncActionValue.ChatAssignmentAction", { deviceAgentId: "" }))).toBe("0a00");
    expect(
      decodeProto("SyncActionValue.ChatAssignmentAction", new Uint8Array([0x0a, 0x00])).deviceAgentId,
    ).toBe("");

    expect(hex(encodeProto("Message.MessageHistoryMetadata", { oldestMessageTimestampInWindow: 0 }))).toBe("1000");
  });

  test("pollResultSnapshotMessageV3 is written at the number the schema declares", () => {
    expect(numberOf("Message", "pollResultSnapshotMessageV3")).toBe(115);
    // 115 << 3 | 2 = 922, varint 0x9a 0x07; then a zero-length submessage.
    // Field 114 would be 0x92 0x07.
    expect(hex(encodeProto("Message", { pollResultSnapshotMessageV3: {} }))).toBe("9a0700");
    expect(
      decodeProto("Message", new Uint8Array([0x9a, 0x07, 0x00])).pollResultSnapshotMessageV3,
    ).toBeDefined();
  });

  test("BotAvatarMetadata is absent from the schema, so BotMetadata drops it", () => {
    expect(SURFACE.some((columns) => columns[0].startsWith("BotAvatarMetadata"))).toBe(false);
    expect(declares("BotMetadata", "avatarMetadata")).toBe(false);
    // BotMetadata's declared numbers start at 2; 1 is the slot the reporter's
    // schema gives avatarMetadata. Pinned as a deliberate omission: the codec
    // cannot write a field its schema does not have.
    const numbers = SURFACE.filter((columns) => columns[0] === "BotMetadata").map((columns) => Number(columns[2]));
    expect(Math.min(...numbers)).toBe(2);
    expect(hex(encodeProto("MessageContextInfo", { botMetadata: { avatarMetadata: {} } }))).toBe("3a00");
  });
});
