/**
 * What a codec count costs, with the codec layer on its own.
 *
 * The `lazy`, `string` and `file` arms are controls rather than counts; what
 * each one isolates, and why the isolated numbers disagree with `in-situ.ts`,
 * is in docs/proto-codec-memory.md.
 *
 * Run: bun run bench:codec-memory   (REPS, NODE_BIN, NODE_FLAGS honoured)
 */

import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { shuffled } from "./schedule";
import { assertRoundTrip, closure, emit, parse, PING_PONG_ROOTS, REGISTRY_ROOTS, ROOT, type Mode } from "./slice";

const HERE = import.meta.dir;
const WORK = join(ROOT, "target", "codec-memory");
const SRC = join(WORK, "src");
const OUT = join(WORK, "artifacts");
mkdirSync(SRC, { recursive: true });
mkdirSync(OUT, { recursive: true });

const REPS = Number(process.env.REPS ?? 25);
if (!Number.isSafeInteger(REPS) || REPS < 1) throw new Error(`REPS must be a positive integer, got ${process.env.REPS}`);
const NODE = process.env.NODE_BIN ?? "node";
const NODE_FLAGS = (process.env.NODE_FLAGS ?? "").split(" ").filter(Boolean);
const READER = join(ROOT, "ts", "proto-reader");

const parsed = parse();
assertRoundTrip(parsed);

/** Smallest closures first, for the finest steps at the low end. */
const byClosureSize = [...parsed.codecs.keys()]
  .map((name) => ({ name, size: closure(parsed, [name]).size }))
  .sort((a, b) => a.size - b.size || a.name.localeCompare(b.name));

const closedSubset = (target: number): Set<string> => {
  const kept = new Set<string>();
  for (const { name } of byClosureSize) {
    if (kept.size >= target) break;
    for (const member of closure(parsed, [name])) kept.add(member);
  }
  return kept;
};

const bundle = (name: string, moduleSource: string, extraExports = ""): number => {
  writeFileSync(join(SRC, `${name}.ts`), moduleSource.replace('"../proto-reader"', JSON.stringify(READER)));
  writeFileSync(
    join(SRC, `${name}.entry.ts`),
    [
      `export { codecs${extraExports} } from "./${name}";`,
      // The reader is in every bundle — the codecs import it, and an empty
      // slice must not measure as "one module smaller".
      `export { BinaryReader, BinaryWriter } from ${JSON.stringify(READER)};`,
      "",
    ].join("\n"),
  );
  const built = Bun.spawnSync({
    cmd: ["bun", "build", join(SRC, `${name}.entry.ts`), "--minify", "--target", "node", "--outfile", join(OUT, `${name}.js`)],
    stdout: "pipe",
    stderr: "inherit",
  });
  if (built.exitCode !== 0) throw new Error(`bundle failed: ${name}`);
  return statSync(join(OUT, `${name}.js`)).size;
};

interface Arm {
  name: string;
  label: string;
  codecs: number;
  bytes: number;
}

const arms: Arm[] = [];
const pad = (n: number) => String(n).padStart(3, "0");

const eager = (target: number, suffix = "") => {
  const keep = closedSubset(target);
  const name = `c${pad(keep.size)}${suffix}`;
  arms.push({ name, label: name, codecs: keep.size, bytes: bundle(name, emit(parsed, keep, "eager")) });
};
const named = (label: string, keep: Set<string>, mode: Mode = "eager") => {
  const name = label.replace(/[^a-z0-9]+/gi, "-");
  arms.push({ name, label, codecs: keep.size, bytes: bundle(name, emit(parsed, keep, mode)) });
};

for (const target of [0, 25, 50, 100, 200, 300, 400, 450, 500, 550, 600]) eager(target);
named("ping-pong closure", closure(parsed, PING_PONG_ROOTS));
named("registry closure", closure(parsed, REGISTRY_ROOTS));
named("all 657", new Set(parsed.codecs.keys()));
named("lazy 657", new Set(parsed.codecs.keys()), "lazy");

// Codec text only: reusing `all-657.js` would put the shared reader inside the
// blob as well as in the arm's own module, and the eager arms carry it once.
{
  const built = Bun.spawnSync({
    cmd: [
      "bun", "build", join(SRC, "all-657.entry.ts"), "--minify", "--target", "node",
      "--external", READER, "--outfile", join(OUT, "all-657-codeconly.js"),
    ],
    stdout: "pipe",
    stderr: "inherit",
  });
  if (built.exitCode !== 0) throw new Error("bundle failed: all-657-codeconly");
}
const minified = readFileSync(join(OUT, "all-657-codeconly.js"), "utf8");
const emptyModule = emit(parsed, new Set(), "eager");
arms.push({
  name: "string-657",
  label: "657 as a string literal",
  codecs: 0,
  bytes: bundle(
    "string-657",
    `${emptyModule}\nexport const blob: string = ${JSON.stringify(minified)};\n(globalThis as any).__blob = blob;\n`,
    ", blob",
  ),
});
arms.push({
  name: "file-657",
  label: "657 read from disk",
  codecs: 0,
  bytes: bundle(
    "file-657",
    `${emptyModule}\nimport { readFileSync } from "node:fs";\nexport const blob: string = readFileSync(${JSON.stringify(
      join(OUT, "all-657-codeconly.js"),
    )}, "utf8");\n(globalThis as any).__blob = blob;\n`,
    ", blob",
  ),
});

// Per-process deltas, not absolute readings: two fresh node processes need not
// start from the same page-commit state, and a difference in where they started
// would land on the codec count.
const deltas = new Map<string, number[]>(arms.map((arm) => [arm.name, []]));
const totals = new Map<string, number[]>(arms.map((arm) => [arm.name, []]));
const heaps = new Map<string, number[]>(arms.map((arm) => [arm.name, []]));

// Round-robin, and a fresh permutation each repetition: sampling in a fixed
// order would leave every arm at a fixed position in the sweep, so a machine
// that drifts within a sweep would charge that drift to arm identity.
for (let rep = 0; rep < REPS; rep++) {
  for (const arm of shuffled(arms, rep)) {
    const proc = Bun.spawnSync({
      cmd: [NODE, "--expose-gc", ...NODE_FLAGS, join(HERE, "probe.mjs"), join(OUT, `${arm.name}.js`)],
      stdout: "pipe",
      stderr: "inherit",
    });
    if (proc.exitCode !== 0) throw new Error(`probe failed: ${arm.name}`);
    const row = JSON.parse(proc.stdout.toString());
    deltas.get(arm.name)!.push(row.deltaPrivateDirty);
    totals.get(arm.name)!.push(row.privateDirty);
    heaps.get(arm.name)!.push(row.heapUsed);
  }
}

const median = (xs: number[]) => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
};

const version = Bun.spawnSync({ cmd: [NODE, "-v"], stdout: "pipe" }).stdout.toString().trim();
// bun built every artifact here, and its optimizer decides the bytes being
// measured, so the header names it alongside the node that ran them.
console.log(`reps=${REPS} node=${version} bun=${Bun.version} flags=${NODE_FLAGS.join(" ") || "(none)"}`);
console.log(
  ["arm", "codecs", "bundleKiB", "import Δ med", "min", "max", "vs 0 codecs", "PrivDirty med", "heapUsed med"].join("\t"),
);
const base = median(deltas.get(arms[0]!.name)!);
for (const arm of arms) {
  const xs = deltas.get(arm.name)!;
  console.log(
    [
      arm.label,
      arm.codecs,
      (arm.bytes / 1024).toFixed(1),
      median(xs).toFixed(0),
      Math.min(...xs),
      Math.max(...xs),
      (median(xs) - base).toFixed(0),
      median(totals.get(arm.name)!).toFixed(0),
      median(heaps.get(arm.name)!).toFixed(0),
    ].join("\t"),
  );
}
