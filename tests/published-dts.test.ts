/**
 * The declarations the package publishes have to typecheck on their own.
 *
 * Every consumer tsconfig this repository knows about sets `skipLibCheck: true`
 * — the TypeScript default in practically every template — which turns an
 * unresolved name inside a `.d.ts` into a silent `any`. That is how nineteen
 * generated declarations came to name types the file never declares: nothing
 * was positioned to see it. This checks `dist/` the way a consumer who turned
 * `skipLibCheck` off would, so the twentieth fails here instead of shipping.
 *
 * Run: bun run build && bun test tests/published-dts.test.ts
 */

import { test, expect } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as ts from "../scripts/typescript-api";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const ENTRY = join(ROOT, "dist", "index.d.ts");
const CONSUMER = join(ROOT, "tests", "fixtures", "published-dts-consumer.ts");
// The consumer imports the package by its own name (`@oxidezap/.../wasm`)
// for the `./wasm` subpath assertion, which only resolves through the
// `exports` map — no nested package.json may shadow the root one.

/**
 * The lib set a consumer would have: `dom` for the stream/fetch globals the
 * bridge's JS adapters take, `esnext.disposable` for the `Symbol.dispose`
 * wasm-bindgen emits on every exported class.
 */
const OPTIONS = {
  strict: true, skipLibCheck: false, noEmit: true,
  target: "es2022", module: "esnext", moduleResolution: "bundler",
  lib: ["es2022", "es2024.string", "esnext.typedarrays", "esnext.disposable", "dom"],
  types: ["node"], typeRoots: [join(ROOT, "node_modules", "@types")],
};

// A full-program check over the bridge's declarations plus the 738 kB proto
// namespace takes longer than the default per-test budget.
const TIMEOUT_MS = 120_000;

test("the published declarations typecheck without skipLibCheck", async () => {
  expect(
    existsSync(ENTRY),
    "dist/index.d.ts is absent — run `bun run build` first",
  ).toBe(true);

  const errors = await ts.diagnostics({ [ENTRY]: undefined, [CONSUMER]: undefined }, OPTIONS);
  expect(errors.map(error => `${error.fileName ?? ""} TS${error.code}: ${error.text}`)).toEqual([]);
}, TIMEOUT_MS);

/**
 * The same declarations under NodeNext, where a relative specifier without an
 * extension is a hard error (TS2834/TS2835) rather than something the bundler
 * resolution this file's first test uses would accept. Kept as its own test
 * so each program stays inside the per-test clock budget.
 */
const NODENEXT_OPTIONS = {
  strict: true, skipLibCheck: false, noEmit: true,
  target: "es2022", module: "nodenext", moduleResolution: "nodenext",
  types: ["node"], typeRoots: [join(ROOT, "node_modules", "@types")],
};

test("the published declarations typecheck under NodeNext without skipLibCheck", async () => {
  expect(
    existsSync(ENTRY),
    "dist/index.d.ts is absent — run `bun run build` first",
  ).toBe(true);

  const errors = await ts.diagnostics({ [ENTRY]: undefined, [CONSUMER]: undefined }, NODENEXT_OPTIONS);
  expect(errors.map(error => `${error.fileName ?? ""} TS${error.code}: ${error.text}`)).toEqual([]);
}, TIMEOUT_MS);

/**
 * `dist/index.js` is bundled, but `proto-reader.d.ts` exposes the base
 * `BinaryReader`/`BinaryWriter` types from `@bufbuild/protobuf/wire`. A clean
 * strict consumer therefore needs the package declared by the published
 * manifest, not merely available from this checkout's devDependencies.
 */
test("package.json declares the wire-type dependency used by published declarations", async () => {
  const manifest = JSON.parse(
    readFileSync(join(ROOT, "package.json"), "utf8"),
  ) as { dependencies?: Record<string, string>; devDependencies?: Record<string, string> };
  expect(manifest.dependencies?.["@bufbuild/protobuf"]).toBe("^2.16.0");
  expect(manifest.devDependencies?.["@bufbuild/protobuf"]).toBeUndefined();
});
