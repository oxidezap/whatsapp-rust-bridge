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
const PACKED_FLAG_CLEAR_AFTER = 1 << 1;

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
   * The definitions are cut with one cursor now, so an offset table that does
   * not ascend from zero moves every value behind it rather than spoiling one
   * definition. A batch carrying one is rejected, and rejected before the
   * standing table is touched: it is the one failure the reader can see before
   * installing anything, so the run it interrupts carries on intact.
   */
  test("a definition table that does not ascend is rejected before it is installed", () => {
    const encoder = new MessageWireBatchEncoder();
    const opening = encoder.encode([entry({ id: "M1" })]);
    expect(decodeMessageWireBatch(opening).infos[0]).toMatchObject({ pushName: "Peer", id: "M1" });

    const poisoned = encoder.encode([
      entry({ id: "M2", chat: "second@g.us", sender: "second@g.us", pushName: "Second" }),
    ]);
    expect(header(poisoned, HEADER_SLOT_DEFINITIONS)).toBe(2);
    // Header, then one record, then the payload offsets, then this table.
    const offsets = new Uint32Array(
      poisoned.buffer,
      poisoned.byteOffset + 24 + MESSAGE_WIRE_INFO_RECORD_WIDTH * 8 + 2 * 4,
      3,
    );
    // The second definition now ends a byte before it starts.
    offsets[2] = offsets[1]! - 1;
    expect(() => decodeMessageWireBatch(poisoned)).toThrow(RangeError);

    // The opening batch's entries are still where it put them.
    const next = encoder.encode([entry({ id: "M3" })]);
    expect(header(next, HEADER_SLOT_DEFINITIONS)).toBe(0);
    expect(decodeMessageWireBatch(next).infos[0]).toMatchObject({
      chat: "5511999@s.whatsapp.net",
      pushName: "Peer",
      id: "M3",
    });
  });

  /**
   * Definitions and inline values share one string region, and the reader cuts
   * it with a cursor rather than a view per value, so a value whose UTF-8
   * width differs from its UTF-16 width moves every value behind it if the cut
   * is wrong. This batch is long enough (over 100 bytes of region) to take the
   * decode-the-whole-region path, and mixes 2, 3 and 4 byte code points across
   * definitions and inline ids alike.
   */
  test("a long non-ASCII region cuts every value where it starts", () => {
    const names = ["José 🇧🇷", "Ünïcödé Ñame", "日本語の名前", "Ω→∞ señor"];
    const ids = ["não-1", "ідентифікатор", "🆔-3", "id-é-4"];
    const entries = names.map((pushName, index) =>
      entry({
        id: ids[index]!,
        pushName,
        chat: `55119${index}@s.whatsapp.net`,
        sender: `55119${index}@s.whatsapp.net`,
        unavailableRequestId: `réq-${index}`,
      }),
    );

    const batch = encodeMessageWireBatch(entries);
    const decoded = decodeMessageWireBatch(batch).infos;
    expect(decoded).toHaveLength(entries.length);
    decoded.forEach((decodedInfo, index) => {
      expect(decodedInfo).toMatchObject({
        id: ids[index]!,
        pushName: names[index]!,
        chat: `55119${index}@s.whatsapp.net`,
        unavailableRequestId: `réq-${index}`,
      });
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
   * A peer sizes its own push name, and the table is what stops one from being
   * written per message. The batch that blows the byte ceiling says so in its
   * own header, so the reader drops the table straight after reading it rather
   * than holding a megabyte until whenever the next batch arrives — on a stream
   * that then goes idle, that is the difference between bounded and not.
   */
  test("an oversized push name is written once and then dropped", () => {
    const oversized = "n".repeat(1024 * 1024);
    const encoder = new MessageWireBatchEncoder();
    const batch = encoder.encode(
      Array.from({ length: 32 }, (_, i) => entry({ id: `M${i}`, pushName: oversized })),
    );

    // The address and the name, once each, for all 32 messages.
    expect(header(batch, HEADER_SLOT_DEFINITIONS)).toBe(2);
    expect(batch.byteLength).toBeLessThan(oversized.length * 2);
    expect(header(batch, HEADER_SLOT_FLAGS) & PACKED_FLAG_CLEAR_AFTER).toBe(PACKED_FLAG_CLEAR_AFTER);

    const infos = decodeMessageWireBatch(batch).infos;
    expect(infos).toHaveLength(32);
    expect(infos.every(i => i.pushName === oversized)).toBe(true);

    // Dropped: the next batch has to define what it names all over again.
    const next = encoder.encode([entry({ id: "M-next" })]);
    expect(header(next, HEADER_SLOT_DEFINITIONS)).toBe(2);
    expect(decodeMessageWireBatch(next).infos[0]).toMatchObject({
      chat: "5511999@s.whatsapp.net",
      pushName: "Peer",
      id: "M-next",
    });
  });

  /**
   * The encoder mutates its table as it goes, so a throw part-way leaves it
   * holding entries whose definitions were never written. Reusing it must not
   * hand the reader indices for those.
   */
  test("an encoder that threw mid-batch gives up its table", () => {
    const encoder = new MessageWireBatchEncoder();
    decodeMessageWireBatch(encoder.encode([entry({ id: "M1" })]));

    const exploding = entry({ id: "M2", chat: "cached-before-the-throw@g.us" });
    Object.defineProperty(exploding.info, "sender", {
      get() {
        throw new Error("boom");
      },
    });
    expect(() => encoder.encode([exploding])).toThrow("boom");

    // The chat the throwing batch cached, in a batch that does get written.
    // Without the rollback the encoder still believes the reader holds it, so
    // it emits no definition and points at a slot the reader never filled.
    const after = encoder.encode([entry({ id: "M3", chat: "cached-before-the-throw@g.us" })]);
    expect(header(after, HEADER_SLOT_FLAGS) & PACKED_FLAG_RESET_CACHES).toBe(
      PACKED_FLAG_RESET_CACHES,
    );
    expect(decodeMessageWireBatch(after).infos[0]).toMatchObject({
      chat: "cached-before-the-throw@g.us",
      sender: "5511999@s.whatsapp.net",
      pushName: "Peer",
      id: "M3",
    });
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
