import { BinaryWriter as BaseBinaryWriter } from "@bufbuild/protobuf/wire";

/**
 * The numeric input contract for `encodeProto`, in one sentence: a numeric
 * field takes a `number`, a `bigint`, or a string that parses in full as a
 * number — never `''`, `true`, `[]` or anything else JavaScript would silently
 * turn into a number — and the value must be one the declared type can hold.
 *
 * Buf's writer enforces part of that (`assertInt32`, `assertFloat32`) and
 * nothing of it on `double` and the 64-bit methods, so the same caller mistake
 * threw in one field and wrote a wrong value in its neighbour. See
 * `docs/proto-numeric-input.md` for the full per-type matrix.
 */

const describe = (value: unknown): string =>
  typeof value === "string"
    ? JSON.stringify(value)
    : typeof value === "number" || typeof value === "bigint"
      ? String(value)
      : typeof value;

const invalid = (type: string, value: unknown): Error =>
  new Error(`invalid ${type}: ${describe(value)}`);

/** A number, a bigint, or a string with something in it. Nothing else is a number. */
function numericInput(value: unknown, type: string): number | bigint | string {
  const kind = typeof value;
  if (kind === "number" || kind === "bigint") return value as number | bigint;
  if (kind === "string" && (value as string).trim() !== "") return value as string;
  throw invalid(type, value);
}

const SPELLS_INFINITY = /^\s*[+-]?Infinity\s*$/;

/** Narrow to the one JS number the 32-bit and floating-point writers take. */
function asNumber(value: unknown, type: string): number {
  const input = numericInput(value, type);
  if (typeof input === "number") return input;
  const parsed = Number(input);
  if (Number.isNaN(parsed)) {
    if (input !== "NaN") throw invalid(type, value);
  } else if (!Number.isFinite(parsed) && !SPELLS_INFINITY.test(String(input))) {
    // A finite input that only reaches infinity by overflowing the conversion
    // is a value the type cannot hold, not the infinity the caller asked for.
    throw invalid(type, value);
  }
  return parsed;
}

/** A 64-bit string goes to BigInt, not Number, so a value past 2^53 stays exact. */
function asInt64(value: unknown, type: string): number | bigint {
  const input = numericInput(value, type);
  if (typeof input === "bigint") return input;
  if (typeof input === "number") {
    // Integrality is checked here so the message reads like every other
    // rejection; BigInt(1.5) throws with an engine-specific wording.
    if (!Number.isInteger(input)) throw invalid(type, value);
    return input;
  }
  try {
    return BigInt(input);
  } catch {
    // BigInt reads integer literals only, so `'1e2'` and `'12.0'` — integers a
    // 32-bit field takes — would otherwise fail on width alone. Number reads
    // them, and only a safe integer is kept: past 2^53 the digits are the value.
    const parsed = Number(input);
    if (!Number.isSafeInteger(parsed)) throw invalid(type, value);
    return parsed;
  }
}

/** BinaryWriter with the numeric input contract applied to every numeric field. */
export class BinaryWriter extends BaseBinaryWriter {
  override uint32(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "uint32");
    return super.uint32(value);
  }

  override int32(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "int32");
    return super.int32(value);
  }

  override sint32(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "sint32");
    return super.sint32(value);
  }

  override fixed32(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "fixed32");
    return super.fixed32(value);
  }

  override sfixed32(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "sfixed32");
    return super.sfixed32(value);
  }

  override float(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "float");
    return super.float(value);
  }

  override double(value: number): this {
    if (typeof value !== "number") value = asNumber(value, "double");
    return super.double(value);
  }

  override int64(value: string | number | bigint): this {
    return super.int64(asInt64(value, "int64"));
  }

  override uint64(value: string | number | bigint): this {
    return super.uint64(asInt64(value, "uint64"));
  }

  override sint64(value: string | number | bigint): this {
    return super.sint64(asInt64(value, "sint64"));
  }

  override fixed64(value: string | number | bigint): this {
    return super.fixed64(asInt64(value, "fixed64"));
  }

  override sfixed64(value: string | number | bigint): this {
    return super.sfixed64(asInt64(value, "sfixed64"));
  }
}
