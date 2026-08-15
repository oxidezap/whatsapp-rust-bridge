import { describe, expect, test } from "bun:test";
import {
  decodeProto,
  decodeProtoBatch,
  encodeProto,
  UnpairedSurrogateError,
  type ProtoDecodeReport,
} from "../ts/proto";
import { BinaryReader } from "../ts/proto-reader";

const CONVERSATION_TAG = 0x0a;
const hex = (bytes: Uint8Array): string => Buffer.from(bytes).toString("hex");

/** A `Message` carrying raw `conversation` bytes the writer would never produce. */
const conversationFrame = (bytes: readonly number[]): Uint8Array =>
  new Uint8Array([CONVERSATION_TAG, bytes.length, ...bytes]);

const conversationOf = (message: unknown): unknown =>
  (message as { conversation?: unknown }).conversation;

describe("decoding a string field whose bytes are not UTF-8", () => {
  test("substitutes U+FFFD instead of failing the message", () => {
    // 0xc3 opens a two-byte sequence that 0x28 does not continue.
    const decoded = decodeProto("Message", conversationFrame([0xc3, 0x28, 0x41]));

    expect(conversationOf(decoded)).toBe("�(A");
    expect(hex(new TextEncoder().encode(conversationOf(decoded) as string))).toBe("efbfbd2841");
  });

  /**
   * One U+FFFD per maximal subpart, not one per run: 0xff never opens a
   * sequence, so each of the four is ill-formed on its own and the 0x0f after
   * them is an ordinary character. A decoder that lets a lead byte swallow the
   * bytes after it reads the same field as a shorter string.
   */
  test("substitutes once per ill-formed byte, not once per run", () => {
    const decoded = conversationOf(
      decodeProto("Message", conversationFrame([0xff, 0xff, 0xff, 0xff, 0x0f])),
    ) as string;

    expect([...decoded].map((unit) => unit.codePointAt(0))).toEqual([
      0xfffd, 0xfffd, 0xfffd, 0xfffd, 0x0f,
    ]);
  });

  test("reports the substitution to a caller that asks for it", () => {
    const report: ProtoDecodeReport = { invalidUtf8Fields: 0 };
    decodeProto("Message", conversationFrame([0xc3, 0x28, 0x41]), report);

    expect(report.invalidUtf8Fields).toBe(1);
  });

  test("does not report a U+FFFD the peer genuinely sent", () => {
    const report: ProtoDecodeReport = { invalidUtf8Fields: 0 };
    const decoded = decodeProto(
      "Message",
      conversationFrame([0xef, 0xbf, 0xbd, 0x41]),
      report,
    );

    expect(conversationOf(decoded)).toBe("�A");
    expect(report.invalidUtf8Fields).toBe(0);
  });

  test("leaves the count at zero for text that needed nothing", () => {
    const report: ProtoDecodeReport = { invalidUtf8Fields: 0 };
    decodeProto("Message", encodeProto("Message", { conversation: "olá 👋" }), report);

    expect(report.invalidUtf8Fields).toBe(0);
  });

  test("still offers Buf's throwing decoder, which the generated codecs never ask for", () => {
    const field = new Uint8Array([2, 0xc3, 0x28]);

    expect(new BinaryReader(field).string()).toBe("�(");
    expect(() => new BinaryReader(field).string(true)).toThrow();
  });
});

describe("a reader shared across messages", () => {
  const malformed = conversationFrame([0xc3, 0x28]);
  const wellFormed = encodeProto("Message", { conversation: "olá 👋" });

  const batchOf = (entries: readonly Uint8Array[], report?: ProtoDecodeReport): unknown[] => {
    const total = entries.reduce((sum, entry) => sum + entry.length, 0);
    const data = new Uint8Array(total);
    const offsets = new Uint32Array(entries.length + 1);
    for (let index = 0; index < entries.length; index++) {
      data.set(entries[index]!, offsets[index]!);
      offsets[index + 1] = offsets[index]! + entries[index]!.length;
    }
    return decodeProtoBatch("Message", data, offsets, report);
  };

  test("does not carry a substitution into the next message", () => {
    const report: ProtoDecodeReport = { invalidUtf8Fields: 0 };
    const decoded = batchOf([malformed, wellFormed], report);

    expect(conversationOf(decoded[0])).toBe("�(");
    expect(conversationOf(decoded[1])).toBe("olá 👋");
    expect(report.invalidUtf8Fields).toBe(1);
  });

  test("does not carry it backwards either", () => {
    const decoded = batchOf([wellFormed, malformed]);

    expect(conversationOf(decoded[0])).toBe("olá 👋");
    expect(conversationOf(decoded[1])).toBe("�(");
  });
});

describe("encoding a string the caller cannot represent in UTF-8", () => {
  test("refuses an unpaired low surrogate instead of rewriting it", () => {
    expect(() => encodeProto("Message", { conversation: "a\udfffb" })).toThrow(
      UnpairedSurrogateError,
    );
  });

  test("names the code unit, the index and the field", () => {
    let thrown: UnpairedSurrogateError | undefined;
    try {
      encodeProto("Message", { extendedTextMessage: { text: "a\ud800b" } });
    } catch (error) {
      thrown = error as UnpairedSurrogateError;
    }

    expect(thrown).toBeInstanceOf(UnpairedSurrogateError);
    expect(thrown!.index).toBe(1);
    expect(thrown!.codeUnit).toBe(0xd800);
    expect(thrown!.path).toBe("extendedTextMessage.text");
    expect(thrown!.message).toContain("U+D800");
  });

  test("names the field the writer rejected, not an earlier key the codec ignores", () => {
    // `bogus` is not a Message field, so the writer never sees it — but it is
    // first in insertion order, which is the order this lookup walks.
    let thrown: UnpairedSurrogateError | undefined;
    try {
      encodeProto("Message", { bogus: "\ud800", conversation: "\udfff" });
    } catch (error) {
      thrown = error as UnpairedSurrogateError;
    }

    expect(thrown!.codeUnit).toBe(0xdfff);
    expect(thrown!.path).toBe("conversation");
  });

  test("still names the field when an ignored key carries the identical surrogate", () => {
    // Same code unit at the same index, so index/code-unit matching cannot
    // separate them — only the codec's own view of its fields can.
    let thrown: UnpairedSurrogateError | undefined;
    try {
      encodeProto("Message", { bogus: "\udfff", conversation: "\udfff" });
    } catch (error) {
      thrown = error as UnpairedSurrogateError;
    }

    expect(thrown!.path).toBe("conversation");
  });

  test("names the map field when the unpaired surrogate is in a map key", () => {
    let thrown: UnpairedSurrogateError | undefined;
    try {
      encodeProto("SyncActionValue", {
        musicUserIdAction: { musicUserIdMap: { "\ud800": "an-id" } },
      });
    } catch (error) {
      thrown = error as UnpairedSurrogateError;
    }

    expect(thrown).toBeInstanceOf(UnpairedSurrogateError);
    expect(thrown!.path).toBe("musicUserIdAction.musicUserIdMap (map key)");
  });

  test("still names the map when several of its keys are malformed the same way", () => {
    // Two candidates, one path — nothing about the field is ambiguous.
    let thrown: UnpairedSurrogateError | undefined;
    try {
      encodeProto("SyncActionValue", {
        musicUserIdAction: { musicUserIdMap: { "\ud800a": "x", "\ud800b": "y" } },
      });
    } catch (error) {
      thrown = error as UnpairedSurrogateError;
    }

    expect(thrown!.path).toBe("musicUserIdAction.musicUserIdMap (map key)");
  });

  test("names no field when two of them could equally be the one that failed", () => {
    let thrown: UnpairedSurrogateError | undefined;
    try {
      encodeProto("Message", {
        conversation: "\udfff",
        extendedTextMessage: { text: "\udfff" },
      });
    } catch (error) {
      thrown = error as UnpairedSurrogateError;
    }

    expect(thrown).toBeInstanceOf(UnpairedSurrogateError);
    expect(thrown!.path).toBeUndefined();
    expect(thrown!.message).toBe("unpaired surrogate U+DFFF at index 0 in a protobuf string field");
  });

  test.each([
    ["lone high surrogate", "\ud800"],
    ["lone low surrogate", "\udfff"],
    ["high surrogate followed by plain text", "\ud83dnot-a-pair"],
    ["low surrogate leading a valid pair", "\udc00😀"],
  ] as const)("refuses %s", (_name, value) => {
    expect(() => encodeProto("Message", { conversation: value })).toThrow(
      UnpairedSurrogateError,
    );
  });
});

describe("text the caller can represent", () => {
  test.each([
    ["ascii", "hello", "0a0568656c6c6f"],
    ["accents", "olá, tudo bem?", "0a0f6f6cc3a12c207475646f2062656d3f"],
    ["cjk", "日本語", "0a09e697a5e69cace8aa9e"],
    ["astral pair", "😀", "0a04f09f9880"],
    ["replacement character", "�", "0a03efbfbd"],
  ] as const)("round-trips %s unchanged", (_name, value, expected) => {
    const encoded = encodeProto("Message", { conversation: value });

    expect(hex(encoded)).toBe(expected);
    expect(conversationOf(decodeProto("Message", encoded))).toBe(value);
  });

  test("round-trips an astral character adjacent to text", () => {
    const value = "a😀b 🎉 日本語 olá";
    const encoded = encodeProto("Message", { conversation: value });

    expect(conversationOf(decodeProto("Message", encoded))).toBe(value);
  });
});
