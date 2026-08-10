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
  label: "optional" | "required" | "repeated" | "repeated packed" | "map";
  type: string;
  ref: string;
}

const FIELDS: Field[] = [];
const MESSAGES = new Set<string>();
const BY_MESSAGE = new Map<string, Field[]>();

for (const line of readFileSync(SURFACE_FILE, "utf8").split("\n")) {
  if (line.length === 0 || line.startsWith("#")) continue;
  const [message, name, number, label, type, ref = ""] = line.split("\t");
  MESSAGES.add(message!);
  // A message the schema declares with no fields carries only its own path.
  if (name === undefined) continue;
  const field: Field = {
    message: message!,
    name: name!,
    number: Number(number),
    label: label as Field["label"],
    type: type!,
    ref,
  };
  FIELDS.push(field);
  BY_MESSAGE.set(message!, [...(BY_MESSAGE.get(message!) ?? []), field]);
}

const isRepeated = (field: Field): boolean => field.label.startsWith("repeated");

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
    // Negative where the type is signed: a decoder quietly swapped for its
    // unsigned counterpart round-trips every non-negative sample and only
    // misreads what a peer sends below zero.
    case "int32":
    case "int64":
    case "sint32":
    case "sint64":
    case "sfixed32":
    case "sfixed64":
    case "float":
    case "double":
      return -1;
    default:
      return 1;
  }
};

// Two entries, not one: a single-entry map cannot tell an encoder that writes
// only the first property, or a decoder that replaces the map per wire entry,
// from a correct one. Integer-like keys iterate ascending in JS, so these are
// ordered to match the order they are written in.
const MAP_KEY_SAMPLES: Record<string, [string, string]> = {
  string: ["k", "m"],
  bool: ["false", "true"],
  int32: ["7", "9"],
  int64: ["7", "9"],
  uint32: ["7", "9"],
  uint64: ["7", "9"],
  sint32: ["7", "9"],
  sint64: ["7", "9"],
};
const MAP_VALUE_SAMPLES: Record<string, [unknown, unknown]> = {
  string: ["v", "w"],
  bytes: [new Uint8Array([1]), new Uint8Array([2, 3])],
  bool: [true, false],
};

const mapTypes = (field: Field): [string, string] => {
  const [key, value] = field.type.slice("map<".length, -1).split(",");
  return [key!, value!];
};

const mapEntriesOf = (field: Field): Array<[string, unknown]> => {
  const [keyType, valueType] = mapTypes(field);
  const keys = MAP_KEY_SAMPLES[keyType];
  if (keys === undefined) throw new Error(`no sample for map key ${keyType}`);
  const values: [unknown, unknown] =
    valueType === "message"
      ? [nestedSampleOf(field.ref), nestedSampleOf(field.ref, true)]
      : (MAP_VALUE_SAMPLES[valueType] ?? [7, 9]);
  return [
    [keys[0], values[0]],
    [keys[1], values[1]],
  ];
};

/** A map key travels as its declared scalar type, not as the JS object key. */
const mapKeyValue = (keyType: string, key: string): unknown => {
  if (keyType === "string") return key;
  if (keyType === "bool") return key === "true";
  return Number(key);
};

const sampleOf = (field: Field): unknown => {
  if (field.label === "map") return Object.fromEntries(mapEntriesOf(field));
  const value = zeroOf(field.type, field.ref);
  return isRepeated(field) ? [value] : value;
};

/**
 * One declared child field of a referenced message. An empty `{}` proves a
 * submessage was written but not *which* codec wrote it — two message types
 * both encode `{}` as a zero-length payload — so the payload sample carries a
 * field only the right codec knows. A scalar child keeps this to one level:
 * `Field.subfield` is a `map<uint32, Field>`, so descending into messages would
 * not terminate.
 */
const nestedSampleOf = (ref: string, alternate = false): Record<string, unknown> => {
  const child = BY_MESSAGE.get(ref)?.find(
    (candidate) => candidate.label !== "map" && candidate.type !== "message",
  );
  if (!child) return {};
  const value = alternate ? (distinctOf(child.type) ?? sampleOf(child)) : sampleOf(child);
  return { [keyOf(child)]: isRepeated(child) ? [value].flat() : value };
};

/** The zero sample, plus a payload-bearing one where the type allows it. */
const samplesOf = (field: Field): unknown[] => {
  const samples = [sampleOf(field)];
  if (field.label === "map") return samples;
  const distinct =
    field.type === "message" ? nestedSampleOf(field.ref) : distinctOf(field.type);
  const usable =
    distinct !== undefined && !(field.type === "message" && Object.keys(distinct).length === 0);
  if (!isRepeated(field)) {
    if (usable) samples.push(distinct);
    return samples;
  }
  // Two elements, not one: a one-element array cannot tell an encoder that
  // emits only the head, or a decoder that resets the array per occurrence,
  // from a correct one. An enum or a childless message has no second value to
  // offer, so it repeats the first — that still pins the count.
  const element = usable ? distinct : zeroOf(field.type, field.ref);
  samples.push([element, zeroOf(field.type, field.ref)]);
  return samples;
};

const keyOf = (field: Field): string =>
  JSON_NAME_KEYS[`${field.message}.${field.name}`] ?? field.name;

/** The first tag in an encoded message, split into field number and wire type. */
const firstTag = (bytes: Uint8Array): [number, number] => {
  let tag = 0;
  for (let shift = 0, i = 0; i < bytes.length; i++, shift += 7) {
    tag |= (bytes[i]! & 0x7f) << shift;
    if ((bytes[i]! & 0x80) === 0) break;
  }
  return [tag >>> 3, tag & 7];
};

// Anything absent is a varint. A wrong wire type here is not a rename the peer
// recovers from: it makes the field unreadable to everyone but this codec, and
// a decoder making the matching mistake hides it from the round trip.
const WIRE_TYPE: Record<string, number> = {
  double: 1,
  fixed64: 1,
  sfixed64: 1,
  float: 5,
  fixed32: 5,
  sfixed32: 5,
  string: 2,
  bytes: 2,
  message: 2,
};

const wireTypeOf = (field: Field): number => {
  if (field.label === "map" || field.label === "repeated packed") return 2;
  return WIRE_TYPE[field.type] ?? 0;
};

// An independent encoder for the declared type, so the codec is checked against
// protobuf rather than against itself. int32 and sint32 share a wire type and a
// self-consistent encoder/decoder pair hides the zigzag; only the bytes tell
// them apart, so the samples are chosen to differ under every candidate
// encoding — negative where the type is signed, multi-byte where it is not.
const WIRE_SAMPLES: Record<string, [unknown, unknown]> = {
  int32: [-1, 2],
  int64: [-1, 2],
  sint32: [-1, 2],
  sint64: [-1, 2],
  sfixed32: [-1, 2],
  sfixed64: [-1, 2],
  uint32: [300, 1],
  uint64: [300, 1],
  fixed32: [300, 1],
  fixed64: [300, 1],
  float: [-1.5, 2.5],
  double: [-1.5, 2.5],
  bool: [true, false],
  string: ["ß", "z"],
  bytes: [new Uint8Array([0x01, 0x7f, 0xff]), new Uint8Array([0x02])],
};

const varintBytes = (value: bigint): number[] => {
  const out: number[] = [];
  let rest = value;
  do {
    const byte = Number(rest & 0x7fn);
    rest >>= 7n;
    out.push(rest > 0n ? byte | 0x80 : byte);
  } while (rest > 0n);
  return out;
};

const fixedBytes = (size: number, write: (view: DataView) => void): number[] => {
  const view = new DataView(new ArrayBuffer(size));
  write(view);
  return [...new Uint8Array(view.buffer)];
};

const lengthPrefixed = (raw: number[]): number[] => [...varintBytes(BigInt(raw.length)), ...raw];

const payloadBytes = (type: string, value: unknown): number[] => {
  // Only reached by the integer cases; a string or bytes sample is not a number.
  const numeric = (): bigint => BigInt(value as number);
  switch (type) {
    case "int32":
    case "int64":
    case "enum":
      return varintBytes(BigInt.asUintN(64, numeric()));
    case "uint32":
    case "uint64":
      return varintBytes(numeric());
    case "sint32":
    case "sint64": {
      const signed = numeric();
      return varintBytes(BigInt.asUintN(64, (signed << 1n) ^ (signed >> 63n)));
    }
    case "bool":
      return [value ? 1 : 0];
    case "fixed32":
      return fixedBytes(4, (view) => view.setUint32(0, Number(numeric()), true));
    case "sfixed32":
      return fixedBytes(4, (view) => view.setInt32(0, Number(numeric()), true));
    case "fixed64":
      return fixedBytes(8, (view) => view.setBigUint64(0, BigInt.asUintN(64, numeric()), true));
    case "sfixed64":
      return fixedBytes(8, (view) => view.setBigInt64(0, numeric(), true));
    case "float":
      return fixedBytes(4, (view) => view.setFloat32(0, value as number, true));
    case "double":
      return fixedBytes(8, (view) => view.setFloat64(0, value as number, true));
    case "string":
      return lengthPrefixed([...new TextEncoder().encode(value as string)]);
    case "bytes":
      return lengthPrefixed([...(value as Uint8Array)]);
    default:
      throw new Error(`no wire encoding for ${type}`);
  }
};

const tagBytes = (number: number, wire: number): number[] => varintBytes(BigInt(number * 8 + wire));

/**
 * A map entry is a submessage of key at 1 and value at 2, so its key and value
 * types have wire encodings of their own. Encoding a `uint32` key as `sint32`
 * on both sides round-trips here and is read as a different key by anyone else.
 */
const mapEntryBytes = (field: Field, key: string, value: unknown): number[] => {
  const [keyType, valueType] = mapTypes(field);
  const valuePayload =
    valueType === "message"
      ? lengthPrefixed(nestedMessageBytes(field.ref, value as Record<string, unknown>))
      : payloadBytes(valueType, value);
  return [
    ...tagBytes(1, WIRE_TYPE[keyType] ?? 0),
    ...payloadBytes(keyType, mapKeyValue(keyType, key)),
    ...tagBytes(2, WIRE_TYPE[valueType] ?? 0),
    ...valuePayload,
  ];
};

/** The declared encoding of a one-scalar-child nested sample. */
const nestedMessageBytes = (ref: string, sample: Record<string, unknown>): number[] => {
  const [entry] = Object.entries(sample);
  if (entry === undefined) return [];
  const [childKey, childValue] = entry;
  const child = BY_MESSAGE.get(ref)?.find((candidate) => keyOf(candidate) === childKey);
  if (child === undefined) throw new Error(`${ref} does not declare ${childKey}`);
  return [...tagBytes(child.number, wireTypeOf(child)), ...payloadBytes(child.type, childValue)];
};

const expectedEncoding = (field: Field, values: unknown[]): number[] => {
  const tag = (wire: number): number[] => tagBytes(field.number, wire);
  if (field.label === "map") {
    return mapEntriesOf(field).flatMap(([key, value]) => [
      ...tag(2),
      ...lengthPrefixed(mapEntryBytes(field, key, value)),
    ]);
  }
  if (field.label === "repeated packed") {
    const payload = values.flatMap((value) => payloadBytes(field.type, value));
    return [...tag(2), ...lengthPrefixed(payload)];
  }
  return values.flatMap((value) => [...tag(wireTypeOf(field)), ...payloadBytes(field.type, value)]);
};

const hex = (bytes: Iterable<number>): string =>
  Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");

const sameValue = (type: string, ref: string, expected: unknown, actual: unknown): boolean => {
  if (type === "bytes") {
    const wanted = expected as Uint8Array;
    return (
      actual instanceof Uint8Array &&
      actual.length === wanted.length &&
      wanted.every((byte, index) => actual[index] === byte)
    );
  }
  if (type === "message") {
    if (typeof actual !== "object" || actual === null) return false;
    // An empty submessage carries nothing to compare; that it decoded to an
    // object at all is the difference between written and dropped.
    const [child] = Object.entries(expected as Record<string, unknown>);
    if (child === undefined) return true;
    const [childKey, childValue] = child;
    const childField = BY_MESSAGE.get(ref)?.find((candidate) => keyOf(candidate) === childKey);
    return childField !== undefined && readsBack(childField, childValue, (actual as Record<string, unknown>)[childKey]);
  }
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
      return decodedKey === key && sameValue(valueType, field.ref, value, decodedValue);
    });
  }
  if (isRepeated(field)) {
    const expected = sample as unknown[];
    return (
      Array.isArray(actual) &&
      actual.length === expected.length &&
      expected.every((value, index) => sameValue(field.type, field.ref, value, actual[index]))
    );
  }
  return sameValue(field.type, field.ref, sample, actual);
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

  test("every declared field is written, at its declared number and wire type", () => {
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
      const [written, wire] = firstTag(bytes);
      if (written !== field.number) failures.push(`${label}: written at #${written}`);
      else if (wire !== wireTypeOf(field)) {
        failures.push(`${label}: wire type ${wire}, schema declares ${wireTypeOf(field)}`);
      }
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

  test("every scalar field puts on the wire exactly the bytes its declared type implies", () => {
    const failures: string[] = [];
    for (const field of FIELDS) {
      if (field.type === "message" && field.label !== "map") continue;
      let values: unknown[];
      let input: unknown;
      if (field.label === "map") {
        values = [];
        input = sampleOf(field);
      } else {
        const samples =
          field.type === "enum"
            ? ([zeroOf("enum", field.ref), zeroOf("enum", field.ref)] as [unknown, unknown])
            : WIRE_SAMPLES[field.type];
        if (samples === undefined) {
          failures.push(`${field.message}.${field.name}: no wire sample for ${field.type}`);
          continue;
        }
        values = isRepeated(field) ? [samples[0], samples[1]] : [samples[0]];
        input = isRepeated(field) ? values : values[0];
      }
      const written = hex(encodeProto(field.message, { [keyOf(field)]: input }));
      const implied = hex(expectedEncoding(field, values));
      if (written !== implied) {
        failures.push(
          `${field.message}.${field.name} (#${field.number}, ${field.label} ${field.type}): wrote ${written}, schema implies ${implied}`,
        );
      }
    }
    expect(failures).toEqual([]);
  });

  test("a field the wire omits decodes as absent, not as its default", () => {
    // "Absent is absent" is the bridge's rule, and it is also what makes the
    // zero samples above mean anything: if an omitted field came back as 0 or
    // "", no round trip could tell a written zero from a missing one.
    const failures: string[] = [];
    const empty = new Uint8Array();
    for (const message of MESSAGES) {
      const decoded = decodeProto(message, empty) as Record<string, unknown>;
      for (const field of BY_MESSAGE.get(message) ?? []) {
        if (field.label === "required") continue;
        const read = decoded[keyOf(field)];
        if (read !== undefined) {
          failures.push(`${field.message}.${field.name}: absent decoded as ${JSON.stringify(read)}`);
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
