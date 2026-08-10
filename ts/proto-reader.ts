import { BinaryReader as BaseBinaryReader } from "@bufbuild/protobuf/wire";
import { Buffer } from "node:buffer";

// The generated codec imports both halves from here (see scripts/gen-ts-proto.ts),
// which is what puts the numeric input contract on every encode.
export { BinaryWriter } from "./proto-writer";

const PROTO_WORD_BITS = 32;
const PROTO_WORD_BASE = 2 ** PROTO_WORD_BITS;
const PROTO_VARINT_DATA_BITS = 7;
const PROTO_VARINT_CONTINUATION_BIT = 1 << PROTO_VARINT_DATA_BITS;
const PROTO_VARINT_DATA_MASK = PROTO_VARINT_CONTINUATION_BIT - 1;
const UTF8_ENCODING = "utf8";
const MAX_SAFE_VARINT_SHIFT =
  Math.floor(Math.log2(Number.MAX_SAFE_INTEGER) / PROTO_VARINT_DATA_BITS) * PROTO_VARINT_DATA_BITS;

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
  private readonly utf8Buffer: Buffer;

  constructor(buf: Uint8Array) {
    super(buf);
    // Buffer.from(ArrayBuffer, offset, length) is a view, not a copy. Keeping
    // one per reader lets every ordinary string decode use byte offsets
    // directly instead of allocating a temporary Uint8Array subarray.
    this.utf8Buffer = Buffer.from(buf.buffer, buf.byteOffset, buf.byteLength);
  }

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
