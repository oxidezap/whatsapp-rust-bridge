/**
 * The schema is the authority: for every field `whatsapp.proto` declares, the
 * generated codec must write bytes, under the declared name, at the declared
 * number — and read the same value back.
 *
 * The field list is not written by hand and not read out of the codec. It is
 * `ts/generated/whatsapp-surface.txt`, which `scripts/gen-ts-proto.ts` derives
 * from the very protoc invocation that produces the codec, so the two cannot
 * describe different schemas.
 *
 * Run: bun test tests/proto-schema-surface.test.ts
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { initWasmEngine, encodeProto, decodeProto } from "../dist/index.js";

const SURFACE_FILE = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "ts",
  "generated",
  "whatsapp-surface.txt",
);

// protoc derives json_name by camel-casing across `_`, and ts-proto emits that
// name rather than the declared one. Every other field in this schema is
// already camelCase, so a field reaching this list means upstream introduced a
// snake_case name and somebody has to decide what the bridge calls it.
//
// Not renamed back: camelCase is what the rest of the bridge already speaks for
// this field — `src/camel_serializer.rs` hands every Rust->JS value over with
// camelCase keys, and `ts/proto-types.d.ts` declares `musicUserIdMap`. A codec
// keyed on `music_user_id_map` would silently drop the field when a decoded
// event is re-encoded, which is the failure this whole file exists to catch.
const JSON_NAME_KEYS: Record<string, string> = {
  "SyncActionValue.MusicUserIdAction.music_user_id_map": "musicUserIdMap",
};

interface Field {
  message: string;
  name: string;
  number: number;
  label: "optional" | "required" | "repeated" | "map";
  type: string;
  ref: string;
}

const FIELDS: Field[] = [];
const MESSAGES = new Set<string>();

for (const line of readFileSync(SURFACE_FILE, "utf8").split("\n")) {
  if (line.length === 0 || line.startsWith("#")) continue;
  const [message, name, number, label, type, ref = ""] = line.split("\t");
  MESSAGES.add(message!);
  // A message the schema declares with no fields carries only its own path.
  if (name === undefined) continue;
  FIELDS.push({
    message: message!,
    name: name!,
    number: Number(number),
    label: label as Field["label"],
    type: type!,
    ref,
  });
}

/**
 * The zero value for the field's type. Zero is deliberate: a proto2 `optional`
 * field has explicit presence, so an encoder that writes only truthy values
 * loses the difference between "zero" and "unset" — and only a zero-valued
 * sample can catch that.
 */
const zeroOf = (type: string, ref: string): unknown => {
  switch (type) {
    case "double":
    case "float":
    case "int32":
    case "int64":
    case "uint32":
    case "uint64":
    case "sint32":
    case "sint64":
    case "fixed32":
    case "fixed64":
    case "sfixed32":
    case "sfixed64":
      return 0;
    case "bool":
      return false;
    case "string":
      return "";
    case "bytes":
      return new Uint8Array();
    case "message":
      return {};
    case "enum":
      // Not necessarily 0: a proto2 enum need not declare a zero value, so the
      // manifest carries the first value it does declare.
      return Number(ref.slice(ref.lastIndexOf("=") + 1));
    default:
      throw new Error(`no sample for type ${type}`);
  }
};

/**
 * A second sample per field, for the round trip only: a zero value proves
 * presence but cannot prove the payload survived — an encoder that swapped the
 * contents of an empty `bytes` still produces an empty `bytes`. Types with no
 * value distinguishable from the schema's own (message, enum) return undefined
 * and are covered by the zero pass alone.
 */
const distinctOf = (type: string): unknown => {
  switch (type) {
    case "bytes":
      return new Uint8Array([0x01, 0x7f, 0xff]);
    case "string":
      return "surface";
    case "bool":
      return true;
    case "message":
    case "enum":
      return undefined;
    default:
      return 1;
  }
};

// A map sample needs a key that survives the declared key type, and a value
// distinguishable from a dropped one, so these are not zero values.
const MAP_KEY_SAMPLE: Record<string, string> = { string: "k", bool: "false", int32: "7", int64: "7", uint32: "7", uint64: "7", sint32: "7", sint64: "7" };
const MAP_VALUE_SAMPLE: Record<string, unknown> = { string: "v", bytes: new Uint8Array([1]), bool: true, message: {} };

const mapTypes = (field: Field): [string, string] => {
  const [key, value] = field.type.slice("map<".length, -1).split(",");
  return [key!, value!];
};

const sampleOf = (field: Field): unknown => {
  if (field.label === "map") {
    const [key, value] = mapTypes(field);
    const mapKey = MAP_KEY_SAMPLE[key];
    const mapValue = MAP_VALUE_SAMPLE[value] ?? 7;
    if (mapKey === undefined) throw new Error(`no sample for map key ${key}`);
    return { [mapKey]: mapValue };
  }
  const value = zeroOf(field.type, field.ref);
  return field.label === "repeated" ? [value] : value;
};

/** The zero sample, plus a payload-bearing one where the type allows it. */
const samplesOf = (field: Field): unknown[] => {
  const samples = [sampleOf(field)];
  if (field.label === "map") return samples;
  const distinct = distinctOf(field.type);
  if (distinct !== undefined) samples.push(field.label === "repeated" ? [distinct] : distinct);
  return samples;
};

const keyOf = (field: Field): string =>
  JSON_NAME_KEYS[`${field.message}.${field.name}`] ?? field.name;

/** Field number carried by the first tag in an encoded message. */
const firstFieldNumber = (bytes: Uint8Array): number => {
  let tag = 0;
  for (let shift = 0, i = 0; i < bytes.length; i++, shift += 7) {
    tag |= (bytes[i]! & 0x7f) << shift;
    if ((bytes[i]! & 0x80) === 0) break;
  }
  return tag >>> 3;
};

const sameValue = (type: string, expected: unknown, actual: unknown): boolean => {
  if (type === "bytes") {
    const wanted = expected as Uint8Array;
    return (
      actual instanceof Uint8Array &&
      actual.length === wanted.length &&
      wanted.every((byte, index) => actual[index] === byte)
    );
  }
  // An empty submessage carries nothing to compare; that it decoded to an
  // object at all is the difference between written and dropped.
  if (type === "message") return typeof actual === "object" && actual !== null;
  return actual === expected;
};

const readsBack = (field: Field, sample: unknown, actual: unknown): boolean => {
  if (actual === undefined || actual === null) return false;
  if (field.label === "map") {
    const expected = Object.entries(sample as Record<string, unknown>);
    const decoded = Object.entries(actual as Record<string, unknown>);
    if (decoded.length !== expected.length) return false;
    const [, valueType] = mapTypes(field);
    return expected.every(([key, value], index) => {
      const [decodedKey, decodedValue] = decoded[index]!;
      return decodedKey === key && sameValue(valueType, value, decodedValue);
    });
  }
  if (field.label === "repeated") {
    const expected = sample as unknown[];
    return (
      Array.isArray(actual) &&
      actual.length === expected.length &&
      expected.every((value, index) => sameValue(field.type, value, actual[index]))
    );
  }
  return sameValue(field.type, sample, actual);
};

beforeAll(() => {
  initWasmEngine();
});

describe("generated codec vs whatsapp.proto", () => {
  test("the surface manifest covers the whole schema", () => {
    expect(FIELDS.length).toBeGreaterThan(3000);
    expect(MESSAGES.size).toBeGreaterThan(600);
    expect(FIELDS.filter((field) => field.label === "map").length).toBeGreaterThan(0);
    // Fieldless messages have a generated type too, and are only in MESSAGES if
    // the manifest gave them a line of their own.
    expect(MESSAGES.size).toBeGreaterThan(new Set(FIELDS.map((field) => field.message)).size);
  });

  test("every declared message resolves to a codec", () => {
    const missing = [...MESSAGES].filter((message) => {
      try {
        encodeProto(message, {});
        return false;
      } catch {
        return true;
      }
    });
    expect(missing).toEqual([]);
  });

  test("every declared field is written, at its declared number", () => {
    const failures: string[] = [];
    for (const field of FIELDS) {
      const key = keyOf(field);
      const label = `${field.message}.${field.name} (#${field.number}, ${field.label} ${field.type})`;
      let bytes: Uint8Array;
      try {
        bytes = encodeProto(field.message, { [key]: sampleOf(field) });
      } catch (error) {
        failures.push(`${label}: encode threw ${error}`);
        continue;
      }
      if (bytes.length === 0) {
        failures.push(`${label}: nothing written under "${key}"`);
        continue;
      }
      const written = firstFieldNumber(bytes);
      if (written !== field.number) failures.push(`${label}: written at #${written}`);
    }
    expect(failures).toEqual([]);
  });

  test("the schema-declared name is the key the codec writes, bar one pinned field", () => {
    const dropped: string[] = [];
    for (const field of FIELDS) {
      let bytes: Uint8Array;
      try {
        bytes = encodeProto(field.message, { [field.name]: sampleOf(field) });
      } catch {
        continue;
      }
      if (bytes.length === 0) dropped.push(`${field.message}.${field.name}`);
    }
    // Every field whose declared name the encoder ignores, enumerated. Anything
    // beyond the pinned entry is a boundary rename the bridge must not make.
    expect(dropped.sort()).toEqual(Object.keys(JSON_NAME_KEYS).sort());
  });

  test("every declared field reads back the value it was written with", () => {
    const failures: string[] = [];
    for (const field of FIELDS) {
      const key = keyOf(field);
      for (const sample of samplesOf(field)) {
        let read: unknown;
        try {
          const bytes = encodeProto(field.message, { [key]: sample });
          read = (decodeProto(field.message, bytes) as Record<string, unknown>)[key];
        } catch (error) {
          failures.push(`${field.message}.${field.name} (#${field.number}): ${error}`);
          continue;
        }
        if (!readsBack(field, sample, read)) {
          failures.push(
            `${field.message}.${field.name} (#${field.number}): wrote ${JSON.stringify(sample)}, read back ${JSON.stringify(read)}`,
          );
        }
      }
    }
    expect(failures).toEqual([]);
  });

  test("the pinned field is the schema's only snake_case name", () => {
    const snakeCase = FIELDS.filter((field) => field.name.includes("_")).map(
      (field) => `${field.message}.${field.name}`,
    );
    expect(snakeCase.sort()).toEqual(Object.keys(JSON_NAME_KEYS).sort());

    for (const [path, jsonName] of Object.entries(JSON_NAME_KEYS)) {
      const field = FIELDS.find((candidate) => `${candidate.message}.${candidate.name}` === path)!;
      expect(field).toBeDefined();
      expect(encodeProto(field.message, { [jsonName]: sampleOf(field) }).length).toBeGreaterThan(0);
    }
  });
});
