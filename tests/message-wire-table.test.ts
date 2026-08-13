import { describe, expect, test } from "bun:test";
import {
  decodeMessageWireBatch,
  encodeMessageWireBatch,
  MESSAGE_WIRE_INFO_RECORD_WIDTH,
  MessageWireBatchEncoder,
  type MessageWireEntry,
  type MessageWireInfo,
} from "../ts/wire-info";

/**
 * The message table outlives the batch, so `decodeMessageWireBatch` carries
 * state between calls. Every test here starts its own run with its own
 * `MessageWireBatchEncoder`, whose first batch clears the reader — which is
 * also the property the first test checks.
 */

const HEADER_SLOT_DEFINITIONS = 4;
const HEADER_SLOT_FLAGS = 20;
const PACKED_FLAG_RESET_CACHES = 1;

const payload = new Uint8Array([10, 2, 104, 105]);

function info(overrides: Partial<MessageWireInfo> & { id: string }): MessageWireInfo {
  return {
    chat: "5511999@s.whatsapp.net",
    sender: "5511999@s.whatsapp.net",
    isFromMe: false,
    isGroup: false,
    timestamp: 1700000000,
    pushName: "Peer",
    isViewOnce: false,
    isOffline: false,
    ...overrides,
  };
}

function entry(overrides: Partial<MessageWireInfo> & { id: string }): MessageWireEntry {
  return { payload, info: info(overrides) };
}

const header = (batch: Uint8Array, slot: number): number =>
  new DataView(batch.buffer, batch.byteOffset, batch.byteLength).getUint32(slot, true);

describe("message wire table", () => {
  test("a repeated address is defined once and referenced afterwards", () => {
    const encoder = new MessageWireBatchEncoder();
    const first = encoder.encode([entry({ id: "M1" })]);
    const second = encoder.encode([entry({ id: "M2" })]);

    // chat and sender share a canonical form here, so: address, push name.
    expect(header(first, HEADER_SLOT_DEFINITIONS)).toBe(2);
    expect(header(second, HEADER_SLOT_DEFINITIONS)).toBe(0);
    expect(second.byteLength).toBeLessThan(first.byteLength);

    expect(decodeMessageWireBatch(first).infos[0]).toMatchObject({
      chat: "5511999@s.whatsapp.net",
      pushName: "Peer",
      id: "M1",
    });
    // The second batch carries no strings but the same values still come back.
    expect(decodeMessageWireBatch(second).infos[0]).toMatchObject({
      chat: "5511999@s.whatsapp.net",
      pushName: "Peer",
      id: "M2",
    });
  });

  test("a batch that clears the table is the batch that rebuilds it", () => {
    const run = new MessageWireBatchEncoder();
    run.encode([entry({ id: "M1" })]);
    const continuation = run.encode([entry({ id: "M2" })]);
    expect(header(continuation, HEADER_SLOT_FLAGS) & PACKED_FLAG_RESET_CACHES).toBe(0);

    // A different writer's first batch: it claims the reader's table, so its
    // own definitions land at index 0 whatever the previous run left there.
    const other = new MessageWireBatchEncoder();
    const claim = other.encode([
      entry({ id: "M3", chat: "120363@g.us", sender: "5511@s.whatsapp.net", pushName: "Alice" }),
    ]);
    expect(header(claim, HEADER_SLOT_FLAGS) & PACKED_FLAG_RESET_CACHES).toBe(
      PACKED_FLAG_RESET_CACHES,
    );

    decodeMessageWireBatch(run.encode([entry({ id: "M-warm" })]));
    expect(decodeMessageWireBatch(claim).infos[0]).toMatchObject({
      chat: "120363@g.us",
      sender: "5511@s.whatsapp.net",
      pushName: "Alice",
      id: "M3",
    });
  });

  /**
   * The failure the reset flag exists to prevent, from the reader's side: a
   * table left standing across a writer that restarted would answer with the
   * previous run's strings instead of failing, so the flag has to clear it.
   */
  test("a stale table is cleared rather than indexed into", () => {
    const staleBatch = new MessageWireBatchEncoder().encode([
      entry({ id: "OLD", chat: "old@g.us", pushName: "Old" }),
    ]);
    const freshBatch = new MessageWireBatchEncoder().encode([
      entry({ id: "NEW", chat: "new@g.us", pushName: "New" }),
    ]);
    expect(header(freshBatch, HEADER_SLOT_FLAGS) & PACKED_FLAG_RESET_CACHES).toBe(
      PACKED_FLAG_RESET_CACHES,
    );

    // Without the flag the reader appends the new definitions behind the stale
    // ones, and every record reads the previous run's value at its index.
    const forged = new Uint8Array(freshBatch);
    new DataView(forged.buffer).setUint32(HEADER_SLOT_FLAGS, 0, true);
    decodeMessageWireBatch(new Uint8Array(staleBatch));
    const corrupted = decodeMessageWireBatch(forged).infos[0]!;
    expect(corrupted.chat).toBe("old@g.us");
    expect(corrupted.pushName).toBe("Old");

    // With it, the same bytes against the same stale table read correctly.
    decodeMessageWireBatch(new Uint8Array(staleBatch));
    const decoded = decodeMessageWireBatch(freshBatch).infos[0]!;
    expect(decoded.chat).toBe("new@g.us");
    expect(decoded.pushName).toBe("New");
    expect(decoded.id).toBe("NEW");
  });

  test("inline values are read in record order, not from the table", () => {
    const encoder = new MessageWireBatchEncoder();
    const batch = encoder.encode([
      entry({ id: "M1", unavailableRequestId: "PDO-1" }),
      entry({ id: "M2" }),
      entry({ id: "M3", unavailableRequestId: "PDO-3", edit: "1" }),
    ]);
    const infos = decodeMessageWireBatch(batch).infos;
    expect(infos.map(i => i.id)).toEqual(["M1", "M2", "M3"]);
    expect(infos.map(i => i.unavailableRequestId)).toEqual(["PDO-1", undefined, "PDO-3"]);
    expect(infos.map(i => i.edit)).toEqual([undefined, undefined, "1"]);
  });

  test("an empty id and an empty request id are not absent", () => {
    const batch = encodeMessageWireBatch([entry({ id: "", unavailableRequestId: "" })]);
    const decoded = decodeMessageWireBatch(batch).infos[0]!;
    expect(decoded.id).toBe("");
    expect(decoded.unavailableRequestId).toBe("");
  });

  /** Push names are the peer's own text, so both regions have to be UTF-8. */
  test("non-ASCII survives in the table and inline alike", () => {
    const encoder = new MessageWireBatchEncoder();
    const first = encoder.encode([entry({ id: "não-1", pushName: "José 🇧🇷" })]);
    const second = encoder.encode([entry({ id: "não-2", pushName: "José 🇧🇷" })]);
    expect(header(second, HEADER_SLOT_DEFINITIONS)).toBe(0);
    expect(decodeMessageWireBatch(first).infos[0]).toMatchObject({
      id: "não-1",
      pushName: "José 🇧🇷",
    });
    expect(decodeMessageWireBatch(second).infos[0]).toMatchObject({
      id: "não-2",
      pushName: "José 🇧🇷",
    });
  });

  /**
   * The two writers are hand-mirrored, so one fixture is pinned byte for byte
   * on both sides. The same literal is asserted against `write_flat` in
   * `message_wire_batch_matches_the_host_encoder_byte_for_byte`
   * (`src/wire_batch.rs`); a change to either writer that the other does not
   * follow fails here or there rather than in a host's decode.
   */
  test("the encoder matches the Rust writer byte for byte", () => {
    const batch = encodeMessageWireBatch([
      {
        payload: new Uint8Array(0),
        info: {
          chat: "120363@g.us",
          sender: "5511@s.whatsapp.net",
          isFromMe: false,
          isGroup: false,
          id: "WIRE-1",
          timestamp: 0,
          pushName: "Alice",
          isViewOnce: false,
          isOffline: false,
        },
      },
    ]);
    const hex = Array.from(batch, byte => byte.toString(16).padStart(2, "0")).join("");
    expect(hex).toBe(
      // header: 1 message, 3 definitions, 0 payload bytes, 41 string bytes,
      // record width 10, flags = reset.
      "010000000300000000000000290000000a00000001000000" +
        // record: chat 0, sender 1, no alternates, id 6 bytes inline,
        // pushName 2, timestamp 0, no flags, no request id, no edit.
        "0000000000000000" +
        "000000000000f03f" +
        "0000000000000000" +
        "0000000000000000" +
        "0000000000001840" +
        "0000000000000040" +
        "0000000000000000" +
        "0000000000000000" +
        "0000000000000000" +
        "0000000000000000" +
        // payload offsets, then definition offsets.
        "0000000000000000" +
        "000000000b0000001e00000023000000" +
        // the three definitions, then the inline id.
        "31323033363340672e7573" +
        "3535313140732e77686174736170702e6e6574" +
        "416c696365" +
        "574952452d31",
    );
  });

  /**
   * A rejected batch still installed its definitions, and they have to stay.
   * The writer counts a definition as held the moment it writes the batch out;
   * it never hears that this one failed to decode. Rolling the definitions
   * back on the reader's side is what would put the two out of step — the very
   * next batch references a slot the rejected batch defined.
   */
  test("a rejected batch keeps the definitions the writer counted", () => {
    const run = new MessageWireBatchEncoder();
    decodeMessageWireBatch(run.encode([entry({ id: "M1" })]));

    const poisoned = run.encode([
      entry({ id: "M2", chat: "second@g.us", sender: "second@g.us", pushName: "Second" }),
    ]);
    expect(header(poisoned, HEADER_SLOT_DEFINITIONS)).toBe(2);
    expect(header(poisoned, HEADER_SLOT_FLAGS) & PACKED_FLAG_RESET_CACHES).toBe(0);
    // Break it after its definitions: an inline length past the string region.
    // The definition loop itself cannot throw once the region check passes, so
    // this is the shape every mid-batch failure has.
    const records = new Float64Array(
      poisoned.buffer,
      poisoned.byteOffset + 24,
      MESSAGE_WIRE_INFO_RECORD_WIDTH,
    );
    records[4] = 4096;
    expect(() => decodeMessageWireBatch(poisoned)).toThrow(RangeError);

    // The next batch defines nothing and indexes what the rejected one added.
    const next = run.encode([
      entry({ id: "M3", chat: "second@g.us", sender: "second@g.us", pushName: "Second" }),
    ]);
    expect(header(next, HEADER_SLOT_DEFINITIONS)).toBe(0);
    expect(decodeMessageWireBatch(next).infos[0]).toMatchObject({
      chat: "second@g.us",
      pushName: "Second",
      id: "M3",
    });
  });

  /**
   * A push name is the peer's own free text and the only cached value with no
   * shape of its own. The table's byte ceiling bounds the aggregate, but the
   * host only hears about a roll on the *next* batch — so an oversized name
   * goes inline and is never in the table to begin with.
   */
  test("an oversized push name goes inline instead of into the table", () => {
    const oversized = "n".repeat(257);
    const encoder = new MessageWireBatchEncoder();
    const batch = encoder.encode([entry({ id: "M1", pushName: oversized })]);
    // The address only: chat and sender share a canonical form here.
    expect(header(batch, HEADER_SLOT_DEFINITIONS)).toBe(1);
    expect(decodeMessageWireBatch(batch).infos[0]).toMatchObject({
      id: "M1",
      pushName: oversized,
      chat: "5511999@s.whatsapp.net",
    });

    // A repeat is not deduplicated, because nothing remembered it.
    const again = encoder.encode([entry({ id: "M2", pushName: oversized })]);
    expect(header(again, HEADER_SLOT_DEFINITIONS)).toBe(0);
    expect(decodeMessageWireBatch(again).infos[0]!.pushName).toBe(oversized);

    // At the limit it is a table entry like any other, and dedups.
    const atLimit = "n".repeat(256);
    const first = encoder.encode([entry({ id: "M3", pushName: atLimit })]);
    expect(header(first, HEADER_SLOT_DEFINITIONS)).toBe(1);
    const second = encoder.encode([entry({ id: "M4", pushName: atLimit })]);
    expect(header(second, HEADER_SLOT_DEFINITIONS)).toBe(0);
    expect(decodeMessageWireBatch(first).infos[0]!.pushName).toBe(atLimit);
    expect(decodeMessageWireBatch(second).infos[0]!.pushName).toBe(atLimit);
  });

  test("an inline push name is read in order, between the id and the request id", () => {
    const oversized = "ç".repeat(200); // 400 UTF-8 bytes, over the limit
    const batch = encodeMessageWireBatch([
      entry({ id: "M1", pushName: oversized, unavailableRequestId: "PDO-1" }),
      entry({ id: "M2", pushName: "Peer", unavailableRequestId: "PDO-2" }),
    ]);
    const infos = decodeMessageWireBatch(batch).infos;
    expect(infos.map(i => i.id)).toEqual(["M1", "M2"]);
    expect(infos.map(i => i.pushName)).toEqual([oversized, "Peer"]);
    expect(infos.map(i => i.unavailableRequestId)).toEqual(["PDO-1", "PDO-2"]);
  });

  test("a record pointing past the table is rejected, not read as undefined", () => {
    const batch = encodeMessageWireBatch([entry({ id: "M1" })]);
    const records = new Float64Array(
      batch.buffer,
      batch.byteOffset + 24,
      MESSAGE_WIRE_INFO_RECORD_WIDTH,
    );
    records[0] = 99;
    expect(() => decodeMessageWireBatch(batch)).toThrow(RangeError);
  });

  test("an inline length past the string region is rejected", () => {
    const batch = encodeMessageWireBatch([entry({ id: "M1" })]);
    const records = new Float64Array(
      batch.buffer,
      batch.byteOffset + 24,
      MESSAGE_WIRE_INFO_RECORD_WIDTH,
    );
    // Slot 4 is the id's inline byte length.
    records[4] = 4096;
    expect(() => decodeMessageWireBatch(batch)).toThrow(RangeError);
  });
});
