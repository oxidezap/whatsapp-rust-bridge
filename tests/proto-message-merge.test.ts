/**
 * Repeated occurrences of a singular message field.
 *
 * The wire format defines one reading for these bytes: "for embedded message
 * fields, the parser merges multiple instances of the same field, as if with
 * the Message::MergeFrom method — that is, all singular scalar fields in the
 * latter instance replace those in the former, singular embedded messages are
 * merged, and repeated fields are concatenated". Two implementations that read
 * the same well-formed bytes as different objects is a parser differential, not
 * a robustness detail, so the reading is pinned here rather than left to
 * whatever the code generator emits.
 *
 * Every byte string below is written by hand from the field number and wire
 * type, never taken from the codec's own output.
 *
 * Run: bun test tests/proto-message-merge.test.ts
 */

import { describe, expect, test } from "bun:test";
import { decodeProto } from "../ts/proto";

const bytes = (...values: number[]) => new Uint8Array(values);

describe("a singular message field read twice", () => {
  // The smallest payload that separates merging from replacing:
  // ExtendedTextMessage.contextInfo = 17 -> tag 0x8a 0x01, twice, each framing
  // one ContextInfo.mentionedJid = 15 (repeated string) -> tag 0x7a.
  test("concatenates the repeated fields of both instances", () => {
    expect(
      decodeProto(
        "Message.ExtendedTextMessage",
        bytes(0x8a, 0x01, 0x03, 0x7a, 0x01, 0x30, 0x8a, 0x01, 0x03, 0x7a, 0x01, 0x31),
      ),
    ).toEqual({ contextInfo: { mentionedJid: ["0", "1"] } });
  });

  // The other half of the same rule, and the half that must not change:
  // ContextInfo.participant = 2 -> tag 0x12, a singular string.
  test("lets the later instance replace a singular scalar", () => {
    expect(
      decodeProto(
        "Message.ExtendedTextMessage",
        bytes(0x8a, 0x01, 0x03, 0x12, 0x01, 0x30, 0x8a, 0x01, 0x03, 0x12, 0x01, 0x31),
      ),
    ).toEqual({ contextInfo: { participant: "1" } });
  });
});

describe("the payloads a differential fuzzer reported", () => {
  /**
   * A declared length larger than what the encoder wrote. The ImageMessage
   * frame still closes, and so does every submessage in it — the inflated
   * length only moved a boundary, pulling the `a0 01 07` that followed
   * (ImageMessage.experimentGroupId = 20) inside contextInfo, where field 20 is
   * conversionDelaySeconds.
   *
   * url = 1 -> 0x0a, contextInfo = 17 -> 0x8a 0x01, staticUrl = 29 -> 0xea 0x01.
   */
  test("Message.ImageMessage, a length that overstates its content", () => {
    expect(
      decodeProto(
        "Message.ImageMessage",
        bytes(
          0x0a, 0x03, 0x78, 0x78, 0x78,
          0x8a, 0x01, 0x06, 0x7a, 0x01, 0x41, 0xa0, 0x01, 0x07,
          0x8a, 0x01, 0x03, 0x7a, 0x01, 0x42,
          0xea, 0x01, 0x01, 0x30,
        ),
      ),
    ).toEqual({
      url: "xxx",
      contextInfo: { mentionedJid: ["A", "B"], conversionDelaySeconds: 7 },
      staticUrl: "0",
    });
  });

  /**
   * Nesting, so the merge has to recurse: HistorySync.globalSettings = 8 ->
   * 0x42 twice, each framing GlobalSettings.lightThemeWallpaper = 1 -> 0x0a,
   * one carrying WallpaperSettings.filename = 1 and the other opacity = 2.
   * Only the innermost message holds both values.
   */
  test("HistorySync, the same field nested twice", () => {
    expect(
      decodeProto(
        "HistorySync",
        bytes(
          0x08, 0x00,
          0x42, 0x06, 0x0a, 0x04, 0x0a, 0x02, 0x78, 0x78,
          0x42, 0x04, 0x0a, 0x02, 0x10, 0x05,
        ),
      ),
    ).toEqual({
      syncType: 0,
      globalSettings: { lightThemeWallpaper: { filename: "xx", opacity: 5 } },
    });
  });

  /**
   * Two encodings back to back. Parsing the concatenation has to equal parsing
   * each and merging the results, which is the property the fuzzer's
   * concatenate mutation tests: `text` is the same in both and stays "hi",
   * `participant` appears only in the first and survives, `mentionedJid`
   * accumulates.
   */
  test("Message.ExtendedTextMessage, two encodings concatenated", () => {
    expect(
      decodeProto(
        "Message.ExtendedTextMessage",
        bytes(
          0x0a, 0x02, 0x68, 0x69,
          0x8a, 0x01, 0x05, 0x7a, 0x01, 0x41, 0x12, 0x00,
          0x0a, 0x02, 0x68, 0x69,
          0x8a, 0x01, 0x03, 0x7a, 0x01, 0x42,
        ),
      ),
    ).toEqual({
      text: "hi",
      contextInfo: { mentionedJid: ["A", "B"], participant: "" },
    });
  });
});
