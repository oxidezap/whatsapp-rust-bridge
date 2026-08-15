/**
 * What a wasm artifact costs in private memory, per process, as a function of
 * its `code` section.
 *
 * Each reading is its own process, and the artifacts are measured round-robin
 * rather than one artifact at a time: V8's heap grows in steps, so consecutive
 * runs of the same artifact share whatever step the machine happened to be in.
 * A single pair proves nothing here — an earlier single-pair reading in this
 * area showed a 20 MiB "win" that repetition dissolved.
 *
 *   node benches/wasm-module-rss/measure.mjs [--reps=8] [--mode=compile] a.wasm b.wasm …
 *
 * `--mode=instantiate` adds an instance over stub imports, which brings the
 * artifact's linear memory in; `compile` isolates the module itself.
 */
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { sectionSizes } from "./sections.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const probe = join(here, "probe.mjs");

const args = process.argv.slice(2);
const reps = Number(args.find((a) => a.startsWith("--reps="))?.slice(7) ?? 8);
const mode = args.find((a) => a.startsWith("--mode="))?.slice(7) ?? "compile";
const artifacts = args.filter((a) => !a.startsWith("--"));
if (artifacts.length === 0) {
  console.error("usage: measure.mjs [--reps=N] [--mode=compile|instantiate] <a.wasm> …");
  process.exit(2);
}

const MIB = 1024 * 1024;
const mib = (bytes) => bytes / MIB;

const median = (xs) => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
const stddev = (xs) => {
  if (xs.length < 2) return 0;
  const m = mean(xs);
  return Math.sqrt(xs.reduce((a, b) => a + (b - m) ** 2, 0) / (xs.length - 1));
};

const samples = new Map(artifacts.map((path) => [path, []]));
for (let rep = 0; rep < reps; rep++) {
  for (const path of artifacts) {
    const out = execFileSync(process.execPath, ["--expose-gc", probe, path, mode], {
      encoding: "utf8",
    });
    samples.get(path).push(JSON.parse(out));
  }
}

const rows = [];
for (const [path, runs] of samples) {
  const sections = sectionSizes(path);
  const peakKey = mode === "instantiate" ? "afterInstantiate" : "afterCompile";
  const delta = (from, to, field) => runs.map((r) => r[to][field] - r[from][field]);
  rows.push({
    path,
    code: sections.code ?? 0,
    total: sections.total,
    functionCount: sections.functionCount ?? 0,
    compileDirty: delta("afterRead", peakKey, "privateDirty"),
    retainedDirty: delta("base", "afterFreeWire", "privateDirty"),
    retainedClean: delta("base", "afterFreeWire", "privateClean"),
    retainedUss: delta("base", "afterFreeWire", "uss"),
    node: runs[0].node,
  });
}

rows.sort((a, b) => a.code - b.code);

const stat = (xs) => `${mib(median(xs)).toFixed(2)} ±${mib(stddev(xs)).toFixed(2)}`;
const columns = [
  ["artifact", (r) => r.path.replace(/^.*\//, "")],
  ["code MiB", (r) => mib(r.code).toFixed(2)],
  ["file MiB", (r) => mib(r.total).toFixed(2)],
  ["functions", (r) => String(r.functionCount)],
  ["compile ΔPriv_Dirty", (r) => stat(r.compileDirty)],
  ["retained ΔPriv_Dirty", (r) => stat(r.retainedDirty)],
  ["retained ΔPriv_Clean", (r) => stat(r.retainedClean)],
  ["retained ΔUSS", (r) => stat(r.retainedUss)],
];

const table = [columns.map(([h]) => h), ...rows.map((r) => columns.map(([, f]) => f(r)))];
const widths = table[0].map((_, i) => Math.max(...table.map((row) => row[i].length)));
console.log(`node ${rows[0].node}, mode=${mode}, reps=${reps}, medians ± sample stddev`);
for (const [i, row] of table.entries()) {
  console.log(row.map((cell, c) => cell.padStart(widths[c])).join("  "));
  if (i === 0) console.log(widths.map((w) => "-".repeat(w)).join("  "));
}

// The slope is the whole question: does a MiB off the code section come back as
// a MiB of private memory, or does V8 just move the commit somewhere else?
if (rows.length >= 2) {
  console.log("\nretained ΔPrivate_Dirty per MiB of code section, against the smallest artifact:");
  const baseRow = rows[0];
  for (const row of rows.slice(1)) {
    const dCode = mib(row.code - baseRow.code);
    const dDirty = mib(median(row.retainedDirty) - median(baseRow.retainedDirty));
    console.log(
      `  ${row.path.replace(/^.*\//, "")}: ${dCode >= 0 ? "+" : ""}${dCode.toFixed(2)} MiB code ` +
        `→ ${dDirty >= 0 ? "+" : ""}${dDirty.toFixed(2)} MiB Private_Dirty  ` +
        `(${(dDirty / dCode).toFixed(2)} MiB/MiB)`
    );
  }
}

// A single slope against the code section hides that two different things are
// being paid for. Fitting both separates them, and the fit is what says which
// lever a proposed cut is actually pulling.
if (rows.length >= 4) {
  // Columns are scaled to comparable magnitudes first: raw wire bytes against a
  // function count and a constant makes XᵀX span fourteen orders of magnitude,
  // and the elimination falls apart on it.
  const y = rows.map((r) => mib(median(r.retainedDirty)));
  const design = rows.map((r) => [mib(r.total), r.functionCount / 1000, 1]);
  const solved = solveNormalEquations(design, y);
  if (solved) {
    const [perWireByte, perThousandFunctions, constantMib] = solved;
    const perFunction = (perThousandFunctions * MIB) / 1000;
    const predicted = design.map((row) => row.reduce((a, v, i) => a + v * solved[i], 0));
    const ssRes = y.reduce((a, v, i) => a + (v - predicted[i]) ** 2, 0);
    const ssTot = y.reduce((a, v) => a + (v - mean(y)) ** 2, 0);
    console.log("\nleast squares, retained Private_Dirty ~ wire bytes + function count:");
    console.log(`  ${perWireByte.toFixed(3)} bytes per wire byte`);
    console.log(`  ${perFunction.toFixed(1)} bytes per function`);
    console.log(`  ${constantMib.toFixed(2)} MiB constant`);
    console.log(`  R² = ${(1 - ssRes / ssTot).toFixed(4)}, residuals in MiB:`);
    for (const [i, row] of rows.entries()) {
      console.log(
        `    ${row.path.replace(/^.*\//, "").padEnd(20)}${(y[i] - predicted[i]).toFixed(3).padStart(7)}`
      );
    }
  }
}

console.log(`\n${JSON.stringify({ mode, reps, rows }, null, 0)}`);

/** Gaussian elimination on XᵀX b = Xᵀy. Returns null if the columns are collinear. */
function solveNormalEquations(design, y) {
  const width = design[0].length;
  const matrix = Array.from({ length: width }, (_, i) => {
    const row = Array.from({ length: width }, (_, j) =>
      design.reduce((a, r) => a + r[i] * r[j], 0)
    );
    row.push(design.reduce((a, r, k) => a + r[i] * y[k], 0));
    return row;
  });
  for (let col = 0; col < width; col++) {
    let pivot = col;
    for (let r = col + 1; r < width; r++) {
      if (Math.abs(matrix[r][col]) > Math.abs(matrix[pivot][col])) pivot = r;
    }
    if (Math.abs(matrix[pivot][col]) < 1e-9) return null;
    [matrix[col], matrix[pivot]] = [matrix[pivot], matrix[col]];
    for (let r = 0; r < width; r++) {
      if (r === col) continue;
      const factor = matrix[r][col] / matrix[col][col];
      for (let c = col; c <= width; c++) matrix[r][c] -= factor * matrix[col][c];
    }
  }
  return matrix.map((row, i) => row[width] / row[i]);
}
