/**
 * The size column of the allocator sweep: what each arm's artifact weighs, and
 * how those bytes are distributed. The distribution matters on its own, because
 * `check:wasm-shape` gates the largest body and a new allocator changes what
 * wasm-opt has to merge and inline.
 *
 *   node benches/wasm-allocator/sizes.mjs a b c
 */
import { join } from "node:path";
import { statSync } from "node:fs";
import { sectionSizes } from "../wasm-module-rss/sections.mjs";
import { codeShape } from "../../scripts/wasm-code-shape.mjs";
import { artifactDir } from "./artifact.mjs";

const names = process.argv.slice(2);
if (names.length === 0) {
  console.error("usage: sizes.mjs <artifact> …");
  process.exit(2);
}

const rows = names.map((name) => {
  const path = join(artifactDir, `${name}.wasm`);
  const sections = sectionSizes(path);
  const shape = codeShape(path);
  return {
    name,
    file: statSync(path).size,
    code: sections.code ?? 0,
    data: sections.data ?? 0,
    functions: shape.bodies.length,
    largest: shape.largestBody,
    medianBody: shape.medianBody,
  };
});

const base = rows[0];
const pad = (s, n) => String(s).padEnd(n);
const rpad = (s, n) => String(s).padStart(n);
const cols = [
  ["artifact", 26, (r) => r.name],
  ["file", 12, (r) => r.file.toLocaleString("en-US")],
  ["vs base", 11, (r) => (r === base ? "" : (r.file - base.file).toLocaleString("en-US"))],
  ["code", 12, (r) => r.code.toLocaleString("en-US")],
  ["data", 10, (r) => r.data.toLocaleString("en-US")],
  ["functions", 11, (r) => r.functions.toLocaleString("en-US")],
  ["median body", 13, (r) => r.medianBody.toLocaleString("en-US")],
  ["largest body", 14, (r) => r.largest.toLocaleString("en-US")],
];

console.log(cols.map(([h, w], i) => (i ? rpad(h, w) : pad(h, w))).join("  "));
for (const row of rows) {
  console.log(cols.map(([, w, f], i) => (i ? rpad(f(row), w) : pad(f(row), w))).join("  "));
}
