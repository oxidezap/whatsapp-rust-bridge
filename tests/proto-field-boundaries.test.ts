/**
 * Where a field ends, and how deep a message is allowed to go.
 *
 * A tag carries a field number and a wire type, and the wire type is half of
 * what says where the field's bytes stop. When the wire type is not the one the
 * schema declares for that number, the bytes are not that field: the parser has
 * to treat the whole thing as an unknown field and skip it by its wire type,
 * because reading it as the declared field would consume a different number of
 * bytes and move every boundary after it. That reading is pinned here, together
 * with the lengths and the nesting depths around it, because two parsers that
 * disagree about framed bytes is a parser differential, not a robustness detail.
 *
 * Every byte string below is written by hand from the field number and wire
 * type, never taken from the codec's own output.
 *
 * Run: bun test tests/proto-field-boundaries.test.ts
 */

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { decodeProto } from "../ts/proto";

const bytes = (...values: number[]) => new Uint8Array(values);

const hexBytes = (hex: string): Uint8Array => {
  const out = new Uint8Array(hex.length / 2);
  for (let index = 0; index < out.length; index++) {
    out[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return out;
};

const hexOf = (data: Uint8Array): string =>
  Array.from(data, (byte) => byte.toString(16).padStart(2, "0")).join("");

const varint = (value: number): number[] => {
  const out: number[] = [];
  let rest = value >>> 0;
  do {
    const byte = rest & 0x7f;
    rest = Math.floor(rest / 128);
    out.push(rest > 0 ? byte | 0x80 : byte);
  } while (rest > 0);
  return out;
};

/** The same number, spelled in `width` varint bytes instead of the fewest. */
const paddedVarint = (value: number, width: number): number[] => {
  const out = varint(value);
  while (out.length < width) {
    out[out.length - 1]! |= 0x80;
    out.push(0x00);
  }
  out[out.length - 1]! &= 0x7f;
  return out;
};

describe("a tag whose wire type the schema does not declare", () => {
  /**
   * Message.ImageMessage.firstScanLength = 19 is a uint32, so its tag is
   * 19 << 3 | 0 = 0x98 0x01. Sent as 19 << 3 | 2 = 0x9a 0x01 it is not that
   * field: the four bytes it frames are skipped whole, and experimentGroupId
   * (20 -> 0xa0 0x01) and staticUrl (29 -> 0xea 0x01) read normally after it.
   *
   * Reading the frame's first varint as the field's value instead consumes two
   * of its four bytes, and the two bytes left then swallow both of the fields
   * that follow.
   */
  test("a length-delimited frame on a uint32 field is skipped whole", () => {
    expect(
      decodeProto(
        "Message.ImageMessage",
        bytes(0x9a, 0x01, 0x04, 0xe9, 0x33, 0x68, 0x89, 0xa0, 0x01, 0x01, 0xea, 0x01, 0x00),
      ),
    ).toEqual({ experimentGroupId: 1, staticUrl: "" });
  });

  test("the wire type the schema does declare still reads", () => {
    expect(
      decodeProto(
        "Message.ImageMessage",
        bytes(0x98, 0x01, 0x1b, 0xa0, 0x01, 0x01, 0xea, 0x01, 0x00),
      ),
    ).toEqual({ firstScanLength: 27, experimentGroupId: 1, staticUrl: "" });
  });

  /**
   * HistorySync.syncType = 1 is an enum, so its tag is 1 << 3 | 0 = 0x08.
   * Sent as 1 << 3 | 2 = 0x0a it frames bytes rather than naming a value, and
   * an absent required field is what a skipped field looks like from here.
   */
  test("a length-delimited frame on an enum field leaves the field absent", () => {
    expect(decodeProto("HistorySync", bytes(0x0a, 0x00))).toEqual({});
    expect(decodeProto("HistorySync", bytes(0x0a, 0x02, 0x0a, 0x00))).toEqual({});
    expect(decodeProto("HistorySync", bytes(0x0a, 0x01, 0x07))).toEqual({});
  });

  test("the wire type the schema does declare still reads the zero", () => {
    expect(decodeProto("HistorySync", bytes(0x08, 0x00))).toEqual({ syncType: 0 });
  });

  /**
   * A repeated numeric field is the one place two wire types are both the
   * schema's: proto2 leaves ImageMessage.scanLengths = 22 unpacked, and a
   * parser still has to accept the packed form. Both are read; neither is
   * skipped as unknown.
   */
  test("a repeated numeric field accepts both of the forms the schema allows", () => {
    expect(decodeProto("Message.ImageMessage", bytes(0xb0, 0x01, 0x07, 0xb0, 0x01, 0x09))).toEqual({
      scanLengths: [7, 9],
    });
    expect(decodeProto("Message.ImageMessage", bytes(0xb2, 0x01, 0x02, 0x07, 0x09))).toEqual({
      scanLengths: [7, 9],
    });
  });
});

describe("framing that is malformed rather than unknown", () => {
  /**
   * An unknown field is skipped and reading goes on; a tag that cannot frame
   * anything is not a field at all. Field number 0 does not exist, and the
   * end-group wire type has no opening group in a schema that declares none —
   * every reference parser rejects the message, and the alternative here is
   * worse than rejecting: leaving the read loop hands back everything read so
   * far, which a caller cannot tell from a message that never carried the rest.
   *
   * url = 1 -> 0x0a, height = 6 -> 0x30, so the field after the bad tag is one
   * a working decode would have to return.
   */
  test("an end-group tag is reported, not treated as the end of the message", () => {
    // 4 << 3 | 4 = 0x24, an end group for a field never opened.
    expect(() =>
      decodeProto("Message.ImageMessage", bytes(0x0a, 0x02, 0x61, 0x62, 0x24, 0x30, 0x07)),
    ).toThrow(RangeError);
    expect(() =>
      decodeProto("Message.ImageMessage", bytes(0x0a, 0x02, 0x61, 0x62, 0x24, 0x30, 0x07)),
    ).toThrow("illegal protobuf tag 36 at offset 5");
  });

  /**
   * Field number 0 does not exist, and no wire type makes it exist: `0x02` is
   * field 0 delimiting nothing, which reads as a well-formed empty frame unless
   * the field number itself is the thing being checked.
   */
  test.each([0, 1, 2, 3, 4, 5, 6, 7])(
    "a field-zero tag is reported under wire type %i",
    (wire) => {
      expect(() =>
        decodeProto(
          "Message.ImageMessage",
          bytes(0x0a, 0x02, 0x61, 0x62, wire, 0, 0, 0, 0, 0, 0, 0, 0, 0x30, 0x07),
        ),
      ).toThrow(`illegal protobuf tag ${wire} at offset 5`);
    },
  );

  test("the same tags inside a submessage reach the caller", () => {
    // Message.imageMessage = 3 -> 0x1a, framing the bytes above.
    expect(() =>
      decodeProto("Message", bytes(0x1a, 0x07, 0x0a, 0x02, 0x61, 0x62, 0x24, 0x30, 0x07)),
    ).toThrow(RangeError);
  });

  test("the wire types that do frame something are still skipped, not reported", () => {
    // Field 13 is undeclared on ImageMessage: varint, 64-bit, 32-bit and
    // length-delimited all say where they end, so all four are skipped.
    expect(decodeProto("Message.ImageMessage", bytes(0x68, 0x07, 0x30, 0x07))).toEqual({ height: 7 });
    expect(
      decodeProto("Message.ImageMessage", bytes(0x69, 1, 2, 3, 4, 5, 6, 7, 8, 0x30, 0x07)),
    ).toEqual({ height: 7 });
    expect(decodeProto("Message.ImageMessage", bytes(0x6d, 1, 2, 3, 4, 0x30, 0x07))).toEqual({
      height: 7,
    });
    expect(decodeProto("Message.ImageMessage", bytes(0x6a, 0x02, 0x61, 0x62, 0x30, 0x07))).toEqual({
      height: 7,
    });
  });
});

describe("the payloads a differential fuzzer reported", () => {
  /**
   * Message.ImageMessage. The divergence is at offset 145, where the tag
   * 0x9a 0x01 puts wire type 2 on firstScanLength: the four bytes it frames are
   * skipped and reading resumes at 152, so experimentGroupId at 152 and
   * staticUrl at 155 are read rather than consumed as part of the field.
   */
  const REPORTED_IMAGE_MESSAGE = hexBytes(
    "0a7f0a01302200280030ffffffff0f4a10faa90f77ea12e1915adff0f42a6e438a5a4078787878787878787878787878" +
      "787878787878787878787878787878787878787878787878787878787878787878787878787878787878787878787878" +
      "787878820120f8d92151174779bcabbcdcf508bcc2bc3d2e82f0a902cf3fc85b2d790a007e3a8a01c802120568656c6c" +
      "6f9a0104e9336889a00101ea0100f801ffffffff0f82020808021a04f09f918d9a020d7b226a736f6e223a747275657d" +
      "b00200ca02040a020a09da02110a0568656c6c6f1a0020012a04f09f918dea0204f09f918df20201618003018a030210" +
      "019203046e756c6c9a03046e756c6cba03641f299d16af40560fbde890074d7d998b23abe4a328702998d228e8ccbc05" +
      "add7b8ca11719f71319fbdcf05df9d0617230e4d5af2f6ba21df8b188510a04bec07938bc4f3256dbf0937b18bf740aa" +
      "e3295225ec57059b00bd05b41ef1ff67416726b873dad80300e003ffffffff07f80301800401980400a2045108ffffff" +
      "ffffffffffff0112001a420a407878787878787878787878787878787878787878787878787878787878787878787878" +
      "7878787878787878787878787878787878787878787878787878787878c20402080b9201105e9c3e93e62da07b9367ee" +
      "01856190a9980101aa0101f6b001ffffffff0fda0100e20164525f2e1ca4027dd92632919f2fc4aaee4342c7bb992e3c" +
      "27e19ef4defbde2f3b0e7158e4a1a1196bd62d81ce6e6f1a5f8af2dcef170a95b92db6e4d0fad8e099fe9ae6f8b11725" +
      "393614a2059dc82a6a8b8a75f2c19d4d5aae960efe60d38ce20a9020fcf8010382020568656c6c6f",
  );

  test("Message.ImageMessage, a mismatched wire type mid-message", () => {
    const decoded = decodeProto("Message.ImageMessage", REPORTED_IMAGE_MESSAGE) as Record<
      string,
      unknown
    >;

    expect(REPORTED_IMAGE_MESSAGE.length).toBe(616);
    // The tag at 145, and the two fields it would swallow if it were read as
    // firstScanLength rather than skipped.
    expect(Array.from(REPORTED_IMAGE_MESSAGE.subarray(145, 158))).toEqual([
      0x9a, 0x01, 0x04, 0xe9, 0x33, 0x68, 0x89, 0xa0, 0x01, 0x01, 0xea, 0x01, 0x00,
    ]);
    expect(decoded.experimentGroupId).toBe(1);
    expect(decoded.staticUrl).toBe("");
    // firstScanLength is set by the well-formed tag at 482, not by the frame at 145.
    expect(decoded.firstScanLength).toBe(1);
    expect(Object.keys(decoded).sort()).toEqual([
      "accessibilityLabel",
      "experimentGroupId",
      "firstScanLength",
      "firstScanSidecar",
      "imageSourceType",
      "mimetype",
      "scanLengths",
      "scansSidecar",
      "staticUrl",
      "thumbnailEncSha256",
      "thumbnailSha256",
      "url",
    ]);
    expect(decoded.mimetype).toBe("hello");
    expect(decoded.accessibilityLabel).toBe("hello");
    expect(decoded.imageSourceType).toBe(3);
    expect(decoded.scanLengths).toEqual([4294967295]);
    // url is the 127 bytes its length prefix declares, not a byte more.
    expect((decoded.url as string).length).toBe(124);
    expect([...(decoded.url as string)].slice(0, 3).map((unit) => unit.codePointAt(0))).toEqual([
      0x0a, 0x01, 0x30,
    ]);
  });

  /**
   * HistorySync, 256 frames of field 1 nested one inside the next. syncType is
   * an enum, so the outermost tag is already the mismatched one: the whole
   * 701-byte frame is skipped in a single step and nothing below it is ever
   * looked at, let alone abandoned at a depth limit.
   */
  test("HistorySync, 256 nested frames on an enum field", () => {
    let payload: number[] = [];
    for (let level = 0; level < 256; level++) {
      payload = [0x0a, ...varint(payload.length), ...payload];
    }
    const nested = new Uint8Array(payload);

    expect(nested.length).toBe(704);
    expect(hexOf(nested.subarray(0, 11))).toBe("0abd050aba050ab7050ab4");
    expect(hexOf(nested.subarray(-18))).toBe("0a100a0e0a0c0a0a0a080a060a040a020a00");
    expect(decodeProto("HistorySync", nested)).toEqual({});
  });
});

describe("a declared length against what is left of the buffer", () => {
  // Message.ImageMessage.url = 1 -> tag 0x0a; height = 6 -> tag 0x30.
  test("declared exactly what is left", () => {
    expect(decodeProto("Message.ImageMessage", bytes(0x0a, 0x02, 0x61, 0x62))).toEqual({
      url: "ab",
    });
  });

  test("declared one byte more than is left", () => {
    expect(() => decodeProto("Message.ImageMessage", bytes(0x0a, 0x03, 0x61, 0x62))).toThrow(
      RangeError,
    );
  });

  test("declared far past the end of the buffer", () => {
    expect(() =>
      decodeProto("Message.ImageMessage", bytes(0x0a, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x61, 0x62)),
    ).toThrow(RangeError);
    // The same length on a bytes field, which reads through a different method.
    expect(() =>
      decodeProto("Message.ImageMessage", bytes(0x22, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x61)),
    ).toThrow(RangeError);
  });

  test("declared zero", () => {
    expect(decodeProto("Message.ImageMessage", bytes(0x0a, 0x00, 0x30, 0x07))).toEqual({
      url: "",
      height: 7,
    });
  });

  /**
   * A length is a varint, and a varint ends at the first byte with the
   * continuation bit clear — however many redundant groups precede it. Ending
   * the length anywhere else moves the field's first byte and every boundary
   * after it, so all ten spellings of "two" have to frame the same two bytes.
   */
  test.each([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])("a length of two spelled in %i varint bytes", (width) => {
    expect(
      decodeProto(
        "Message.ImageMessage",
        new Uint8Array([0x0a, ...paddedVarint(2, width), 0x61, 0x62, 0x30, 0x07]),
      ),
    ).toEqual({ url: "ab", height: 7 });
  });
});

describe("nesting depth", () => {
  // Message.ephemeralMessage = 40 -> tag 0xc2 0x02, framing a
  // Message.FutureProofMessage whose message = 1 -> tag 0x0a frames a Message.
  // Sized from the inside out, then written from the outside in, so building a
  // hundred-thousand-level payload stays linear.
  const nestMessages = (levels: number): Uint8Array => {
    const outer: number[] = [0];
    const inner: number[] = [];
    for (let level = 1; level <= levels; level++) {
      const framed = 1 + varint(outer[level - 1]!).length + outer[level - 1]!;
      inner.push(framed);
      outer.push(2 + varint(framed).length + framed);
    }
    const out = new Uint8Array(outer[levels]!);
    let pos = 0;
    for (let level = levels; level > 0; level--) {
      out.set([0xc2, 0x02, ...varint(inner[level - 1]!), 0x0a, ...varint(outer[level - 1]!)], pos);
      pos = out.length - outer[level - 1]!;
    }
    return out;
  };

  const depthOf = (decoded: unknown): number => {
    let depth = 0;
    let node = decoded as { ephemeralMessage?: { message?: unknown } } | undefined;
    while (node?.ephemeralMessage) {
      depth++;
      node = node.ephemeralMessage.message as { ephemeralMessage?: { message?: unknown } };
    }
    return depth;
  };

  test("one level decodes to the message it frames", () => {
    expect(decodeProto("Message", nestMessages(1))).toEqual({ ephemeralMessage: { message: {} } });
  });

  /**
   * 64 decode frames is where the decoder these payloads were compared against
   * gives up; 1000 is far past it and still well inside any host stack this
   * runs on. Reading these does not by itself rule out a larger configured cap
   * — a cap and a stack ceiling look the same from here — so the absence of one
   * is pinned structurally below rather than inferred from a depth that worked.
   */
  test.each([63, 64, 65, 1000])("%i levels are past where the compared decoder stops", (levels) => {
    expect(depthOf(decodeProto("Message", nestMessages(levels)))).toBe(levels);
  });

  /**
   * What a configured cap would look like in the codec itself. protobufjs
   * carries `Reader.recursionLimit` and threads a depth argument through every
   * generated `decode`; ts-proto emits neither, and this is the assertion that
   * would fail if it started to.
   */
  test("the generated codec counts no depth", () => {
    const generated = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "..", "ts", "generated", "whatsapp.ts"),
      "utf8",
    );

    expect(generated).not.toContain("recursionLimit");
    expect(generated).not.toContain("nesting depth");
    // Every decode takes the same three parameters and no fourth: a depth would
    // have to be threaded through all of them to be counted.
    const parameters = new Set(
      (generated.match(/decode\(input: BinaryReader \| Uint8Array,[^)]*\)/g) ?? []).map((signature) =>
        signature.replace(/into\?: [A-Za-z0-9_]+/, "into?: T"),
      ),
    );

    expect([...parameters]).toEqual([
      "decode(input: BinaryReader | Uint8Array, length?: number, into?: T)",
    ]);
  });

  /**
   * Where the stack gives out is the host's business — 12500 levels on Bun,
   * 2928 on Node, and neither number belongs in an assertion. What is this
   * codec's business is that there is no third outcome: every depth either
   * comes back whole or throws, and none comes back short, which a caller could
   * not tell from a message that never carried those levels.
   */
  test("every depth is read whole or throws, never short", () => {
    const outcomes = [100, 1_000, 5_000, 20_000, 100_000, 400_000].map((levels) => {
      try {
        return depthOf(decodeProto("Message", nestMessages(levels))) === levels ? "read" : "short";
      } catch (error) {
        return error instanceof RangeError ? "threw" : "other";
      }
    });

    expect(outcomes.filter((outcome) => outcome !== "read" && outcome !== "threw")).toEqual([]);
    // The ladder has to straddle the ceiling, or it proves only that shallow
    // payloads decode.
    expect(outcomes).toContain("read");
    expect(outcomes).toContain("threw");
  });
});
