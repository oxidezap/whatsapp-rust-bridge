/**
 * One process, one stage of `import "@oxidezap/whatsapp-rust-bridge"`.
 *
 * argv: <stage> <workdir> — stages are `wasm-bytes`, `wasm-module`,
 * `wasm-module-freed`, `js-only`, `full`.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

const priv = () => {
  const match = /^Private_Dirty:\s+(\d+) kB$/m.exec(readFileSync("/proc/self/smaps_rollup", "utf8"));
  if (!match) throw new Error("/proc/self/smaps_rollup carries no Private_Dirty line — this probe needs Linux");
  return Number(match[1]);
};
const settle = () => {
  global.gc();
  global.gc();
};

const stage = process.argv[2];
const work = process.argv[3];
const wasm = join(work, "whatsapp_rust_bridge_bg.wasm");

settle();
const before = priv();

if (stage === "wasm-bytes") {
  globalThis.__keep = readFileSync(wasm);
} else if (stage === "wasm-module") {
  const bytes = readFileSync(wasm);
  globalThis.__keep = [bytes, new WebAssembly.Module(bytes)];
} else if (stage === "wasm-module-freed") {
  // The Buffer goes out of scope before the reading, so the settle() below can
  // collect it. Whether the pages come back is the question.
  globalThis.__keep = new WebAssembly.Module(readFileSync(wasm));
} else if (stage === "js-only") {
  globalThis.__keep = await import(join(work, "js-only.js"));
} else if (stage === "full") {
  globalThis.__keep = await import(join(work, "full.js"));
} else {
  throw new Error(`unknown stage: ${stage}`);
}

settle();
const after = priv();
const heap = process.memoryUsage();
console.log(
  JSON.stringify({
    stage,
    delta: after - before,
    heapUsed: Math.round(heap.heapUsed / 1024),
    external: Math.round(heap.external / 1024),
  }),
);
