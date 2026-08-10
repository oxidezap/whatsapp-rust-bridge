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

const parseSurface = (): Field[] =>
  readFileSync(SURFACE_FILE, "utf8")
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => {
      const [message, name, number, label, type, ref = ""] = line.split("\t");
      return {
        message: message!,
        name: name!,
        number: Number(number),
        label: label as Field["label"],
        type: type!,
        ref,
      };
    });

const FIELDS = parseSurface();
const MESSAGES = [...new Set(FIELDS.map((field) => field.message))];

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
      // Not necessarily 0: a proto2 enum need not declare a zero value, and a
      // number outside the enum decodes back as UNRECOGNIZED.
      return Number(ref.slice(ref.lastIndexOf("=") + 1));
    default:
      throw new Error(`no sample for type ${type}`);
  }
};

const KEYED_MAP_SAMPLE: Record<string, unknown> = { string: "k", uint32: 7, int32: 7, int64: 7, uint64: 7, bool: false };

const sampleOf = (field: Field): unknown => {
  if (field.label === "map") {
    const [key, value] = field.type.slice("map<".length, -1).split(",");
    const mapKey = KEYED_MAP_SAMPLE[key!];
    if (mapKey === undefined) throw new Error(`no sample for map key ${key}`);
    return { [String(mapKey)]: zeroOf(value!, field.ref) };
  }
  const value = zeroOf(field.type, field.ref);
  return field.label === "repeated" ? [value] : value;
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

const isPresent = (field: Field, value: unknown): boolean => {
  if (value === undefined || value === null) return false;
  if (field.label === "map") return Object.keys(value as object).length === 1;
  const scalar = field.label === "repeated" ? (value as unknown[])[0] : value;
  if (field.label === "repeated" && (value as unknown[]).length !== 1) return false;
  if (field.type === "bytes") return scalar instanceof Uint8Array && scalar.length === 0;
  if (field.type === "message") return typeof scalar === "object" && scalar !== null;
  return scalar === zeroOf(field.type, field.ref);
};

beforeAll(() => {
  initWasmEngine();
});

describe("generated codec vs whatsapp.proto", () => {
  test("the surface manifest covers the whole schema", () => {
    expect(FIELDS.length).toBeGreaterThan(3000);
    expect(MESSAGES.length).toBeGreaterThan(600);
    expect(FIELDS.filter((field) => field.label === "map").length).toBeGreaterThan(0);
  });

  test("every declared message resolves to a codec", () => {
    const missing = MESSAGES.filter((message) => {
      try {
        encodeProto(message, {});
        return false;
      } catch {
        return true;
      }
    });
    expect(missing).toEqual([]);
  });

  test("every declared field is written, under its declared name, at its declared number", () => {
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

  test("every declared field reads back the zero value it was written with", () => {
    const failures: string[] = [];
    for (const field of FIELDS) {
      const key = keyOf(field);
      let read: unknown;
      try {
        const bytes = encodeProto(field.message, { [key]: sampleOf(field) });
        read = (decodeProto(field.message, bytes) as Record<string, unknown>)[key];
      } catch (error) {
        failures.push(`${field.message}.${field.name} (#${field.number}): ${error}`);
        continue;
      }
      if (!isPresent(field, read)) {
        failures.push(`${field.message}.${field.name} (#${field.number}): read back ${JSON.stringify(read)}`);
      }
    }
    expect(failures).toEqual([]);
  });

  test("only snake_case fields are written under a name other than the declared one", () => {
    const snakeCase = FIELDS.filter((field) => field.name.includes("_")).map(
      (field) => `${field.message}.${field.name}`,
    );
    expect(snakeCase.sort()).toEqual(Object.keys(JSON_NAME_KEYS).sort());

    for (const [path, jsonName] of Object.entries(JSON_NAME_KEYS)) {
      const field = FIELDS.find((candidate) => `${candidate.message}.${candidate.name}` === path)!;
      expect(field).toBeDefined();
      // Pinned, not endorsed: the declared name is silently dropped.
      expect(encodeProto(field.message, { [field.name]: sampleOf(field) }).length).toBe(0);
      expect(encodeProto(field.message, { [jsonName]: sampleOf(field) }).length).toBeGreaterThan(0);
    }
  });
});
