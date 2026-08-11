/**
 * The build-time check that keeps the generated codec skipping a field whose
 * wire type the schema does not declare.
 *
 * `tests/proto-field-boundaries.test.ts` pins what the codec reads; nothing
 * there fails if the check itself is deleted, because the codec in the tree
 * already carries the guards. So the check is exercised directly here: a codec
 * written by hand against a descriptor built by hand, then one mutation per way
 * a generator upgrade could take the behaviour away.
 *
 * Run: bun test tests/proto-wire-type-guards.test.ts
 */

import { describe, expect, test } from "bun:test";
import { create, toBinary } from "@bufbuild/protobuf";
import {
  FieldDescriptorProto_Label as Label,
  FieldDescriptorProto_Type as Type,
  FileDescriptorSetSchema,
} from "@bufbuild/protobuf/wkt";
import { assertWireTypeGuards } from "../scripts/proto-wire-type-guards";

/**
 * package probe;
 * message Probe {
 *   optional uint32 head  = 1;   // tag 8
 *   optional string body  = 2;   // tag 18
 *   repeated uint32 items = 3;   // tags 24 and 26
 *   message Inner { optional string leaf = 1; }   // tag 10
 * }
 */
const DESCRIPTOR = toBinary(
  FileDescriptorSetSchema,
  create(FileDescriptorSetSchema, {
    file: [
      {
        name: "probe.proto",
        package: "probe",
        syntax: "proto2",
        messageType: [
          {
            name: "Probe",
            field: [
              { name: "head", number: 1, label: Label.OPTIONAL, type: Type.UINT32 },
              { name: "body", number: 2, label: Label.OPTIONAL, type: Type.STRING },
              { name: "items", number: 3, label: Label.REPEATED, type: Type.UINT32 },
            ],
            nestedType: [
              {
                name: "Inner",
                field: [{ name: "leaf", number: 1, label: Label.OPTIONAL, type: Type.STRING }],
              },
            ],
          },
        ],
      },
    ],
  }),
);

/** The shapes ts-proto emits, at the indentation it emits them. */
const CODEC = `export const Probe: MessageFns<Probe> = {
  decode(input: BinaryReader | Uint8Array, length?: number, into?: Probe): Probe {
    while (reader.pos < end) {
      const tag = reader.uint32();
      switch (tag >>> 3) {
        case 1: {
          if (tag !== 8) {
            break;
          }

          message.head = reader.uint32();
          continue;
        }
        case 2: {
          if (tag !== 18) {
            break;
          }

          message.body = reader.string();
          continue;
        }
        case 3: {
          if (tag === 24) {
            message.items!.push(reader.uint32());

            continue;
          }

          if (tag === 26) {
            const end2 = reader.uint32() + reader.pos;
            while (reader.pos < end2) {
              message.items!.push(reader.uint32());
            }

            continue;
          }

          break;
        }
      }
      if ((tag & 7) === 4 || tag === 0) {
        break;
      }
      reader.skip(tag & 7);
    }
    return message;
  },
};

export const Probe_Inner: MessageFns<Probe_Inner> = {
  decode(input: BinaryReader | Uint8Array, length?: number, into?: Probe_Inner): Probe_Inner {
    while (reader.pos < end) {
      const tag = reader.uint32();
      switch (tag >>> 3) {
        case 1: {
          if (tag !== 10) {
            break;
          }

          message.leaf = reader.string();
          continue;
        }
      }
      if ((tag & 7) === 4 || tag === 0) {
        break;
      }
      reader.skip(tag & 7);
    }
    return message;
  },
};
`;

const mutate = (from: string, to: string): string => {
  expect(CODEC.split(from).length - 1).toBe(1);
  return CODEC.replace(from, to);
};

/** The decode loop epilogue is per codec, so mutating it needs one named. */
const mutateProbe = (from: string, to: string): string => {
  const at = CODEC.indexOf("export const Probe_Inner");
  const head = CODEC.slice(0, at);
  expect(head.split(from).length - 1).toBe(1);
  return head.replace(from, to) + CODEC.slice(at);
};

describe("the codec ts-proto emits today", () => {
  test("passes the check", () => {
    expect(() => assertWireTypeGuards(CODEC, DESCRIPTOR)).not.toThrow();
  });
});

describe("a generator upgrade that takes the behaviour away", () => {
  test("guards the wrong tag, accepting the wire type it should skip", () => {
    // head is a uint32, so 8; 10 is the same field number carrying wire type 2.
    expect(() =>
      assertWireTypeGuards(mutate("if (tag !== 8) {", "if (tag !== 10) {"), DESCRIPTOR),
    ).toThrow("Probe field 1 guards tags [10], the schema declares [8]");
  });

  test("drops a guard, reading whatever arrives under that field number", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate("          if (tag !== 18) {\n            break;\n          }\n\n", ""),
        DESCRIPTOR,
      ),
    ).toThrow("Probe field 2 opens its case on [message.body = reader.string();], expected a tag guard");
  });

  /**
   * A read placed ahead of the guard moves the reader before the tag it does
   * not match is skipped, so the skip that follows starts mid-field.
   */
  test("reads before the guard, moving the reader ahead of the skip", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate("        case 1: {\n          if (tag !== 8) {", "        case 1: {\n          reader.uint32();\n          if (tag !== 8) {"),
        DESCRIPTOR,
      ),
    ).toThrow("Probe field 1 opens its case on [reader.uint32();], expected a tag guard");
  });

  test("stops skipping the unknown field its cases break out for", () => {
    expect(() =>
      assertWireTypeGuards(mutateProbe("      reader.skip(tag & 7);", "      // nothing"), DESCRIPTOR),
    ).toThrow("Probe does not skip the unknown field its cases break out for");
  });

  test("inverts the comparison, skipping the one tag it should read", () => {
    expect(() =>
      assertWireTypeGuards(mutate("if (tag !== 8) {", "if (tag === 8) {"), DESCRIPTOR),
    ).toThrow("Probe field 1 guards with [===], expected !==");
  });

  test("falls through instead of leaving the case", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate("if (tag !== 8) {\n            break;", "if (tag !== 8) {\n            reader.skip(tag & 7);"),
        DESCRIPTOR,
      ),
    ).toThrow("Probe field 1 leaves its guard on [nothing], expected break");
  });

  /**
   * The other path through the case, which the guard says nothing about. A
   * singular case that reads and then falls out of the switch reaches the
   * `reader.skip(tag & 7)` the decode loop ends on, and skips the field after
   * the one it just read.
   */
  test("falls out of the switch after reading instead of continuing", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate("message.head = reader.uint32();\n          continue;", "message.head = reader.uint32();"),
        DESCRIPTOR,
      ),
    ).toThrow("Probe field 1 ends its case on [message.head = reader.uint32();], expected continue;");
  });

  test("a repeated numeric case stops leaving on a tag matching neither form", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate("            continue;\n          }\n\n          break;\n        }", "            continue;\n          }\n        }"),
        DESCRIPTOR,
      ),
    ).toThrow("Probe field 3 ends its case on [}], expected break;");
  });

  test("drops the packed branch of a repeated numeric field", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate(
          "          if (tag === 26) {\n            const end2 = reader.uint32() + reader.pos;\n            while (reader.pos < end2) {\n              message.items!.push(reader.uint32());\n            }\n\n            continue;\n          }\n\n",
          "",
        ),
        DESCRIPTOR,
      ),
    ).toThrow("Probe field 3 guards tags [24], the schema declares [24,26]");
  });

  test("emits a case in a shape the check no longer recognises", () => {
    expect(() =>
      assertWireTypeGuards(
        mutate(
          "        case 1: {\n          if (tag !== 10) {\n            break;\n          }\n\n          message.leaf",
          "        case 1:\n          message.leaf",
        ),
        DESCRIPTOR,
      ),
    ).toThrow("Probe_Inner decodes no guarded case for field 1");
  });

  test("stops emitting a codec the schema declares", () => {
    const withoutInner = CODEC.slice(0, CODEC.indexOf("export const Probe_Inner"));

    expect(() => assertWireTypeGuards(withoutInner, DESCRIPTOR)).toThrow(
      "Probe_Inner has no generated codec",
    );
  });

  test("decodes a field number the schema does not declare", () => {
    expect(() =>
      assertWireTypeGuards(mutate("        case 2: {", "        case 9: {"), DESCRIPTOR),
    ).toThrow("Probe decodes field 9, which the schema does not declare");
  });
});

describe("the reindented codec prettier produces for a long type name", () => {
  // Two spaces deeper throughout, which is what ts-proto wrapping
  // `MessageFns<…>` onto its own line does to the whole block.
  const reindented = CODEC.split("\n")
    .map((line) => (line.startsWith("  ") ? `  ${line}` : line))
    .join("\n");

  test("is checked rather than skipped", () => {
    expect(() => assertWireTypeGuards(reindented, DESCRIPTOR)).not.toThrow();
    expect(() =>
      assertWireTypeGuards(reindented.replace("if (tag !== 8) {", "if (tag !== 10) {"), DESCRIPTOR),
    ).toThrow("Probe field 1 guards tags [10], the schema declares [8]");
  });
});
