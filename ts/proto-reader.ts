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

const int64ToWire = (value: Int64 | bigint | string): number | bigint | string =>
  typeof value === "object" ? longToBigInt(value) : value;

/**
 * BinaryWriter that accepts back what the reader below produces. Buf's own
 * 64-bit setters take `number | bigint | string`, so a decoded `Long` would
 * reach `BigInt(object)` and throw.
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

  override int64(value: Int64 | bigint | string): this {
    return super.int64(int64ToWire(value));
  }

  override uint64(value: Int64 | bigint | string): this {
    return super.uint64(int64ToWire(value));
  }

  override sint64(value: Int64 | bigint | string): this {
    return super.sint64(int64ToWire(value));
  }

  override fixed64(value: Int64 | bigint | string): this {
    return super.fixed64(int64ToWire(value));
  }

  override sfixed64(value: Int64 | bigint | string): this {
    return super.sfixed64(int64ToWire(value));
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
