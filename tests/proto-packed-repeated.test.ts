/**
 * Wire form of repeated fields.
 *
 * whatsapp.proto is `syntax = "proto2"`, where a repeated numeric or enum field
 * is unpacked unless it declares `[packed = true]`. These tests pin the emitted
 * bytes to that per-field declaration so a generator option or a schema bump
 * cannot silently move a field between the two forms.
 *
 * Every expected byte string here is written by hand from the field number and
 * wire type, never taken from the codec's own output.
 *
 * Run: bun test tests/proto-packed-repeated.test.ts
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { initWasmEngine, encodeProto, decodeProto } from "../dist/index.js";

beforeAll(() => {
  initWasmEngine();
});

const bytes = (...values: number[]) => new Uint8Array(values);

const expectBytes = (actual: Uint8Array, expected: Uint8Array) => {
  expect(Array.from(actual)).toEqual(Array.from(expected));
};

describe("repeated fields declared [packed = true]", () => {
  // ADVKeyIndexList.validIndexes = 4, uint32, [packed = true].
  // Packed tag = (4 << 3) | 2 = 0x22, then a byte length, then the varints.
  test("validIndexes encodes length-delimited", () => {
    expectBytes(
      encodeProto("ADVKeyIndexList", { validIndexes: [0, 1] }),
      bytes(0x22, 0x02, 0x00, 0x01),
    );
  });

  test("validIndexes decodes from the packed form", () => {
    const decoded = decodeProto("ADVKeyIndexList", bytes(0x22, 0x02, 0x00, 0x01)) as {
      validIndexes: number[];
    };
    expect(decoded.validIndexes).toEqual([0, 1]);
  });

  // A conforming peer may send the other form regardless of the declaration.
  // Unpacked tag = (4 << 3) | 0 = 0x20, repeated once per element.
  test("validIndexes still decodes from the unpacked form", () => {
    const decoded = decodeProto("ADVKeyIndexList", bytes(0x20, 0x00, 0x20, 0x01)) as {
      validIndexes: number[];
    };
    expect(decoded.validIndexes).toEqual([0, 1]);
  });

  test("DeviceListMetadata index lists encode length-delimited", () => {
    // senderKeyIndexes = 3 -> 0x1a; recipientKeyIndexes = 10 -> 0x52.
    expectBytes(
      encodeProto("DeviceListMetadata", { senderKeyIndexes: [1, 2] }),
      bytes(0x1a, 0x02, 0x01, 0x02),
    );
    expectBytes(
      encodeProto("DeviceListMetadata", { recipientKeyIndexes: [1, 2] }),
      bytes(0x52, 0x02, 0x01, 0x02),
    );
  });
});

describe("repeated fields left at the proto2 default", () => {
  // BotModeSelectionMetadata.overrideMode = 2, uint32, no packed option.
  // Unpacked tag = (2 << 3) | 0 = 0x10, once per element.
  test("overrideMode encodes one tag per element", () => {
    expectBytes(
      encodeProto("BotModeSelectionMetadata", { overrideMode: [0, 1] }),
      bytes(0x10, 0x00, 0x10, 0x01),
    );
  });

  // BotModeSelectionMetadata.mode = 1, enum, no packed option.
  // An enum is packable, so this is the field a blanket "pack everything"
  // setting would move first. Unpacked tag = (1 << 3) | 0 = 0x08.
  test("a repeated enum encodes one tag per element", () => {
    expectBytes(
      encodeProto("BotModeSelectionMetadata", { mode: [0, 1] }),
      bytes(0x08, 0x00, 0x08, 0x01),
    );
  });

  test("overrideMode decodes from the unpacked form", () => {
    const decoded = decodeProto(
      "BotModeSelectionMetadata",
      bytes(0x10, 0x00, 0x10, 0x01),
    ) as { overrideMode: number[] };
    expect(decoded.overrideMode).toEqual([0, 1]);
  });

  // Packed tag = (2 << 3) | 2 = 0x12. A peer is free to pack a field the
  // schema leaves unpacked, and the reader must take it.
  test("overrideMode also decodes from the packed form", () => {
    const decoded = decodeProto(
      "BotModeSelectionMetadata",
      bytes(0x12, 0x02, 0x00, 0x01),
    ) as { overrideMode: number[] };
    expect(decoded.overrideMode).toEqual([0, 1]);
  });

  test("a repeated enum decodes from either form", () => {
    const fromUnpacked = decodeProto(
      "BotModeSelectionMetadata",
      bytes(0x08, 0x00, 0x08, 0x01),
    ) as { mode: number[] };
    const fromPacked = decodeProto(
      "BotModeSelectionMetadata",
      bytes(0x0a, 0x02, 0x00, 0x01),
    ) as { mode: number[] };

    expect(fromUnpacked.mode).toEqual([0, 1]);
    expect(fromPacked.mode).toEqual([0, 1]);
  });
});

describe("repeated string and bytes are never packed", () => {
  // ContextInfo.mentionedJid = 15, string. Tag = (15 << 3) | 2 = 0x7a, and
  // each element carries its own tag and length. protobuf has no packed
  // encoding for length-delimited types, so a broad generator setting must
  // not reach these.
  test("mentionedJid writes a tag and length per element", () => {
    expectBytes(
      encodeProto("ContextInfo", { mentionedJid: ["a", "bc"] }),
      bytes(0x7a, 0x01, 0x61, 0x7a, 0x02, 0x62, 0x63),
    );
  });

  // Message.PollVoteMessage.selectedOptions = 1, bytes. Tag = (1 << 3) | 2 = 0x0a.
  test("selectedOptions writes a tag and length per element", () => {
    expectBytes(
      encodeProto("Message.PollVoteMessage", {
        selectedOptions: [bytes(0xaa), bytes(0xbb, 0xcc)],
      }),
      bytes(0x0a, 0x01, 0xaa, 0x0a, 0x02, 0xbb, 0xcc),
    );
  });
});

describe("empty and single-element lists", () => {
  test("an empty list writes nothing, in either form", () => {
    expect(encodeProto("ADVKeyIndexList", { validIndexes: [] }).length).toBe(0);
    expect(encodeProto("BotModeSelectionMetadata", { overrideMode: [] }).length).toBe(0);
  });

  test("a single element keeps the field's declared form", () => {
    expectBytes(encodeProto("ADVKeyIndexList", { validIndexes: [7] }), bytes(0x22, 0x01, 0x07));
    expectBytes(
      encodeProto("BotModeSelectionMetadata", { overrideMode: [7] }),
      bytes(0x10, 0x07),
    );
  });

  test("an empty packed field decodes to an empty list, not to absent", () => {
    const decoded = decodeProto("ADVKeyIndexList", bytes(0x22, 0x00)) as {
      validIndexes: number[];
    };
    expect(decoded.validIndexes).toEqual([]);
  });
});

// The tests above cover the fields a change would most likely touch. This one
// covers the rest: it fails if any *other* field starts or stops packing,
// which is what a generator-wide setting or a schema bump would do.
describe("the generated codec packs exactly the fields the schema declares", () => {
  const PACKED_BY_SCHEMA = [
    "validIndexes", // ADVKeyIndexList
    "senderKeyIndexes", // DeviceListMetadata
    "recipientKeyIndexes", // DeviceListMetadata
    "deviceIndexes", // Message.AppStateSyncKeyFingerprint
  ];

  test("no field packs beyond the four declared [packed = true]", () => {
    const source = readFileSync(
      new URL("../ts/generated/whatsapp.ts", import.meta.url),
      "utf8",
    );
    const lines = source.split("\n");
    const packed: string[] = [];

    for (let i = 1; i < lines.length; i++) {
      if (!/^\s*writer\.uint32\(\d+\)\.fork\(\);$/.test(lines[i]!)) continue;
      const owner = /message\.([A-Za-z0-9_]+) !== undefined/.exec(lines[i - 1]!);
      packed.push(owner ? owner[1]! : `unattributed line ${i + 1}`);
    }

    expect([...new Set(packed)].sort()).toEqual([...PACKED_BY_SCHEMA].sort());
  });
});
