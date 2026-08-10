import {
  BinaryReader as BaseBinaryReader,
  BinaryWriter as BaseBinaryWriter,
} from "@bufbuild/protobuf/wire";
import { Buffer } from "node:buffer";

const PROTO_WORD_BITS = 32;
const PROTO_WORD_BASE = 2 ** PROTO_WORD_BITS;
const PROTO_VARINT_DATA_BITS = 7;
const PROTO_VARINT_CONTINUATION_BIT = 1 << PROTO_VARINT_DATA_BITS;
const PROTO_VARINT_DATA_MASK = PROTO_VARINT_CONTINUATION_BIT - 1;
const UTF8_ENCODING = "utf8";
const REPLACEMENT_CHARACTER = "\uFFFD";
const SURROGATE_MIN = 0xd800;
const HIGH_SURROGATE_MAX = 0xdbff;
const SURROGATE_MAX = 0xdfff;
const CODE_UNIT_HEX_DIGITS = 4;
const MAX_SAFE_VARINT_SHIFT =
  Math.floor(Math.log2(Number.MAX_SAFE_INTEGER) / PROTO_VARINT_DATA_BITS) * PROTO_VARINT_DATA_BITS;

/** Index of the first UTF-16 code unit with no partner, or -1 when well-formed. */
export function unpairedSurrogateIndex(value: string): number {
  for (let index = 0; index < value.length; index++) {
    const unit = value.charCodeAt(index);
    if (unit < SURROGATE_MIN || unit > SURROGATE_MAX) continue;
    if (unit > HIGH_SURROGATE_MAX) return index;
    const next = value.charCodeAt(index + 1);
    if (Number.isNaN(next) || next <= HIGH_SURROGATE_MAX || next > SURROGATE_MAX) return index;
    index++;
  }
  return -1;
}

/**
 * A `string` field is UTF-8 on the wire and an unpaired UTF-16 surrogate has no
 * UTF-8 form, so there is nothing faithful to write. `path` is filled in by
 * `encodeProto`, which is the only layer that knows the field names.
 */
export class UnpairedSurrogateError extends RangeError {
  constructor(
    readonly index: number,
    readonly codeUnit: number,
    readonly path?: string,
  ) {
    super(
      `unpaired surrogate U+${codeUnit.toString(16).toUpperCase().padStart(CODE_UNIT_HEX_DIGITS, "0")}` +
        ` at index ${index} in ${path === undefined ? "a protobuf string field" : `protobuf field ${path}`}`,
    );
    this.name = "UnpairedSurrogateError";
  }
}

/**
 * protobufjs-style 64-bit value, `high * 2^32 + low`, with `low` unsigned and
 * `high` the signed 32-bit field protobufjs uses. Same shape `camel_serializer.rs`
 * emits on the event path, and `JSON.stringify`-safe where a BigInt is not.
 */
export interface Long {
  low: number;
  high: number;
  unsigned: boolean;
}

/**
 * What a 64-bit field crosses as: a plain `number` while the value is exact as
 * a double, a `Long` beyond that. Rejecting the wide value instead — which is
 * what ts-proto's generated `longToNumber()` does — fails the whole message
 * over one field, and truncating it to a double would lose the value silently.
 */
export type Int64 = number | Long;

/** Everything a numeric field accepts. `Int64` is the decode-side subset. */
export type NumericInput = number | bigint | string | Long;

const toLong = (low: number, high: number, unsigned: boolean): Long => ({
  low: low >>> 0,
  high: high | 0,
  unsigned,
});

const unsignedWordsToInt64 = (low: number, high: number): Int64 => {
  const value = (high >>> 0) * PROTO_WORD_BASE + (low >>> 0);
  return value <= Number.MAX_SAFE_INTEGER ? value : toLong(low, high, true);
};

const signedWordsToInt64 = (low: number, high: number): Int64 => {
  const value = (high | 0) * PROTO_WORD_BASE + (low >>> 0);
  return value >= Number.MIN_SAFE_INTEGER && value <= Number.MAX_SAFE_INTEGER
    ? value
    : toLong(low, high, false);
};

export const longToBigInt = (value: Long): bigint => {
  const low = BigInt(value.low >>> 0);
  const high = value.unsigned ? BigInt(value.high >>> 0) : BigInt(value.high | 0);
  return high * BigInt(PROTO_WORD_BASE) + low;
};

/** Either half of a Long, as a signed or unsigned 32-bit word. */
const isWord = (value: unknown): boolean =>
  typeof value === "number" && Number.isInteger(value) && value >= -0x80000000 && value <= 0xffffffff;

/**
 * `unsigned` (a boolean) is the discriminant: it short-circuits virtually every
 * non-Long object in one comparison, and matches the guard in
 * `camel_serializer.rs`. A plain `{ low, high }` data object is NOT a Long, and
 * neither is one whose words are not words — `longToBigInt` would truncate
 * `low: 1.5` to `1` and write a value nobody sent.
 */
export const isLong = (value: unknown): value is Long =>
  typeof value === "object" &&
  value !== null &&
  typeof (value as Long).unsigned === "boolean" &&
  isWord((value as Long).low) &&
  isWord((value as Long).high);

/**
 * The numeric input contract, in one sentence: a numeric field takes a
 * `number`, a `bigint`, a `Long` the reader produced, or a string that parses
 * in full as a number — never `''`, `true`, `[]` or anything else JavaScript
 * would silently turn into a number — and the value must be one the declared
 * type can hold. See `docs/proto-numeric-input.md` for the per-type matrix.
 *
 * Buf's writer enforces part of that (`assertInt32`, `assertFloat32`) and none
 * of it on `double` and the 64-bit methods, so the same caller mistake threw in
 * one field and wrote a wrong value in its neighbour.
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
const SPELLS_NAN = /^\s*NaN\s*$/;

/** Narrow to the one JS number the floating-point writers take. */
function asNumber(value: unknown, type: string): number {
  if (isLong(value)) return Number(longToBigInt(value));
  const input = numericInput(value, type);
  if (typeof input === "number") return input;
  const parsed = Number(input);
  if (Number.isNaN(parsed)) {
    // Whitespace is tolerated everywhere else a string is read, including the
    // infinity check below; `' NaN '` names the same value as `'NaN'`.
    if (!SPELLS_NAN.test(String(input))) throw invalid(type, value);
  } else if (!Number.isFinite(parsed) && !SPELLS_INFINITY.test(String(input))) {
    // A finite input that only reaches infinity by overflowing the conversion
    // is a value the type cannot hold, not the infinity the caller asked for.
    throw invalid(type, value);
  }
  return parsed;
}

const DECIMAL_TEXT = /^\s*([+-]?)(\d*)(?:\.(\d*))?(?:[eE]([+-]?\d+))?\s*$/;
/** No protobuf integer type reaches twenty digits; the cap bounds the padding. */
const MAX_INTEGRAL_DIGITS = 40;

/**
 * The exact integer a decimal string names, or `undefined` when it names
 * something else. Reading the digits rather than `Number(text)` is what keeps
 * `'1.0000000000000000001'` and `'1e-400'` from arriving as `1` and `0`, and
 * what lets `'9007199254740992.0'` through: it is exact, it just is not a
 * *safe* integer.
 */
function exactIntegerFromText(text: string): bigint | undefined {
  const parsed = DECIMAL_TEXT.exec(text);
  if (!parsed) return undefined;
  const [, sign, whole = "", fraction = "", exponent] = parsed;
  if (whole === "" && fraction === "") return undefined;

  const written = whole + fraction;
  // Zero is zero at any exponent, and answering it here keeps `'0e100'` from
  // being turned away by a cap that only exists to bound the padding below.
  if (/^0*$/.test(written)) return 0n;

  // Leading zeros are notation, not magnitude, and dropping one moves the
  // decimal point with it. Stripping them across the whole significand — not
  // just the part before the point — is what keeps the cap about how large the
  // value is rather than how it was spelled.
  const leadingZeros = written.length - written.replace(/^0+/, "").length;
  const digits = written.slice(leadingZeros);
  const pointAt = whole.length + (exponent ? Number(exponent) : 0) - leadingZeros;
  if (pointAt <= 0 || pointAt > MAX_INTEGRAL_DIGITS) return undefined;

  let integral: string;
  if (pointAt >= digits.length) {
    integral = digits + "0".repeat(pointAt - digits.length);
  } else {
    if (!/^0*$/.test(digits.slice(pointAt))) return undefined;
    integral = digits.slice(0, pointAt);
  }
  const value = BigInt(integral);
  return sign === "-" ? -value : value;
}

/** An integer field's input, exact: never rounded on the way in. */
function asInteger(value: unknown, type: string): number | bigint {
  // A Long is what the reader hands back for a value past 2^53, so it is a
  // number here — but only a real one: `typeof value === "object"` alone would
  // read `{}` and `[]` as zero.
  if (isLong(value)) return longToBigInt(value);
  const input = numericInput(value, type);
  if (typeof input === "bigint") return input;
  if (typeof input === "number") {
    // Integrality is checked here so the message reads like every other
    // rejection; BigInt(1.5) throws with an engine-specific wording.
    if (!Number.isInteger(input)) throw invalid(type, value);
    return input;
  }
  try {
    // Integer-literal syntax first: it keeps plain digits past 2^53 exact and
    // covers the 0x/0o/0b forms `Number` also accepts.
    return BigInt(input);
  } catch {
    const exact = exactIntegerFromText(input);
    if (exact === undefined) throw invalid(type, value);
    return exact;
  }
}

/** The 32-bit writers take a JS number; the width check downstream is Buf's. */
function asInteger32(value: unknown, type: string): number {
  const exact = asInteger(value, type);
  return typeof exact === "bigint" ? Number(exact) : exact;
}

/**
 * BinaryWriter that accepts back what the reader below produces — Buf's own
 * 64-bit setters take `number | bigint | string`, so a decoded `Long` would
 * reach `BigInt(object)` and throw — and that holds every numeric field to the
 * input contract above.
 *
 * It also refuses an unpaired surrogate rather than letting `TextEncoder`
 * substitute U+FFFD for it, which would send the server text the caller never
 * wrote. The value came from the caller, so the caller is the one who can fix it.
 */
export class BinaryWriter extends BaseBinaryWriter {
  override string(value: string): this {
    if (!value.isWellFormed()) {
      const index = unpairedSurrogateIndex(value);
      throw new UnpairedSurrogateError(index, value.charCodeAt(index));
    }
    return super.string(value);
  }

  override uint32(value: NumericInput): this {
    return super.uint32(typeof value === "number" ? value : asInteger32(value, "uint32"));
  }

  override int32(value: NumericInput): this {
    return super.int32(typeof value === "number" ? value : asInteger32(value, "int32"));
  }

  override sint32(value: NumericInput): this {
    return super.sint32(typeof value === "number" ? value : asInteger32(value, "sint32"));
  }

  override fixed32(value: NumericInput): this {
    return super.fixed32(typeof value === "number" ? value : asInteger32(value, "fixed32"));
  }

  override sfixed32(value: NumericInput): this {
    return super.sfixed32(typeof value === "number" ? value : asInteger32(value, "sfixed32"));
  }

  override float(value: NumericInput): this {
    return super.float(typeof value === "number" ? value : asNumber(value, "float"));
  }

  override double(value: NumericInput): this {
    return super.double(typeof value === "number" ? value : asNumber(value, "double"));
  }

  override int64(value: NumericInput): this {
    return super.int64(asInteger(value, "int64"));
  }

  override uint64(value: NumericInput): this {
    return super.uint64(asInteger(value, "uint64"));
  }

  override sint64(value: NumericInput): this {
    return super.sint64(asInteger(value, "sint64"));
  }

  override fixed64(value: NumericInput): this {
    return super.fixed64(asInteger(value, "fixed64"));
  }

  override sfixed64(value: NumericInput): this {
    return super.sfixed64(asInteger(value, "sfixed64"));
  }
}

/**
 * BinaryReader specialized for ts-proto's 64-bit output contract. Buf's own
 * 64-bit methods materialize a BigInt and then a decimal string; the `*Value()`
 * methods below avoid both intermediates, while the inherited methods retain
 * Buf's public BigInt/string behavior for direct users.
 */
export class BinaryReader extends BaseBinaryReader {
  protected readonly utf8Buffer: Buffer;

  constructor(buf: Uint8Array) {
    super(buf);
    // Buffer.from(ArrayBuffer, offset, length) is a view, not a copy. Keeping
    // one per reader lets every ordinary string decode use byte offsets
    // directly instead of allocating a temporary Uint8Array subarray.
    this.utf8Buffer = Buffer.from(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  /**
   * Bytes come from a peer this side does not control, so a malformed one
   * substitutes U+FFFD rather than costing the whole message. `strict` reaches
   * Buf's throwing decoder; the generated codecs never pass it.
   */
  override string(strict?: boolean): string {
    if (strict) return super.string(true);

    const byteLength = this.uint32();
    const start = this.pos;
    this.pos += byteLength;
    this.assertBounds();
    return this.utf8Buffer.toString(UTF8_ENCODING, start, this.pos);
  }

  override bool(): boolean {
    const byte = this.buf[this.pos++]!;
    if ((byte & PROTO_VARINT_CONTINUATION_BIT) === 0) {
      this.assertBounds();
      return byte !== 0;
    }

    // Non-canonical or deliberately wide bool values remain valid protobuf
    // varints. Rewind and retain the base reader's complete 64-bit semantics.
    this.pos--;
    const [low, high] = this.varint64();
    return low !== 0 || high !== 0;
  }

  /**
   * Decode the overwhelmingly common positive/safe varint without allocating
   * Buf's `[low, high]` tuple. Returning `undefined` rewinds for the full signed
   * two-word path (negative int64 values always use ten wire bytes, and eight
   * data bytes can carry more than a double holds exactly).
   */
  private positiveSafeVarint(): number | undefined {
    const start = this.pos;
    let value = 0;
    let factor = 1;
    for (let shift = 0; shift <= MAX_SAFE_VARINT_SHIFT; shift += PROTO_VARINT_DATA_BITS) {
      const byte = this.buf[this.pos++]!;
      value += (byte & PROTO_VARINT_DATA_MASK) * factor;
      if ((byte & PROTO_VARINT_CONTINUATION_BIT) === 0) {
        this.assertBounds();
        if (value <= Number.MAX_SAFE_INTEGER) return value;
        break;
      }
      factor *= PROTO_VARINT_CONTINUATION_BIT;
    }
    this.pos = start;
    return undefined;
  }

  uint64Value(): Int64 {
    const fast = this.positiveSafeVarint();
    if (fast !== undefined) return fast;
    const [low, high] = this.varint64();
    return unsignedWordsToInt64(low, high);
  }

  int64Value(): Int64 {
    const fast = this.positiveSafeVarint();
    if (fast !== undefined) return fast;
    const [low, high] = this.varint64();
    return signedWordsToInt64(low, high);
  }

  sint64Value(): Int64 {
    let [low, high] = this.varint64();
    const sign = -(low & 1);
    low = ((low >>> 1) | ((high & 1) << (PROTO_WORD_BITS - 1))) ^ sign;
    high = (high >>> 1) ^ sign;
    return signedWordsToInt64(low, high);
  }

  fixed64Value(): Int64 {
    return unsignedWordsToInt64(this.sfixed32(), this.sfixed32());
  }

  sfixed64Value(): Int64 {
    return signedWordsToInt64(this.sfixed32(), this.sfixed32());
  }
}

/**
 * Counts the substitutions `BinaryReader` makes silently. It is a separate
 * class so the ordinary decode path carries neither the flag nor the branch:
 * a consumer who never asks for the count pays nothing for it existing.
 */
export class InvalidUtf8CountingReader extends BinaryReader {
  invalidUtf8Fields = 0;

  override string(strict?: boolean): string {
    if (strict) return super.string(true);

    const byteLength = this.uint32();
    const start = this.pos;
    this.pos += byteLength;
    this.assertBounds();
    const text = this.utf8Buffer.toString(UTF8_ENCODING, start, this.pos);
    if (text.includes(REPLACEMENT_CHARACTER) && !this.decodedExactly(text, start, this.pos)) {
      this.invalidUtf8Fields++;
    }
    return text;
  }

  /** A U+FFFD the peer actually sent re-encodes to the bytes it arrived as; a substituted one does not. */
  private decodedExactly(text: string, start: number, end: number): boolean {
    const reencoded = Buffer.from(text, UTF8_ENCODING);
    return this.utf8Buffer.compare(reencoded, 0, reencoded.length, start, end) === 0;
  }
}
