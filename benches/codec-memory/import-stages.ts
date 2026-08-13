/**
 * Where the private memory of one import goes, stage by stage.
 *
 * This is the evidence for the import decomposition in
 * docs/proto-codec-memory.md — in particular that freeing the wasm Buffer does
 * not return the pages, which is what decides whether the JS half of an import
 * is ~8 MiB or ~18 MiB.
 *
 * Run: bun run bench:codec-memory:import-stages   (needs pkg/)
 */

import { cpSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT } from "./slice";

const HERE = import.meta.dir;
const WORK = join(ROOT, "target", "codec-memory", "import-stages");

const REPS = Number(process.env.REPS ?? 5);
if (!Number.isSafeInteger(REPS) || REPS < 1) throw new Error(`REPS must be a positive integer, got ${process.env.REPS}`);
const NODE = process.env.NODE_BIN ?? "node";
const NODE_FLAGS = (process.env.NODE_FLAGS ?? "").split(" ").filter(Boolean);

const BOOTSTRAP = `const wasmUrl = new URL("whatsapp_rust_bridge_bg.wasm", import.meta.url);
const wasmBytes = readFileSync(wasmUrl);
initSync({ module: wasmBytes });`;

const tree = (name: string): string => {
  const dir = join(WORK, name);
  rmSync(dir, { recursive: true, force: true });
  cpSync(join(ROOT, "ts"), join(dir, "ts"), { recursive: true });
  symlinkSync(join(ROOT, "pkg"), join(dir, "pkg"));
  symlinkSync(join(ROOT, "node_modules"), join(dir, "node_modules"));
  return dir;
};

const build = (dir: string, out: string): number => {
  const built = Bun.spawnSync({
    cmd: ["bun", "build", join(dir, "ts", "index.ts"), "--minify", "--target", "node", "--outfile", join(WORK, out)],
    stdout: "pipe",
    stderr: "inherit",
  });
  if (built.exitCode !== 0) throw new Error(`bundle failed: ${out}`);
  return statSync(join(WORK, out)).size;
};

mkdirSync(WORK, { recursive: true });
// The bundle resolves the wasm next to itself, the way dist/ ships it, and the
// byte stages read the same copy.
cpSync(join(ROOT, "pkg", "whatsapp_rust_bridge_bg.wasm"), join(WORK, "whatsapp_rust_bridge_bg.wasm"));

const full = build(tree("full"), "full.js");

// The same library with the three bootstrap lines gone: the glue is still
// evaluated, the module is never compiled or instantiated.
const bare = tree("js-only");
const entry = join(bare, "ts", "index.ts");
const source = readFileSync(entry, "utf8");
if (!source.includes(BOOTSTRAP)) throw new Error("ts/index.ts no longer carries the bootstrap this strips");
writeFileSync(entry, source.replace(BOOTSTRAP, ""));
const jsOnly = build(bare, "js-only.js");

const STAGES = ["wasm-bytes", "wasm-module", "wasm-module-freed", "js-only", "full"] as const;
const samples = new Map<string, number[]>(STAGES.map((stage) => [stage, []]));

for (let rep = 0; rep < REPS; rep++) {
  for (let i = 0; i < STAGES.length; i++) {
    const stage = STAGES[(i + rep) % STAGES.length]!;
    const proc = Bun.spawnSync({
      cmd: [NODE, "--expose-gc", ...NODE_FLAGS, join(HERE, "import-stages-probe.mjs"), stage, WORK],
      stdout: "pipe",
      stderr: "inherit",
    });
    if (proc.exitCode !== 0) throw new Error(`probe failed: ${stage}`);
    samples.get(stage)!.push(JSON.parse(proc.stdout.toString()).delta);
  }
}

const median = (xs: number[]) => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
};

const version = Bun.spawnSync({ cmd: [NODE, "-v"], stdout: "pipe" }).stdout.toString().trim();
console.log(`reps=${REPS} node=${version} bun=${Bun.version} flags=${NODE_FLAGS.join(" ") || "(none)"}`);
console.log(`wasm=${statSync(join(WORK, "whatsapp_rust_bridge_bg.wasm")).size} B  full=${full} B  js-only=${jsOnly} B`);
console.log(["stage", "MiB med", "KiB med", "min", "max"].join("\t"));
for (const stage of STAGES) {
  const xs = samples.get(stage)!;
  console.log(
    [stage, (median(xs) / 1024).toFixed(2), median(xs).toFixed(0), Math.min(...xs), Math.max(...xs)].join("\t"),
  );
}
