/**
 * The `dist/` specifier rewrite, pinned without a build: every supported
 * module-specifier form gains `.js` exactly when `dist/` has the target, an
 * already-correct specifier and every bare package stay untouched, and quoted
 * lookalikes in comments and string-literal types are never mistaken for a
 * module. What the rewrite cannot resolve it reports rather than guesses.
 *
 * Run: bun test tests/finalize-dist.test.ts
 */

import { test, expect } from "bun:test";
import {
  fixModuleSpecifier,
  rewriteDtsSpecifiers,
} from "../scripts/dts-specifiers";

const DIST_FILES = new Set([
  "./plain.d.ts",
  "./single.d.ts",
  "./re.d.ts",
  "./re2.d.ts",
  "./side.d.ts",
  "./itype.d.ts",
  "./foo.types.d.ts",
  "./ok.d.ts",
]);
const exists = (relPath: string): boolean => DIST_FILES.has(relPath);

const FIXTURE = `import { a } from "./plain";
import type { B } from './single';
export * from "./re";
export { x } from "./re2";
import "./side";
export declare const t: import("./itype").X;
export { c } from "./foo.types";
export { ok } from "./ok.js";
import { w } from "@bufbuild/protobuf/wire";
import { Buffer } from "node:buffer";
// import { ghost } from "./comment-lookalike";
/* export { nope } from "./block-comment"; */
export type S = "./string-lookalike";
declare const s: "./const-lookalike";
`;

test("supported specifier forms resolve, lookalikes do not move", () => {
  const { text, edits } = rewriteDtsSpecifiers(FIXTURE, "fixture.d.ts", exists);

  for (const [from, to] of [
    ["./plain", "./plain.js"],
    ["./re", "./re.js"],
    ["./re2", "./re2.js"],
    ["./side", "./side.js"],
    ["./itype", "./itype.js"],
    ["./foo.types", "./foo.types.js"],
  ] as const) {
    expect(text).toContain(`"${to}"`);
    expect(edits).toContainEqual({ from, to });
  }
  expect(text).toContain(`from './single.js'`);
  expect(edits).toContainEqual({ from: "./single", to: "./single.js" });
  expect(text).toContain(`from "./ok.js"`);
  expect(text).toContain(`from "@bufbuild/protobuf/wire"`);
  expect(text).toContain(`from "node:buffer"`);
  expect(text).toContain(`// import { ghost } from "./comment-lookalike";`);
  expect(text).toContain(`/* export { nope } from "./block-comment"; */`);
  expect(text).toContain(`export type S = "./string-lookalike";`);
  expect(text).toContain(`declare const s: "./const-lookalike";`);
  expect(edits).toHaveLength(7);
});

test("the rewrite is idempotent", () => {
  const once = rewriteDtsSpecifiers(FIXTURE, "fixture.d.ts", exists).text;
  const twice = rewriteDtsSpecifiers(once, "fixture.d.ts", exists);
  expect(twice.text).toBe(once);
  expect(twice.edits).toEqual([]);
});

test("bare packages and exact files are left alone", () => {
  expect(fixModuleSpecifier("@bufbuild/protobuf/wire", exists)).toBeNull();
  expect(fixModuleSpecifier("node:buffer", exists)).toBeNull();
  expect(fixModuleSpecifier("./ok.js", exists)).toBeNull();
  expect(
    fixModuleSpecifier("./whatsapp_rust_bridge.js", (p) =>
      ["./whatsapp_rust_bridge.js", "./whatsapp_rust_bridge.d.ts"].includes(p),
    ),
  ).toBeNull();
});

test("a relative specifier that names nothing stops the build", () => {
  expect(() => fixModuleSpecifier("./missing", exists)).toThrow(
    /names nothing in dist/,
  );
  expect(() => fixModuleSpecifier("./ok.jsx", exists)).toThrow();
  expect(() => fixModuleSpecifier("../outside", exists)).toThrow(/outside dist/);
});
