/**
 * Which functions in the wasm are large, and by how much.
 *
 * `scripts/check-size.ts` measures the package; this measures the shape inside
 * it. The two are not the same problem: a module can hold its byte budget and
 * still hand V8 a handful of functions whose register allocator dominates the
 * compile, which is what `scripts/wasm-zone-peak.mjs` then prices. Sizes here
 * are body bytes, so they line up with the indices V8 prints under
 * `--trace-wasm-compilation-times`. `scripts/check-wasm-shape.mjs` gates the
 * largest of them, off the same parse.
 *
 * The shipped artifact carries no name section (`strip = true` plus wasm-opt's
 * `--strip-debug`), so this reports indices. To get names, build with
 * `strip = "none"` in `[profile.release]`, run `wasm-bindgen --keep-debug`, and
 * run the release wasm-opt flags with `-g` and without `--strip-debug`; the
 * body sizes match the shipped build, which is what ties an index to a name.
 *
 *   node scripts/wasm-fn-sizes.mjs pkg/whatsapp_rust_bridge_bg.wasm [topN]
 */
import { codeShape } from "./wasm-code-shape.mjs";

const path = process.argv[2];
const topN = Number(process.argv[3] ?? 20);
if (!path) {
  console.error("usage: node scripts/wasm-fn-sizes.mjs <file.wasm> [topN]");
  process.exit(2);
}

const shape = codeShape(path);

console.log(`${path}`);
console.log(`  functions     ${shape.bodies.length} defined, ${shape.importedFunctions} imported`);
console.log(`  code bodies   ${shape.total} bytes`);
console.log(`  median body   ${shape.medianBody} bytes`);
console.log(`  largest body  ${shape.largestBody} bytes`);
console.log(
  `  name section  ${shape.names.size ? `${shape.names.size} names` : "absent (indices only)"}`
);
console.log("");

for (const [rank, fn] of shape.bySize.slice(0, topN).entries()) {
  const share = ((fn.size / shape.total) * 100).toFixed(2);
  console.log(
    `${String(rank + 1).padStart(3)}. #${String(fn.index).padEnd(6)} ${String(fn.size).padStart(7)} B  ${share.padStart(5)}%  ${shape.names.get(fn.index) ?? ""}`
  );
}
