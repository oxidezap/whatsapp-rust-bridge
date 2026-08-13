/**
 * Whether a second process holding the same artifact makes the first one's
 * cost shareable. For a file the runtime maps, it does: those pages move from
 * `Private_Clean` to `Shared_Clean` and the second process pays nothing for
 * them. This asks whether the wasm module works that way.
 *
 * The first child compiles and reads its own numbers before the second one is
 * spawned, so the "solo" column really is solo; both then park at a barrier and
 * read again with both resident.
 *
 *   node --expose-gc benches/wasm-module-rss/pair.mjs <artifact.wasm> [--reps=4]
 */
import { readFileSync, writeFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

const self = fileURLToPath(import.meta.url);
const args = process.argv.slice(2);
const childIndex = args.find((a) => a.startsWith("--child="))?.slice(8);
const syncDir = args.find((a) => a.startsWith("--sync="))?.slice(7);
const wasmPath = args.find((a) => !a.startsWith("--"));
const reps = Number(args.find((a) => a.startsWith("--reps="))?.slice(7) ?? 4);

function memory() {
  const rollup = readFileSync("/proc/self/smaps_rollup", "utf8");
  const field = (name) => Number(rollup.match(new RegExp(`^${name}:\\s+(\\d+) kB`, "m"))[1]) * 1024;
  return {
    privateDirty: field("Private_Dirty"),
    privateClean: field("Private_Clean"),
    sharedClean: field("Shared_Clean"),
    rss: field("Rss"),
  };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (childIndex !== undefined) {
  const base = memory();
  const bytes = readFileSync(wasmPath);
  let module = new WebAssembly.Module(bytes);
  const solo = memory();

  writeFileSync(join(syncDir, `ready-${childIndex}`), "");
  while (!existsSync(join(syncDir, "ready-0")) || !existsSync(join(syncDir, "ready-1"))) {
    await sleep(5);
  }
  // Give the other process a moment past its own barrier so neither reading
  // lands while the other is still faulting the artifact in.
  await sleep(250);
  const paired = memory();

  void module;
  module = null;
  writeFileSync(join(syncDir, `out-${childIndex}.json`), JSON.stringify({ base, solo, paired }));
  process.exit(0);
}

if (!wasmPath) {
  console.error("usage: pair.mjs <artifact.wasm> [--reps=N]");
  process.exit(2);
}

const MIB = 1024 * 1024;
const mib = (bytes) => (bytes / MIB).toFixed(2);
const runs = [];

const spawnChild = (index, dir) =>
  new Promise((resolve, reject) =>
    execFile(
      process.execPath,
      ["--expose-gc", self, wasmPath, `--child=${index}`, `--sync=${dir}`],
      (error) => (error ? reject(error) : resolve())
    )
  );

for (let rep = 0; rep < reps; rep++) {
  const dir = mkdtempSync(join(tmpdir(), "wasm-pair-"));
  const first = spawnChild(0, dir);
  // Child 0 writes `ready-0` only after it has compiled and taken its solo
  // reading, so nothing of child 1 is resident while that reading is taken.
  while (!existsSync(join(dir, "ready-0"))) await sleep(5);
  await Promise.all([first, spawnChild(1, dir)]);
  runs.push([0, 1].map((i) => JSON.parse(readFileSync(join(dir, `out-${i}.json`), "utf8"))));
  rmSync(dir, { recursive: true, force: true });
}

console.log(`node ${process.version}, ${wasmPath}, ${reps} repetitions, both children per rep\n`);
console.log("            solo (only this process holds it)   paired (both processes hold it)");
for (const [rep, pair] of runs.entries()) {
  for (const [i, child] of pair.entries()) {
    const line = (label, key) =>
      `${label.padEnd(14)}${mib(child.solo[key]).padStart(10)} MiB${mib(child.paired[key]).padStart(24)} MiB`;
    console.log(`rep ${rep} child ${i}:`);
    for (const [label, key] of [
      ["Private_Dirty", "privateDirty"],
      ["Private_Clean", "privateClean"],
      ["Shared_Clean", "sharedClean"],
      ["Rss", "rss"],
    ]) {
      console.log("  " + line(label, key));
    }
  }
}
