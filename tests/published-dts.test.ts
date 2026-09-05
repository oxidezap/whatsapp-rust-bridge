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
import ts from "typescript";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const ENTRY = join(ROOT, "dist", "index.d.ts");
const CONSUMER = join(ROOT, "tests", "fixtures", "published-dts-consumer.ts");

/**
 * The lib set a consumer would have: `dom` for the stream/fetch globals the
 * bridge's JS adapters take, `esnext.disposable` for the `Symbol.dispose`
 * wasm-bindgen emits on every exported class.
 */
const OPTIONS: ts.CompilerOptions = {
  strict: true,
  skipLibCheck: false,
  noEmit: true,
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  lib: [
    "lib.es2022.d.ts",
    "lib.es2024.string.d.ts",
    "lib.esnext.typedarrays.d.ts",
    "lib.esnext.disposable.d.ts",
    "lib.dom.d.ts",
  ],
  types: ["node"],
  typeRoots: [join(ROOT, "node_modules", "@types")],
};

// A full-program check over the bridge's declarations plus the 738 kB proto
// namespace takes longer than the default per-test budget.
const TIMEOUT_MS = 120_000;

test("the published declarations typecheck without skipLibCheck", () => {
  expect(
    existsSync(ENTRY),
    "dist/index.d.ts is absent — run `bun run build` first",
  ).toBe(true);

  const program = ts.createProgram([ENTRY, CONSUMER], OPTIONS);
  const messages = ts
    .getPreEmitDiagnostics(program)
    .map((diagnostic) => {
      const text = ts.flattenDiagnosticMessageText(diagnostic.messageText, " ");
      if (!diagnostic.file || diagnostic.start === undefined) {
        return `TS${diagnostic.code}: ${text}`;
      }
      const { line } = diagnostic.file.getLineAndCharacterOfPosition(
        diagnostic.start,
      );
      const path = diagnostic.file.fileName.replace(`${ROOT}/`, "");
      return `${path}:${line + 1} TS${diagnostic.code}: ${text}`;
    })
    .sort();

  expect(messages).toEqual([]);
}, TIMEOUT_MS);

/**
 * The same declarations under NodeNext, where a relative specifier without an
 * extension is a hard error (TS2834/TS2835) rather than something the bundler
 * resolution this file's first test uses would accept. Kept as its own test
 * so each program stays inside the per-test clock budget.
 */
const NODENEXT_OPTIONS: ts.CompilerOptions = {
  strict: true,
  skipLibCheck: false,
  noEmit: true,
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.NodeNext,
  moduleResolution: ts.ModuleResolutionKind.NodeNext,
  types: ["node"],
  typeRoots: [join(ROOT, "node_modules", "@types")],
};

test("the published declarations typecheck under NodeNext without skipLibCheck", () => {
  expect(
    existsSync(ENTRY),
    "dist/index.d.ts is absent — run `bun run build` first",
  ).toBe(true);

  const program = ts.createProgram([ENTRY, CONSUMER], NODENEXT_OPTIONS);
  const messages = ts
    .getPreEmitDiagnostics(program)
    .map((diagnostic) => {
      const text = ts.flattenDiagnosticMessageText(diagnostic.messageText, " ");
      if (!diagnostic.file || diagnostic.start === undefined) {
        return `TS${diagnostic.code}: ${text}`;
      }
      const { line } = diagnostic.file.getLineAndCharacterOfPosition(
        diagnostic.start,
      );
      const path = diagnostic.file.fileName.replace(`${ROOT}/`, "");
      return `${path}:${line + 1} TS${diagnostic.code}: ${text}`;
    })
    .sort();

  expect(messages).toEqual([]);
}, TIMEOUT_MS);

/**
 * `dist/proto-reader.d.ts` imports `@bufbuild/protobuf/wire`, so the package
 * must declare it: this checkout's own devDependencies mask the hole (the
 * first test above resolves it from the repo's `node_modules`), and only an
 * isolated install of the tarball proves the failure — and the fix — for real.
 */
test("the published declarations resolve their type imports from declared dependencies", () => {
  const manifest = JSON.parse(
    readFileSync(join(ROOT, "package.json"), "utf8"),
  ) as { dependencies?: Record<string, string> };
  expect(manifest.dependencies?.["@bufbuild/protobuf"]).toBeDefined();
});
