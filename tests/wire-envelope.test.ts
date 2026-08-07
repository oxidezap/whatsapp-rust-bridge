import { describe, expect, test } from "bun:test";
import {
  decodeEventWireEnvelope,
  decodeMessageWireBatch,
  decodeReceiptWireBatch,
  decodeServerAckWireBatch,
  encodeEventWireEnvelope,
  encodeMessageWireBatch,
  encodeReceiptWireBatch,
  encodeServerAckWireBatch,
  EVENT_SEGMENT_KIND_MESSAGE,
  EVENT_SEGMENT_KIND_RECEIPT,
  EVENT_SEGMENT_KIND_SERVER_ACK,
  type EventWireSegment,
} from "../ts/wire-info";

const jid = { user: "5511999", server: "s.whatsapp.net", agent: 0, device: 0, integrator: 0 };

const messageBatch = encodeMessageWireBatch([
  {
    payload: new Uint8Array([10, 2, 104, 105]),
    info: {
      chat: "5511999@s.whatsapp.net",
      sender: "5511999@s.whatsapp.net",
      isFromMe: false,
      isGroup: false,
      id: "M1",
      timestamp: 1700000000,
      pushName: "Peer",
      isViewOnce: false,
      isOffline: false,
    },
  },
]);

const receiptBatch = encodeReceiptWireBatch([
  {
    source: { chat: jid, sender: jid, is_from_me: false, is_group: false },
    message_ids: ["M1"],
    timestamp: 1700000001,
    type: "Delivered",
    offline: false,
  },
]);

const ackBatch = encodeServerAckWireBatch([
  { id: "M1", class: "message", timestamp: 1700000002 },
]);

const turn: EventWireSegment[] = [
  { kind: EVENT_SEGMENT_KIND_MESSAGE, batch: messageBatch },
  { kind: EVENT_SEGMENT_KIND_RECEIPT, batch: receiptBatch },
  { kind: EVENT_SEGMENT_KIND_SERVER_ACK, batch: ackBatch },
];

describe("event wire envelope", () => {
  test("segments survive the round trip byte for byte, in order", () => {
    const segments = decodeEventWireEnvelope(encodeEventWireEnvelope(turn));
    expect(segments.map(s => s.kind)).toEqual(turn.map(s => s.kind));
    for (let i = 0; i < turn.length; i++) {
      expect(Array.from(segments[i]!.batch)).toEqual(Array.from(turn[i]!.batch));
    }
  });

  test("each segment decodes with the codec its kind names", () => {
    const [message, receipt, ack] = decodeEventWireEnvelope(encodeEventWireEnvelope(turn));
    expect(decodeMessageWireBatch(message!.batch).infos[0]!.id).toBe("M1");
    expect(decodeReceiptWireBatch(receipt!.batch)[0]!.message_ids).toEqual(["M1"]);
    expect(decodeServerAckWireBatch(ack!.batch)[0]!.id).toBe("M1");
  });

  test("message segments stay 8-aligned, so their records are read as a view", () => {
    const envelope = encodeEventWireEnvelope([
      // An odd-length segment ahead of the message forces the padding to matter.
      { kind: EVENT_SEGMENT_KIND_RECEIPT, batch: receiptBatch },
      { kind: EVENT_SEGMENT_KIND_MESSAGE, batch: messageBatch },
    ]);
    expect(receiptBatch.byteLength % 8).not.toBe(0);
    const [, message] = decodeEventWireEnvelope(envelope);
    expect(message!.batch.byteOffset % 8).toBe(0);
    expect(decodeMessageWireBatch(message!.batch).infos[0]!.pushName).toBe("Peer");
  });

  test("a truncated envelope is rejected rather than read past its end", () => {
    const envelope = encodeEventWireEnvelope(turn);
    expect(() => decodeEventWireEnvelope(envelope.subarray(0, 4))).toThrow(RangeError);
    expect(() => decodeEventWireEnvelope(envelope.slice(0, envelope.byteLength - 8))).toThrow(
      RangeError,
    );
  });
});
