/**
 * Which functions in the wasm are large, and by how much.
 *
 * `scripts/check-size.ts` measures the package; this measures the shape inside
 * it. The two are not the same problem: a module can hold its byte budget and
 * still hand V8 a handful of functions whose register allocator dominates the
 * compile, which is what `scripts/wasm-zone-peak.mjs` then prices. Sizes here
 * are body bytes, so they line up with the indices V8 prints under
 * `--trace-wasm-compilation-times`.
 *
 * The shipped artifact carries no name section (`strip = true` plus wasm-opt's
 * `--strip-debug`), so this reports indices. To get names, build with
 * `strip = "none"` in `[profile.release]`, run `wasm-bindgen --keep-debug`, and
 * run the release wasm-opt flags with `-g` and without `--strip-debug`; the
 * body sizes match the shipped build, which is what ties an index to a name.
 *
 *   node scripts/wasm-fn-sizes.mjs pkg/whatsapp_rust_bridge_bg.wasm [topN]
 */
import { readFileSync } from "node:fs";

const path = process.argv[2];
const topN = Number(process.argv[3] ?? 20);
if (!path) {
  console.error("usage: node scripts/wasm-fn-sizes.mjs <file.wasm> [topN]");
  process.exit(2);
}

const buf = readFileSync(path);
let at = 0;
const byte = () => buf[at++];
const u32 = () => {
  const v = buf.readUInt32LE(at);
  at += 4;
  return v;
};
const uleb = () => {
  let result = 0;
  let shift = 0;
  let b;
  do {
    b = buf[at++];
    result += (b & 0x7f) * 2 ** shift;
    shift += 7;
  } while (b & 0x80);
  return result;
};

if (u32() !== 0x6d736100) throw new Error(`${path} is not a wasm module`);
u32();

let importedFunctions = 0;
const bodies = [];
const names = new Map();

while (at < buf.length) {
  const id = byte();
  const size = uleb();
  const start = at;

  if (id === 2) {
    // Imported functions come first in the index space, so the code section's
    // Nth body is function N + importedFunctions.
    const count = uleb();
    for (let i = 0; i < count; i++) {
      const moduleLength = uleb();
      at += moduleLength;
      const fieldLength = uleb();
      at += fieldLength;
      const kind = byte();
      if (kind === 0) {
        uleb();
        importedFunctions++;
      } else if (kind === 1) {
        byte();
        const flags = byte();
        uleb();
        if (flags & 1) uleb();
      } else if (kind === 2) {
        const flags = byte();
        uleb();
        if (flags & 1) uleb();
      } else if (kind === 3) {
        byte();
        byte();
      } else if (kind === 4) {
        byte();
        uleb();
      } else {
        throw new Error(`unknown import kind ${kind}`);
      }
    }
  } else if (id === 10) {
    const count = uleb();
    for (let i = 0; i < count; i++) {
      const bodySize = uleb();
      bodies.push({ index: importedFunctions + i, size: bodySize });
      at += bodySize;
    }
  } else if (id === 0) {
    const nameLength = uleb();
    const sectionName = buf.subarray(at, at + nameLength).toString("utf8");
    at += nameLength;
    if (sectionName === "name") {
      const end = start + size;
      while (at < end) {
        const subsection = byte();
        const subsectionSize = uleb();
        const subsectionEnd = at + subsectionSize;
        if (subsection === 1) {
          const count = uleb();
          for (let i = 0; i < count; i++) {
            const index = uleb();
            const length = uleb();
            names.set(index, buf.subarray(at, at + length).toString("utf8"));
            at += length;
          }
        }
        at = subsectionEnd;
      }
    }
  }

  at = start + size;
}

const total = bodies.reduce((sum, f) => sum + f.size, 0);
const sorted = [...bodies].sort((a, b) => a.size - b.size);

console.log(`${path}`);
console.log(`  functions     ${bodies.length} defined, ${importedFunctions} imported`);
console.log(`  code bodies   ${total} bytes`);
console.log(`  median body   ${sorted[Math.floor(sorted.length / 2)].size} bytes`);
console.log(`  largest body  ${sorted[sorted.length - 1].size} bytes`);
console.log(`  name section  ${names.size ? `${names.size} names` : "absent (indices only)"}`);
console.log("");

for (const [rank, fn] of sorted.reverse().slice(0, topN).entries()) {
  const share = ((fn.size / total) * 100).toFixed(2);
  console.log(
    `${String(rank + 1).padStart(3)}. #${String(fn.index).padEnd(6)} ${String(fn.size).padStart(7)} B  ${share.padStart(5)}%  ${names.get(fn.index) ?? ""}`
  );
}
