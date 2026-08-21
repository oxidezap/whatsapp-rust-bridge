/**
 * A receipt's id count has to survive the record it is written into.
 *
 * The count was a `u8`, so 255 ids was the ceiling — and a read receipt
 * aggregates every message it acknowledges, so an active group clears past that
 * in one go. Reaching it was not a lost receipt, on either side of the bridge:
 *
 *  - the TS encoder wrote `builder.u8(300)`, which truncates to 44. The decoder
 *    then read 44 inline values out of 300 and walked into the next record's
 *    bytes, so every record after it decoded shifted — usually landing on
 *    `packed record references a missing jid slot`, sometimes on plausible
 *    garbage;
 *  - the Rust encoder caught the overflow, but only after writing the flags,
 *    eight slots and the timestamp into the shared `records` buffer. Returning
 *    `Err` there left those bytes in place with `record_count` un-incremented,
 *    which is the same shift, from the other direction. That is what
 *    `receipt wire serialization failed` looked like in production.
 *
 * The count is now the same 16-bit shape a slot uses, and the length is checked
 * before the first byte is written, so a rejected receipt costs only itself.
 */

import { describe, expect, test } from "bun:test";
import { decodeReceiptWireBatch, encodeReceiptWireBatch } from "../ts/wire-info.ts";
import type { ReceiptWireData } from "../ts/wire-info.ts";

const receipt = (chat: string, ids: number): ReceiptWireData => ({
  source: {
    chat: { user: chat, server: "g.us" },
    sender: { user: "5511999999999", server: "s.whatsapp.net" },
    is_from_me: false,
    is_group: true,
    addressing_mode: undefined,
    sender_alt: undefined,
    recipient_alt: undefined,
    broadcast_list_owner: undefined,
    recipient: undefined,
  },
  message_ids: Array.from({ length: ids }, (_, i) => `MSGID${i}`),
  timestamp: 1_700_000_000,
  type: "read",
  offline: false,
});

describe("receipt id count", () => {
  // 255 passed before this fix; 256 is where the u8 wrapped.
  for (const ids of [0, 1, 255, 256, 1000]) {
    test(`round-trips a receipt carrying ${ids} ids`, () => {
      const [back] = decodeReceiptWireBatch(encodeReceiptWireBatch([receipt("grupo", ids)]));

      expect(back?.message_ids.length).toBe(ids);
      expect(back?.message_ids[ids - 1]).toBe(ids > 0 ? `MSGID${ids - 1}` : undefined);
    });
  }

  // The failure that actually hurt: not the big receipt, but the records after
  // it in the same batch.
  test("a large receipt does not shift the records that follow it", () => {
    const batch = decodeReceiptWireBatch(
      encodeReceiptWireBatch([receipt("grupoA", 300), receipt("grupoB", 2), receipt("grupoC", 7)]),
    );

    expect(batch.length).toBe(3);
    expect(batch[0]?.source.chat.user).toBe("grupoA");
    expect(batch[1]?.source.chat.user).toBe("grupoB");
    expect(batch[1]?.message_ids.length).toBe(2);
    expect(batch[2]?.source.chat.user).toBe("grupoC");
    expect(batch[2]?.message_ids.length).toBe(7);
  });

  test("refuses a receipt past the 16-bit ceiling instead of truncating it", () => {
    expect(() => encodeReceiptWireBatch([receipt("grupo", 0x1_0000)])).toThrow(
      /more message ids than the wire format holds/,
    );
  });
});
