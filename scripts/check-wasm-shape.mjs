/**
 * Shape gate: fails when the wasm's largest function body grows past its
 * budget.
 *
 * `check-size.ts` guards the package's total bytes, and the total is blind to
 * this: the artifact that costs a consumer ~18 MiB more private memory at
 * connect is 157 bytes LARGER than the one that does not. What separates them
 * is how those bytes are distributed — one 177,110-byte function against a
 * largest of 86,902 — because V8 pays per function it compiles, and the
 * compile-zone peak is set by the most expensive one
 * (`docs/wasm-compile-zone-peak.md`).
 *
 * That distribution is set by one wasm-opt flag in `Cargo.toml`
 * (`--one-caller-inline-max-function-size`). A flag list is easy to reorder,
 * copy from an older revision, or drop while chasing something else, and
 * nothing about the result would look wrong: same exports, same behaviour,
 * same size, no test to fail. This is what fails instead.
 *
 * Why the largest body and not the zone peak: the peak is a V8 statistic that
 * moves with the V8 version, so a budget for it committed here would fail on a
 * Node bump rather than on a regression. The largest body is a property of our
 * own bytes, it is what sets the serialised peak, and it moves with the same
 * flag. `measure:zone-peak` prices the peak itself when a human wants the
 * number.
 *
 *   node scripts/check-wasm-shape.mjs [pkg/whatsapp_rust_bridge_bg.wasm]
 */
import { codeShape } from "./wasm-code-shape.mjs";

/**
 * 14% over the 87,456 bytes the capped build produces — room for a core update
 * that grows the decoder, and under the 101,933 that merely doubling the cap to
 * 4000 would produce, so a loosened flag fails here rather than passing as a
 * rounding difference. An uncapped build is 177,110.
 *
 * Going over is a prompt to run `bun run measure:fn-sizes`, look at what grew,
 * and decide — then either fix it or raise this in the same commit, with the
 * connect-window cost of the growth measured rather than assumed.
 */
const MAX_LARGEST_BODY_BYTES = 100_000;

const path = process.argv[2] ?? "pkg/whatsapp_rust_bridge_bg.wasm";

let shape;
try {
  shape = codeShape(path);
} catch (error) {
  console.error(`check-wasm-shape: cannot measure ${path}: ${error.message}`);
  console.error(`check-wasm-shape: run 'bun run build' first.`);
  process.exit(1);
}

// A --dev build skips wasm-opt entirely, so nothing has been inlined and its
// largest body clears this budget by half — a pass that measured the wrong
// artifact. The published one is stripped (`strip = true` plus wasm-opt's
// `--strip-debug`), so a name section means this is not it.
if (shape.names.size > 0) {
  console.error(
    `check-wasm-shape: ${path} still carries a name section (${shape.names.size} names).\n` +
      `check-wasm-shape: that is a --dev or --keep-debug build, whose function sizes are ` +
      `not the published ones. Measure a 'bun run build'.`
  );
  process.exit(1);
}

for (const fn of shape.bySize.slice(0, 5)) {
  const share = ((fn.size / shape.total) * 100).toFixed(2);
  console.log(
    `check-wasm-shape:   #${String(fn.index).padEnd(6)} ${String(fn.size).padStart(7)} B  ${share.padStart(5)}%`
  );
}

if (shape.largestBody > MAX_LARGEST_BODY_BYTES) {
  console.error(
    `check-wasm-shape: largest function body ${shape.largestBody} B, over the ` +
      `${MAX_LARGEST_BODY_BYTES} B budget by ${shape.largestBody - MAX_LARGEST_BODY_BYTES} B.\n` +
      `check-wasm-shape: if the wasm-opt pass list in Cargo.toml no longer carries ` +
      `--one-caller-inline-max-function-size, that is the cause — see ` +
      `docs/wasm-compile-zone-peak.md.`
  );
  process.exit(1);
}

console.log(
  `check-wasm-shape: ${shape.bodies.length} functions, largest body ${shape.largestBody} B, ` +
    `${MAX_LARGEST_BODY_BYTES - shape.largestBody} B under budget`
);
