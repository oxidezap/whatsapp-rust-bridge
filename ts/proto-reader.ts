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
function unpairedSurrogateIndex(value: string): number {
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
 * `TextEncoder` answers an unpaired surrogate by substituting U+FFFD, which
 * would send the server text the caller never wrote. Refuse instead: the value
 * came from the caller, so the caller is the one who can fix it.
 */
export class BinaryWriter extends BaseBinaryWriter {
  override string(value: string): this {
    if (!value.isWellFormed()) {
      const index = unpairedSurrogateIndex(value);
      throw new UnpairedSurrogateError(index, value.charCodeAt(index));
    }
    return super.string(value);
  }
}

/**
 * ts-proto is configured to expose every 64-bit field as a JavaScript number.
 * Its generated `longToNumber()` rejects values outside the safe-integer range,
 * but Buf's default reader first materializes a BigInt and then a decimal string
 * for that conversion. Generated codecs call the `*Number()` methods below,
 * avoiding both intermediates while the inherited methods retain Buf's public
 * BigInt/string behavior for direct users.
 */
const assertSafeInteger = (value: number): number => {
  if (value > Number.MAX_SAFE_INTEGER) {
    throw new Error("Value is larger than Number.MAX_SAFE_INTEGER");
  }
  if (value < Number.MIN_SAFE_INTEGER) {
    throw new Error("Value is smaller than Number.MIN_SAFE_INTEGER");
  }
  return value;
};

const unsignedWordsToNumber = (low: number, high: number): number =>
  assertSafeInteger((high >>> 0) * PROTO_WORD_BASE + (low >>> 0));

const signedWordsToNumber = (low: number, high: number): number =>
  assertSafeInteger((high | 0) * PROTO_WORD_BASE + (low >>> 0));

/** BinaryReader specialized for ts-proto's safe-number output contract. */
export class BinaryReader extends BaseBinaryReader {
  /**
   * `string` fields whose bytes were not valid UTF-8 and were handed back with
   * U+FFFD in place of them. Only counted when the reader was built with
   * `trackInvalidUtf8`; otherwise it stays at zero.
   */
  invalidUtf8Fields = 0;

  private readonly utf8Buffer: Buffer;
  private readonly trackInvalidUtf8: boolean;

  constructor(buf: Uint8Array, trackInvalidUtf8 = false) {
    super(buf);
    // Buffer.from(ArrayBuffer, offset, length) is a view, not a copy. Keeping
    // one per reader lets every ordinary string decode use byte offsets
    // directly instead of allocating a temporary Uint8Array subarray.
    this.utf8Buffer = Buffer.from(buf.buffer, buf.byteOffset, buf.byteLength);
    this.trackInvalidUtf8 = trackInvalidUtf8;
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
    const text = this.utf8Buffer.toString(UTF8_ENCODING, start, this.pos);
    if (
      this.trackInvalidUtf8 &&
      text.includes(REPLACEMENT_CHARACTER) &&
      !this.decodedExactly(text, start, this.pos)
    ) {
      this.invalidUtf8Fields++;
    }
    return text;
  }

  /** A U+FFFD the peer actually sent re-encodes to the same bytes; a substituted one does not. */
  private decodedExactly(text: string, start: number, end: number): boolean {
    const reencoded = Buffer.from(text, UTF8_ENCODING);
    return this.utf8Buffer.compare(reencoded, 0, reencoded.length, start, end) === 0;
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
   * two-word path (negative int64 values always use ten wire bytes).
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
        return assertSafeInteger(value);
      }
      factor *= PROTO_VARINT_CONTINUATION_BIT;
    }
    this.pos = start;
    return undefined;
  }

  uint64Number(): number {
    const fast = this.positiveSafeVarint();
    if (fast !== undefined) return fast;
    const [low, high] = this.varint64();
    return unsignedWordsToNumber(low, high);
  }

  int64Number(): number {
    const fast = this.positiveSafeVarint();
    if (fast !== undefined) return fast;
    const [low, high] = this.varint64();
    return signedWordsToNumber(low, high);
  }

  sint64Number(): number {
    let [low, high] = this.varint64();
    const sign = -(low & 1);
    low = ((low >>> 1) | ((high & 1) << (PROTO_WORD_BITS - 1))) ^ sign;
    high = (high >>> 1) ^ sign;
    return signedWordsToNumber(low, high);
  }

  fixed64Number(): number {
    return unsignedWordsToNumber(this.sfixed32(), this.sfixed32());
  }

  sfixed64Number(): number {
    return signedWordsToNumber(this.sfixed32(), this.sfixed32());
  }
}
