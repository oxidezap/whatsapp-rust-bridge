import { describe, expect, test } from "bun:test";
import { BinaryWriter as BufBinaryWriter } from "@bufbuild/protobuf/wire";
import { WebMessageInfo } from "../ts/generated/whatsapp";
import { decodeProtoBatch, encodeProto } from "../ts/proto";
import { BinaryReader, longToBigInt, type Int64 } from "../ts/proto-reader";

type Int64WriteMethod = "uint64" | "int64" | "sint64" | "fixed64" | "sfixed64";

const decode = (method: Int64WriteMethod, value: bigint): Int64 => {
  const writer = new BufBinaryWriter();
  writer[method](value);
  const reader = new BinaryReader(writer.finish());
  return reader[`${method}Value`]();
};

const longOf = (value: bigint, unsigned: boolean) => {
  const bits = value < 0n ? (1n << 64n) + value : value;
  return {
    low: Number(bits & 0xffffffffn),
    high: Number(BigInt.asIntN(32, bits >> 32n)),
    unsigned,
  };
};

describe("64-bit protobuf reader", () => {
  test.each([
    ["uint64", 0n],
    ["uint64", BigInt(Number.MAX_SAFE_INTEGER)],
    ["int64", BigInt(Number.MIN_SAFE_INTEGER)],
    ["int64", BigInt(Number.MAX_SAFE_INTEGER)],
    ["sint64", BigInt(Number.MIN_SAFE_INTEGER)],
    ["sint64", BigInt(Number.MAX_SAFE_INTEGER)],
    ["fixed64", BigInt(Number.MAX_SAFE_INTEGER)],
    ["sfixed64", BigInt(Number.MIN_SAFE_INTEGER)],
    ["sfixed64", BigInt(Number.MAX_SAFE_INTEGER)],
  ] as const)("decodes an exact safe %s value", (method, value) => {
    expect(decode(method, value)).toBe(Number(value));
  });

  test.each([
    ["uint64", BigInt(Number.MAX_SAFE_INTEGER) + 1n, true],
    ["uint64", 2n ** 64n - 1n, true],
    ["int64", BigInt(Number.MAX_SAFE_INTEGER) + 1n, false],
    ["int64", BigInt(Number.MIN_SAFE_INTEGER) - 1n, false],
    ["int64", -(2n ** 63n), false],
    ["sint64", BigInt(Number.MAX_SAFE_INTEGER) + 1n, false],
    ["sint64", BigInt(Number.MIN_SAFE_INTEGER) - 1n, false],
    ["fixed64", BigInt(Number.MAX_SAFE_INTEGER) + 1n, true],
    ["sfixed64", BigInt(Number.MAX_SAFE_INTEGER) + 1n, false],
    ["sfixed64", BigInt(Number.MIN_SAFE_INTEGER) - 1n, false],
  ] as const)("widens an unsafe %s value to a Long", (method, value, unsigned) => {
    expect(decode(method, value)).toEqual(longOf(value, unsigned));
  });

  test("feeds safe numbers directly into generated codecs", () => {
    const writer = new BufBinaryWriter().uint32(24).uint64(BigInt(Number.MAX_SAFE_INTEGER));
    expect(WebMessageInfo.decode(writer.finish()).messageTimestamp).toBe(Number.MAX_SAFE_INTEGER);
  });

  test("hands generated codecs a Long past the safe-number boundary", () => {
    const value = BigInt(Number.MAX_SAFE_INTEGER) + 1n;
    const writer = new BufBinaryWriter().uint32(24).uint64(value);
    expect(WebMessageInfo.decode(writer.finish()).messageTimestamp).toEqual(longOf(value, true));
  });

  /**
   * A varint ends at the first byte with the continuation bit clear, and where
   * it ends decides where the next tag is read. The safe-number fast path reads
   * at most eight bytes and rewinds for the rest, so the boundary it reports has
   * to be the same one the full two-word path reports — a byte of difference
   * here moves every field after it.
   */
  test.each([
    [[0x01], 1, 1n],
    [[0x81, 0x80, 0x80, 0x80, 0x80, 0x00], 6, 1n],
    [[0x81, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00], 10, 1n],
    [[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x0f], 8, 2n ** 53n - 1n],
    [[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x10], 8, 2n ** 53n],
    [[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 9, 2n ** 56n],
    [[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01], 10, 2n ** 64n - 1n],
  ] as const)("consumes %p as %i bytes", (encoded, consumed, value) => {
    const reader = new BinaryReader(new Uint8Array([...encoded, 0xaa]));
    const decoded = reader.uint64Value();
    expect(reader.pos).toBe(consumed);
    expect(typeof decoded === "number" ? BigInt(decoded) : longToBigInt(decoded)).toBe(value);
  });

  test("decodes UTF-8 directly from a reader with a non-zero byte offset", () => {
    const value = "olá 👋";
    const encoded = new BufBinaryWriter().string(value).finish();
    const framed = new Uint8Array(encoded.length + 2);
    framed.set(encoded, 1);

    const reader = new BinaryReader(framed.subarray(1, 1 + encoded.length));
    expect(reader.string()).toBe(value);
    expect(reader.pos).toBe(encoded.length);
  });

  test("retains replacement and strict-error semantics for malformed UTF-8", () => {
    const malformed = new Uint8Array([2, 0xc3, 0x28]);
    expect(new BinaryReader(malformed).string()).toBe("�(");
    expect(() => new BinaryReader(malformed).string(true)).toThrow();
  });

  test("decodes canonical and wide bool varints without changing semantics", () => {
    expect(new BinaryReader(new Uint8Array([0])).bool()).toBe(false);
    expect(new BinaryReader(new Uint8Array([1])).bool()).toBe(true);
    expect(new BinaryReader(new Uint8Array([0x80, 0])).bool()).toBe(false);
    expect(new BinaryReader(new Uint8Array([0x80, 1])).bool()).toBe(true);
    expect(() => new BinaryReader(new Uint8Array()).bool()).toThrow("premature EOF");
  });

  test("decodes a delimited protobuf batch with one shared reader", () => {
    const encoded = [
      encodeProto("Message", { conversation: "one" }),
      encodeProto("Message", { extendedTextMessage: { text: "two" } }),
      encodeProto("Message", {}),
    ];
    const data = new Uint8Array(encoded.reduce((total, entry) => total + entry.length, 0));
    const offsets = new Uint32Array(encoded.length + 1);
    for (let index = 0; index < encoded.length; index++) {
      data.set(encoded[index]!, offsets[index]);
      offsets[index + 1] = offsets[index]! + encoded[index]!.length;
    }

    expect(decodeProtoBatch("Message", data, offsets)).toEqual([
      { conversation: "one" },
      { extendedTextMessage: { text: "two" } },
      {},
    ]);
  });

  test.each([
    [new Uint32Array(), "leading sentinel"],
    [new Uint32Array([1, 1]), "start at zero"],
    [new Uint32Array([0, 2, 1]), "monotonic"],
    [new Uint32Array([0]), "end at the data length"],
  ] as const)("rejects invalid protobuf batch offsets", (offsets, message) => {
    expect(() => decodeProtoBatch("Message", new Uint8Array(1), offsets)).toThrow(message);
  });

});
