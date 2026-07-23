import { describe, expect, test } from "bun:test";
import { BinaryWriter as BufBinaryWriter } from "@bufbuild/protobuf/wire";
import { WebMessageInfo } from "../ts/generated/whatsapp";
import { decodeProtoBatch, encodeProto } from "../ts/proto";
import { BinaryReader } from "../ts/proto-reader";

type Int64WriteMethod = "uint64" | "int64" | "sint64" | "fixed64" | "sfixed64";

const decode = (method: Int64WriteMethod, value: bigint): number => {
  const writer = new BufBinaryWriter();
  writer[method](value);
  const reader = new BinaryReader(writer.finish());
  return reader[`${method}Number`]();
};

describe("safe-number protobuf reader", () => {
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
    ["uint64", BigInt(Number.MAX_SAFE_INTEGER) + 1n],
    ["int64", BigInt(Number.MAX_SAFE_INTEGER) + 1n],
    ["int64", BigInt(Number.MIN_SAFE_INTEGER) - 1n],
    ["sint64", BigInt(Number.MAX_SAFE_INTEGER) + 1n],
    ["sint64", BigInt(Number.MIN_SAFE_INTEGER) - 1n],
    ["fixed64", BigInt(Number.MAX_SAFE_INTEGER) + 1n],
    ["sfixed64", BigInt(Number.MAX_SAFE_INTEGER) + 1n],
    ["sfixed64", BigInt(Number.MIN_SAFE_INTEGER) - 1n],
  ] as const)("rejects an unsafe %s value", (method, value) => {
    expect(() => decode(method, value)).toThrow(/Number\.(?:MAX|MIN)_SAFE_INTEGER/);
  });

  test("feeds safe numbers directly into generated codecs", () => {
    const writer = new BufBinaryWriter().uint32(24).uint64(BigInt(Number.MAX_SAFE_INTEGER));
    expect(WebMessageInfo.decode(writer.finish()).messageTimestamp).toBe(Number.MAX_SAFE_INTEGER);
  });

  test("keeps generated codecs strict at the safe-number boundary", () => {
    const writer = new BufBinaryWriter().uint32(24).uint64(BigInt(Number.MAX_SAFE_INTEGER) + 1n);
    expect(() => WebMessageInfo.decode(writer.finish())).toThrow("Number.MAX_SAFE_INTEGER");
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
