/**
 * The two artifacts expose the same operations, so the same inputs must
 * produce the same outputs. Anything that drifts on one side only shows up
 * here rather than in a consumer's lap.
 *
 * This checks that both artifacts agree — not that either is correct. The
 * per-operation tests own correctness.
 */
import { describe, expect, test } from "bun:test";
import * as bindgen from "../dist/index.js";
import * as boltffi from "../dist/boltffi/pkg/node.js";

/**
 * One case per operation both backends expose. `args` is rebuilt per backend
 * so neither can observe a buffer the other already handed across.
 */
const CASES: Array<{
  name: keyof typeof boltffi & string;
  args: () => unknown[];
}> = [
  { name: "md5", args: () => [new Uint8Array([1, 2, 3, 4])] },
  { name: "md5", args: () => [new Uint8Array()] },
  {
    name: "hkdf",
    args: () => [
      new Uint8Array(32).fill(7),
      64,
      { salt: new Uint8Array(16).fill(1), info: "wa" },
    ],
  },
  // BoltFFI records carry every declared field; wasm-bindgen's `HkdfInfo` is
  // `#[serde(default)]` and tolerates omission. An explicit null is what both
  // accept, so that is what parity is measured on.
  { name: "hkdf", args: () => [new Uint8Array(32).fill(9), 16, { salt: null, info: null }] },
  {
    name: "getPublicFromPrivateKey",
    args: () => [KNOWN_PRIVATE_KEY],
  },
  {
    name: "calculateAgreement",
    args: () => [KNOWN_PUBLIC_KEY, KNOWN_PRIVATE_KEY],
  },
  {
    name: "verifySignature",
    args: () => [KNOWN_PUBLIC_KEY, new Uint8Array([1, 2, 3]), new Uint8Array(64)],
  },
  // The limit is passed explicitly: wasm-bindgen declares it optional, BoltFFI
  // declares it `number | null`, so only an explicit value is common to both.
  { name: "inflateZlib", args: () => [ZLIB_HELLO, null] },
  { name: "inflateZlib", args: () => [ZLIB_HELLO, 1024] },
  { name: "inflateZlib", args: () => [new Uint8Array([1, 2, 3]), null] },
];

/**
 * Operations whose result is not a pure function of the arguments, so the two
 * backends cannot be compared value-for-value. Each still gets a shape check
 * below, and each still has its own test on both sides.
 */
const NONDETERMINISTIC = new Set<string>([]);

/**
 * Exposed by wasm-bindgen only. Both draw randomness, and this repository
 * builds `getrandom` with the `wasm_js` backend, whose `__wbg_getRandomValues_*`
 * import the BoltFFI runtime cannot satisfy. Asserted below so the gap stays
 * visible instead of looking like an oversight.
 */
const BINDGEN_ONLY = new Set(["generateKeyPair", "calculateSignature"]);

/**
 * Operations that need inputs this suite cannot construct without a live
 * session (a real message secret and matching ciphertext).
 */
const NEEDS_SESSION_STATE = new Set([
  "decryptPollVotePayload",
  "decryptEventResponsePayload",
]);

// Curve25519 private key with the clamping the core applies, so the derived
// public key is stable across runs.
const KNOWN_PRIVATE_KEY = new Uint8Array(32).fill(0x42);
KNOWN_PRIVATE_KEY[0] &= 248;
KNOWN_PRIVATE_KEY[31] &= 127;
KNOWN_PRIVATE_KEY[31] |= 64;

const KNOWN_PUBLIC_KEY = bindgen.getPublicFromPrivateKey(KNOWN_PRIVATE_KEY);

// zlib stream for "hello", built by the wasm-bindgen side's own inflate being
// the thing under test would be circular, so this is a literal.
const ZLIB_HELLO = new Uint8Array([
  0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
]);

/**
 * Both backends carry the core's rendered message, in different slots:
 * wasm-bindgen throws it as the `Error` message, BoltFFI throws a typed
 * exception whose payload holds it. Parity is on the text, not the carrier.
 */
function errorText(error: unknown): string {
  const payload = (error as { value?: { value0?: unknown } })?.value;
  if (payload && typeof payload.value0 === "string") return payload.value0;
  return String((error as Error)?.message ?? error);
}

/** Normalise so a `Uint8Array` and a plain array compare structurally. */
function normalise(value: unknown): unknown {
  if (value instanceof Uint8Array) return Array.from(value);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, inner]) => [key, normalise(inner)]),
    );
  }
  return value;
}

function callBoth(name: string, args: () => unknown[]) {
  const run = (module: Record<string, unknown>) => {
    const fn = module[name];
    if (typeof fn !== "function") {
      throw new Error(`backend does not export ${name}`);
    }
    try {
      return { ok: true as const, value: normalise(fn(...args())) };
    } catch (error) {
      return { ok: false as const, message: errorText(error) };
    }
  };
  return {
    bindgen: run(bindgen as unknown as Record<string, unknown>),
    boltffi: run(boltffi as unknown as Record<string, unknown>),
  };
}

describe("backend equivalence", () => {
  for (const { name, args } of CASES) {
    test(`${name} agrees across backends`, () => {
      const { bindgen: a, boltffi: b } = callBoth(name, args);
      expect(b.ok).toBe(a.ok);
      if (a.ok && b.ok) {
        expect(b.value).toEqual(a.value);
      } else if (!a.ok && !b.ok) {
        // Both backends must reject, and both must say why. The exact prose
        // comes from the core, so it has to match too.
        expect(b.message).toBe(a.message);
      }
    });
  }

  test("operations exposed by only one backend are the documented ones", () => {
    for (const name of BINDGEN_ONLY) {
      expect(typeof (bindgen as Record<string, unknown>)[name]).toBe("function");
      expect(typeof (boltffi as Record<string, unknown>)[name]).not.toBe("function");
    }
  });

  /**
   * The guard that makes this suite self-maintaining: an operation added to
   * the BoltFFI backend without a case here fails instead of passing quietly.
   */
  test("every BoltFFI export is covered by a case", () => {
    // Module plumbing, not operations: the loader's default export and the
    // generated error class.
    const NOT_AN_OPERATION = /^(default|init|initialized)$|Exception$/;
    const exported = Object.entries(boltffi)
      .filter(([name, value]) => typeof value === "function" && !NOT_AN_OPERATION.test(name))
      .map(([name]) => name);

    const covered = new Set<string>([
      ...CASES.map((entry) => entry.name),
      ...NONDETERMINISTIC,
      ...NEEDS_SESSION_STATE,
    ]);

    const uncovered = exported.filter((name) => !covered.has(name)).sort();
    expect(uncovered).toEqual([]);
  });

  /** The mirror of the guard above: a case naming an operation neither backend has. */
  test("every covered operation exists on both backends", () => {
    const names = [...new Set(CASES.map((entry) => entry.name))].concat([
      ...NONDETERMINISTIC,
      ...NEEDS_SESSION_STATE,
    ]);
    const missing = names
      .filter(
        (name) =>
          typeof (bindgen as Record<string, unknown>)[name] !== "function" ||
          typeof (boltffi as Record<string, unknown>)[name] !== "function",
      )
      .sort();
    expect(missing).toEqual([]);
  });
});
