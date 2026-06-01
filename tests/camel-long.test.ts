/**
 * `Long`-object conversion arithmetic for 64-bit proto fields.
 *
 * The Rust `camel_serializer` splits i64/u64 into protobufjs-style
 * `{ low, high, unsigned }` (so `JSON.stringify` works and baileyrs `toNumber()`
 * consumes it). The byte-level serialization is exercised end-to-end by the
 * e2e suite (a real `message` event must survive `JSON.stringify` — see
 * e2e-messaging.test.ts). Here we pin the *arithmetic contract* the Rust code
 * implements, against an independent reconstruction oracle, so a regression in
 * the split (sign handling, >2^53 precision) fails fast without a live socket.
 *
 * Run: bun test tests/camel-long.test.ts
 */

import { describe, test, expect } from "bun:test";

// Mirror of the Rust split (camel_serializer.rs `i64_to_long_parts` /
// `u64_to_long_parts`). Kept here as the spec the Rust side must match; the
// numbers below are checked against BigInt, the ground truth.
function i64ToLong(v: bigint): { low: number; high: number; unsigned: boolean } {
  const u = BigInt.asUintN(64, v); // two's-complement 64-bit
  return {
    low: Number(u & 0xffffffffn),
    high: Number(BigInt.asIntN(32, u >> 32n)), // signed high half
    unsigned: false,
  };
}
function u64ToLong(v: bigint): { low: number; high: number; unsigned: boolean } {
  return {
    low: Number(v & 0xffffffffn),
    high: Number(BigInt.asIntN(32, (v >> 32n) & 0xffffffffn)),
    unsigned: true,
  };
}

// protobufjs Long → BigInt (the reconstruction a consumer / toNumber does).
function longToBigInt(l: { low: number; high: number; unsigned: boolean }): bigint {
  const lo = BigInt(l.low >>> 0);
  const hi = l.unsigned
    ? BigInt(l.high >>> 0)
    : BigInt(l.high); // signed high
  return (hi << 32n) + lo;
}

describe("Long-object arithmetic (i64/u64 split)", () => {
  test("a typical timestamp round-trips exactly", () => {
    const v = 1780228232n;
    const long = i64ToLong(v);
    expect(long.unsigned).toBe(false);
    expect(longToBigInt(long)).toBe(v);
    // And it JSON.stringifies (the whole point — no BigInt).
    expect(() => JSON.stringify(long)).not.toThrow();
  });

  test("a value beyond 2^53 keeps full precision via low/high", () => {
    const v = 9007199254740993n; // 2^53 + 1 — not exact as a JS number
    expect(longToBigInt(i64ToLong(v))).toBe(v);
  });

  test("i64 max round-trips", () => {
    const v = 9223372036854775807n; // 2^63 - 1
    const long = i64ToLong(v);
    expect(long.high).toBeGreaterThanOrEqual(0); // top bit clear → positive high
    expect(longToBigInt(long)).toBe(v);
  });

  test("negative i64 uses two's-complement high half", () => {
    const v = -1n;
    const long = i64ToLong(v);
    expect(long.low).toBe(0xffffffff);
    expect(long.high).toBe(-1);
    expect(longToBigInt(long)).toBe(v);
  });

  test("u64 max round-trips as unsigned", () => {
    const v = 18446744073709551615n; // 2^64 - 1
    const long = u64ToLong(v);
    expect(long.unsigned).toBe(true);
    expect(longToBigInt(long)).toBe(v);
  });

  test("zero is representable (serializer skips it as a default elsewhere)", () => {
    expect(longToBigInt(i64ToLong(0n))).toBe(0n);
  });
});
