import { describe, expect, test } from "bun:test";
import {
  decodeReceiptWireBatch,
  decodeServerAckWireBatch,
  encodeReceiptWireBatch,
  encodeServerAckWireBatch,
  type PackedWireBatch,
  type ReceiptWireData,
  type WireJid,
} from "../ts/wire-info";

/**
 * The packed receipt/ack transport carries every string of a batch in one
 * region, and each value's length in the record that references it. That length
 * is a UTF-8 byte count on the Rust side (`FlatBatchWriter::cache_str` and
 * `write_inline` both write `value.len()`), so a value carrying any character
 * outside ASCII is where the host's reading of the region has to agree.
 *
 * Every value asserted here comes from the wire: `ReceiptType::Other` is the raw
 * `type` attribute, and message and ack ids are the ones the peer chose.
 */

/** Mirrors the decoder's short-region threshold in `ts/wire-info.ts`. */
const TINY_STRING_LIMIT = 100;

const pn: WireJid = { user: "5511999", server: "s.whatsapp.net", agent: 0, device: 0, integrator: 0 };
const group: WireJid = { user: "120363", server: "g.us", agent: 0, device: 0, integrator: 0 };

/** Bytes of a packed batch's string region: `u32 str_len` at header offset 12. */
function regionBytes(batch: PackedWireBatch): number {
  return new DataView(batch.buffer, batch.byteOffset, batch.byteLength).getUint32(12, true);
}

/**
 * Two receipts: the first defines a non-ASCII cached string (`leído`) and two
 * inline ids, the second is read entirely out of the region behind them. `pad`
 * only grows the region, so the same assertions cover both size regimes.
 */
function receiptsWithPadding(pad: string): readonly ReceiptWireData[] {
  return [
    {
      source: { chat: pn, sender: pn, is_from_me: false, is_group: false },
      message_ids: ["Ação-1", "M2"],
      timestamp: 1700000001,
      type: { Other: "leído" },
      offline: false,
    },
    {
      source: { chat: group, sender: group, is_from_me: false, is_group: true },
      message_ids: [`M3${pad}`],
      timestamp: 1700000002,
      type: "Delivered",
      offline: false,
    },
  ];
}

function expectReceiptsSurviveTheRegion(pad: string): void {
  const decoded = decodeReceiptWireBatch(encodeReceiptWireBatch(receiptsWithPadding(pad)));

  // The non-ASCII values themselves.
  expect(decoded[0]!.type).toEqual({ Other: "leído" });
  expect(decoded[0]!.message_ids).toEqual(["Ação-1", "M2"]);

  // And everything read out of the region after them, which a length counted in
  // the wrong unit would shift.
  expect(decoded[1]!.source.chat).toEqual(group);
  expect(decoded[1]!.source.sender).toEqual(group);
  expect(decoded[1]!.type).toBe("Delivered");
  expect(decoded[1]!.message_ids).toEqual([`M3${pad}`]);
}

describe("packed batch string region", () => {
  test("a short region keeps non-ASCII receipts intact", () => {
    expect(regionBytes(encodeReceiptWireBatch(receiptsWithPadding("")))).toBeLessThan(
      TINY_STRING_LIMIT,
    );
    expectReceiptsSurviveTheRegion("");
  });

  test("a long region keeps non-ASCII receipts intact", () => {
    const pad = "P".repeat(60);
    expect(regionBytes(encodeReceiptWireBatch(receiptsWithPadding(pad)))).toBeGreaterThanOrEqual(
      TINY_STRING_LIMIT,
    );
    expectReceiptsSurviveTheRegion(pad);
  });

  test("a non-ASCII ack id neither corrupts itself nor the ids behind it", () => {
    for (const pad of ["", "P".repeat(100)]) {
      const acks = [
        { id: "Ação-A", class: "message", timestamp: 1700000003 },
        { id: `B2${pad}`, class: "receipt", timestamp: 1700000004 },
      ];
      const batch = encodeServerAckWireBatch(acks);
      expect(regionBytes(batch) < TINY_STRING_LIMIT).toBe(pad === "");
      const decoded = decodeServerAckWireBatch(batch);
      expect(decoded.map(ack => ack.id)).toEqual(["Ação-A", `B2${pad}`]);
      expect(decoded.map(ack => ack.class)).toEqual(["message", "receipt"]);
    }
  });

  test("ids of every UTF-8 width leave the region aligned", () => {
    // One value per encoded width, including the surrogate pair, then a plain
    // tail: a width read as the wrong number of units shifts everything after.
    const ids = ["a", "é", "€", "😀", "TAIL"];
    for (const pad of ["", "P".repeat(100)]) {
      const acks = [...ids, `LAST${pad}`].map(id => ({ id, class: "message" }));
      const batch = encodeServerAckWireBatch(acks);
      expect(regionBytes(batch) < TINY_STRING_LIMIT).toBe(pad === "");
      expect(decodeServerAckWireBatch(batch).map(ack => ack.id)).toEqual([
        ...ids,
        `LAST${pad}`,
      ]);
    }
  });

  test("an inline length on the wire is a UTF-8 byte count", () => {
    const batch = encodeServerAckWireBatch([{ id: "Ação-A" }]);
    const view = new DataView(batch.buffer, batch.byteOffset, batch.byteLength);
    const recordsAt = 20 + view.getUint32(4, true) * 4 + view.getUint32(8, true) * 11;
    // Record: u16 class | u16 from | u16 error | f64 timestamp | u16 id length.
    expect(view.getUint16(recordsAt + 14, true)).toBe(new TextEncoder().encode("Ação-A").length);
    expect(view.getUint16(recordsAt + 14, true)).not.toBe("Ação-A".length);
  });
});
