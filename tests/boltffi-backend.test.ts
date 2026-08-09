/**
 * Every operation the BoltFFI artifact exposes, exercised through the BoltFFI
 * artifact. The wasm-bindgen suite covering the same function proves nothing
 * about this one: it is a different generator over a different wire.
 *
 * What these prove is traversal and return shape. The equivalence suite proves
 * the two backends agree; the core owns whether the answer is right.
 */
import { describe, expect, test } from "bun:test";
import { boltffi as loaded, boltffiAvailable } from "./boltffi-artifact";

const boltffi = loaded as unknown as typeof import("../dist/boltffi/pkg/node.js");

const CURVE_KEY_BYTES = 32;
const SIGNATURE_BYTES = 64;
const MD5_BYTES = 16;

/**
 * Clamped curve private keys. This backend does not expose `generateKeyPair`
 * (see the note in `crates/bridge-boltffi/src/lib.rs`), so the key material is
 * fixed here rather than drawn at runtime.
 */
const PRIVATE_KEY_A = clamp(new Uint8Array(32).fill(0x42));
const PRIVATE_KEY_B = clamp(new Uint8Array(32).fill(0x17));

function clamp(key: Uint8Array): Uint8Array {
  key[0] &= 248;
  key[31] &= 127;
  key[31] |= 64;
  return key;
}

// zlib stream for "hello".
const ZLIB_HELLO = new Uint8Array([
  0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
]);

describe.skipIf(!boltffiAvailable)("BoltFFI artifact", () => {
  test("loads and exposes the utility surface", () => {
    expect(typeof boltffi.md5).toBe("function");
    expect(typeof boltffi.inflateZlib).toBe("function");
  });

  test("md5 returns a 16-byte digest", () => {
    const digest = boltffi.md5(new Uint8Array([1, 2, 3]));
    expect(digest).toBeInstanceOf(Uint8Array);
    expect(digest.length).toBe(MD5_BYTES);
  });

  test("md5 accepts an empty input", () => {
    expect(boltffi.md5(new Uint8Array()).length).toBe(MD5_BYTES);
  });

  test("hkdf expands to the requested length", () => {
    const out = boltffi.hkdf(new Uint8Array(32).fill(7), 64, { salt: new Uint8Array(16), info: "wa" });
    expect(out.length).toBe(64);
  });

  // BoltFFI records carry every declared field, so an omitted `salt`/`info` is
  // not the same as a null one — the null form is what this backend accepts.
  test("hkdf accepts a null salt and info", () => {
    expect(
      boltffi.hkdf(new Uint8Array(32).fill(3), 32, { salt: null, info: null }).length,
    ).toBe(32);
  });

  test("getPublicFromPrivateKey derives a curve public key", () => {
    const pub = boltffi.getPublicFromPrivateKey(PRIVATE_KEY_A);
    expect(pub.length).toBe(CURVE_KEY_BYTES + 1);
  });

  test("calculateAgreement is symmetric across two key pairs", () => {
    const pubA = boltffi.getPublicFromPrivateKey(PRIVATE_KEY_A);
    const pubB = boltffi.getPublicFromPrivateKey(PRIVATE_KEY_B);
    expect(Array.from(boltffi.calculateAgreement(pubA, PRIVATE_KEY_B))).toEqual(
      Array.from(boltffi.calculateAgreement(pubB, PRIVATE_KEY_A)),
    );
  });

  // Signing is wasm-bindgen-only here (it draws randomness), so verification is
  // exercised against a signature this backend did not produce.
  test("verifySignature rejects a signature that does not match", () => {
    const pub = boltffi.getPublicFromPrivateKey(PRIVATE_KEY_A);
    expect(
      boltffi.verifySignature(pub, new Uint8Array([2]), new Uint8Array(SIGNATURE_BYTES)),
    ).toBe(false);
  });

  test("inflateZlib inflates a stream", () => {
    expect(Buffer.from(boltffi.inflateZlib(ZLIB_HELLO, null)).toString()).toBe("hello");
  });

  test("inflateZlib honours an explicit limit", () => {
    expect(Buffer.from(boltffi.inflateZlib(ZLIB_HELLO, 1024)).toString()).toBe("hello");
  });

  describe("errors cross as a typed, catchable throw", () => {
    test("a rejected limit names what was wrong", () => {
      let caught: unknown;
      try {
        boltffi.inflateZlib(ZLIB_HELLO, 0);
      } catch (error) {
        caught = error;
      }
      expect(caught).toBeInstanceOf(Error);
      // BoltFFI throws a typed exception; the core's rendered message is the
      // payload, not the `Error` message (which names the error type). Read it
      // defensively so a change in the wrapper shape names itself rather than
      // failing as an opaque TypeError.
      const payload = (caught as { value?: { value0?: unknown } })?.value;
      expect(typeof payload?.value0).toBe("string");
      expect(payload?.value0).toContain("maxOutputBytes");
    });

    test("input that is not a zlib stream throws rather than returning empty", () => {
      expect(() => boltffi.inflateZlib(new Uint8Array([1, 2, 3]), null)).toThrow();
    });

    test("a malformed private key throws", () => {
      expect(() => boltffi.getPublicFromPrivateKey(new Uint8Array([1, 2, 3]))).toThrow();
    });

    test("addon payload decryption throws on a bad secret rather than hanging", () => {
      expect(() =>
        boltffi.decryptPollVotePayload(
          new Uint8Array(32),
          new Uint8Array(12),
          new Uint8Array(32),
          "stanza",
          "1@s.whatsapp.net",
          "2@s.whatsapp.net",
        ),
      ).toThrow();
      expect(() =>
        boltffi.decryptEventResponsePayload(
          new Uint8Array(32),
          new Uint8Array(12),
          new Uint8Array(32),
          "stanza",
          "1@s.whatsapp.net",
          "2@s.whatsapp.net",
        ),
      ).toThrow();
    });
  });
});
