/**
 * `npm pack --dry-run --json` envelope shapes, pinned without running npm.
 *
 * npm 12 emits one record keyed by package name (captured from npm 12.0.2
 * output); older npm emitted a single-element array. Both must parse to the
 * same measurement, while zero records, several records, missing fields and
 * non-JSON fail closed instead of letting a gate succeed on undefined/NaN.
 *
 * Run: bun test tests/pack-envelope.test.ts
 */

import { describe, test, expect } from "bun:test";
import { parsePackOutput } from "../scripts/pack";

// Minimal record in the npm 12 keyed-object shape: the fields the parser
// reads, with the envelope that broke the old array destructuring.
const KEYED = JSON.stringify({
  "@oxidezap/whatsapp-rust-bridge": {
    id: "@oxidezap/whatsapp-rust-bridge@0.20.0",
    name: "@oxidezap/whatsapp-rust-bridge",
    version: "0.20.0",
    size: 2540976,
    unpackedSize: 8113888,
    shasum: "75a5bc2d24e6bb550dfb4b6961709cc260752d23",
    filename: "oxidezap-whatsapp-rust-bridge-0.20.0.tgz",
    files: [
      { path: "dist/index.js", size: 1200000, mode: 420 },
      { path: "dist/index.d.ts", size: 1514, mode: 420 },
    ],
  },
});

const LEGACY = JSON.stringify([
  {
    id: "@oxidezap/whatsapp-rust-bridge@0.20.0",
    name: "@oxidezap/whatsapp-rust-bridge",
    files: [{ path: "dist/index.js", size: 1200000 }],
    unpackedSize: 8113888,
  },
]);

describe("npm pack envelope parsing", () => {
  test("the keyed-object shape parses to files and unpackedSize", () => {
    const packed = parsePackOutput(KEYED);
    expect(packed.unpackedSize).toBe(8113888);
    expect(packed.files.map((f) => f.path)).toEqual([
      "dist/index.js",
      "dist/index.d.ts",
    ]);
  });

  test("the legacy single-element array shape parses identically", () => {
    const packed = parsePackOutput(LEGACY);
    expect(packed.unpackedSize).toBe(8113888);
    expect(packed.files).toEqual([{ path: "dist/index.js", size: 1200000 }]);
  });

  test.each([
    ["empty object", "{}"],
    ["empty array", "[]"],
    ["two records", JSON.stringify({ a: { files: [], unpackedSize: 1 }, b: { files: [], unpackedSize: 2 } })],
    ["missing files", JSON.stringify({ pkg: { unpackedSize: 1 } })],
    ["missing unpackedSize", JSON.stringify({ pkg: { files: [] } })],
    ["NaN unpackedSize", '{"pkg": {"files": [], "unpackedSize": null}}'],
    ["malformed entry", JSON.stringify({ pkg: { files: [{ path: 7 }], unpackedSize: 1 } })],
    ["not JSON", "not json at all"],
    ["JSON null", "null"],
  ])("malformed envelope fails closed: %s", (_name, raw) => {
    expect(() => parsePackOutput(raw)).toThrow(/pack: /);
  });
});
