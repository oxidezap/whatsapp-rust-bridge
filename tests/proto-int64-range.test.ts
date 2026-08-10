/**
 * A 64-bit field outside ±(2^53−1) must not take the whole message down with
 * it. The server sends microsecond timestamps, media sizes and 64-bit ids that
 * do not fit a double; decoding one of those has to keep the rest of the
 * message and hand the value back in a shape `encodeProto` accepts again.
 *
 * Pure codec: no wasm, no client, no network. Run: bun test tests/proto-int64-range.test.ts
 */
import { describe, expect, test } from "bun:test";
import { decodeProto, encodeProto } from "../ts/proto";

const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);
const MIN_SAFE = BigInt(Number.MIN_SAFE_INTEGER);
const U64_MAX = 2n ** 64n - 1n;
const I64_MIN = -(2n ** 63n);

/**
 * The `Long` split `camel_serializer.rs` emits: `low` unsigned, `high` as the
 * signed 32-bit field protobufjs uses, `value = high * 2^32 + low`.
 */
const longOf = (value: bigint, unsigned: boolean) => {
  const bits = value < 0n ? (1n << 64n) + value : value;
  return {
    low: Number(bits & 0xffffffffn),
    high: Number(BigInt.asIntN(32, bits >> 32n)),
    unsigned,
  };
};

/** `fileLength` is uint64, `mediaKeyTimestamp` is int64 — one message, both signednesses. */
const videoMessage = (fileLength: unknown, mediaKeyTimestamp: unknown) => ({
  videoMessage: {
    url: "https://mmg.whatsapp.net/d/f/x",
    mimetype: "video/mp4",
    seconds: 12,
    mediaKey: new Uint8Array(32).fill(7),
    caption: "look at this",
    fileLength,
    mediaKeyTimestamp,
  },
});

const roundTrip = (fileLength: unknown, mediaKeyTimestamp: unknown): any =>
  decodeProto("Message", encodeProto("Message", videoMessage(fileLength, mediaKeyTimestamp)));

describe("64-bit fields outside the safe-integer range", () => {
  test("decodes an unsafe uint64 instead of failing the message", () => {
    const value = MAX_SAFE + 2n;
    expect(roundTrip(value, 1700000123).videoMessage.fileLength).toEqual(longOf(value, true));
  });

  test("decodes an unsafe negative int64 instead of failing the message", () => {
    const value = MIN_SAFE - 2n;
    expect(roundTrip(1024, value).videoMessage.mediaKeyTimestamp).toEqual(longOf(value, false));
  });

  test("carries the rest of the message across untouched", () => {
    const decoded = roundTrip(U64_MAX, I64_MIN).videoMessage;
    expect(decoded.url).toBe("https://mmg.whatsapp.net/d/f/x");
    expect(decoded.mimetype).toBe("video/mp4");
    expect(decoded.caption).toBe("look at this");
    expect(decoded.seconds).toBe(12);
    expect(Array.from(decoded.mediaKey as Uint8Array)).toEqual(new Array(32).fill(7));
    expect(decoded.fileLength).toEqual(longOf(U64_MAX, true));
    expect(decoded.mediaKeyTimestamp).toEqual(longOf(I64_MIN, false));
  });

  test.each([
    ["max", MAX_SAFE, Number.MAX_SAFE_INTEGER],
    ["max+1", MAX_SAFE + 1n, longOf(MAX_SAFE + 1n, true)],
  ] as const)("pins the uint64 boundary at %s", (_label, encoded, expected) => {
    expect(roundTrip(encoded, 1).videoMessage.fileLength).toEqual(expected);
  });

  test.each([
    ["max", MAX_SAFE, Number.MAX_SAFE_INTEGER],
    ["max+1", MAX_SAFE + 1n, longOf(MAX_SAFE + 1n, false)],
    ["min", MIN_SAFE, Number.MIN_SAFE_INTEGER],
    ["min-1", MIN_SAFE - 1n, longOf(MIN_SAFE - 1n, false)],
  ] as const)("pins the int64 boundary at %s", (_label, encoded, expected) => {
    expect(roundTrip(1, encoded).videoMessage.mediaKeyTimestamp).toEqual(expected);
  });

  test("re-encodes what it decoded to the same bytes", () => {
    const original = encodeProto("Message", videoMessage(U64_MAX, I64_MIN));
    const reencoded = encodeProto("Message", decodeProto("Message", original));
    expect(Buffer.from(reencoded).toString("hex")).toBe(Buffer.from(original).toString("hex"));
  });

  test("leaves an in-range value a plain number", () => {
    const decoded = roundTrip(1_048_576, 1_700_000_123).videoMessage;
    expect(decoded.fileLength).toBe(1_048_576);
    expect(decoded.mediaKeyTimestamp).toBe(1_700_000_123);
  });

  test("keeps a message carrying an unsafe field JSON-serializable", () => {
    const decoded = roundTrip(U64_MAX, I64_MIN);
    expect(JSON.parse(JSON.stringify(decoded)).videoMessage.fileLength).toEqual(
      longOf(U64_MAX, true),
    );
  });
});
