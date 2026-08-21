/**
 * Size gate: fails when the published package grows past its budget.
 *
 * Nothing measured the artifact, so it drifted quietly — 7,027,606 bytes
 * unpacked at 0.6.3, 7,099,879 at 0.6.4, 7,133,345 at 0.6.5. Each step was
 * small enough to pass unnoticed in review, which is exactly the growth a
 * number in a file catches and a reviewer does not.
 *
 * This is a budget, not a limit. Going over is a prompt to look at the largest
 * entries below and decide, then either trim or raise the constant in the same
 * commit that earned the bytes.
 */
import { packedContents } from "./pack";

/**
 * Headroom over 0.6.5's 7,133,345 bytes, at roughly five times the per-release
 * drift above — enough that ordinary work does not trip it, tight enough that
 * a jump does.
 *
 * Raised from 7,500,000 when the core regenerated its whatspec bundle at
 * 2.3000.1044659339: a WhatsApp schema release adds messages and enums across
 * the proto, and the derived declarations and codec grow with it. That is the
 * one kind of growth this budget cannot ask anyone to trim, so it buys the same
 * headroom again over the new floor rather than tracking it.
 *
 * Raised from 7,900,000 for the same reason at 2.3000.1045368834, which grew
 * the package by 217,140 bytes across `index.js` and `proto-types.d.ts` and
 * left the wasm 791 bytes smaller. Same headroom over the new floor.
 */
const MAX_UNPACKED_BYTES = 8_300_000;

const { files, unpackedSize: total } = packedContents();

const mb = (bytes: number) => `${(bytes / 1_000_000).toFixed(2)} MB`;

// Without a build, `npm pack` still succeeds — it ships the always-included
// metadata and nothing else, and a near-zero total would clear the budget by
// the whole budget. A gate that reports 7.5 MB of headroom because it measured
// no package is worse than no gate, so require what has to be there.
const REQUIRED = ["dist/index.js", "dist/whatsapp_rust_bridge_bg.wasm"];
const missing = REQUIRED.filter((path) => !files.some((f) => f.path === path));
if (missing.length > 0) {
  console.error(
    `check-size: nothing to measure — ${missing.join(", ")} absent from the tarball.\n` +
      `check-size: run 'bun run build' first.`
  );
  process.exit(1);
}

for (const file of [...files].sort((a, b) => b.size - a.size).slice(0, 5)) {
  console.log(`check-size:   ${mb(file.size).padStart(8)}  ${file.path}`);
}

if (total > MAX_UNPACKED_BYTES) {
  console.error(
    `check-size: ${mb(total)} unpacked, over the ${mb(MAX_UNPACKED_BYTES)} budget ` +
      `by ${mb(total - MAX_UNPACKED_BYTES)}.\n` +
      `check-size: trim it, or raise MAX_UNPACKED_BYTES in scripts/check-size.ts ` +
      `in the commit that spends the bytes.`
  );
  // A dev build skips wasm-opt and lands around 40 MB, so it fails this by a
  // margin no real change produces. Say so rather than let someone go hunting.
  if (total > MAX_UNPACKED_BYTES * 3) {
    console.error(`check-size: that margin usually means a --dev wasm; measure a 'bun run build'.`);
  }
  process.exit(1);
}

console.log(
  `check-size: ${mb(total)} unpacked across ${files.length} files, ` +
    `${mb(MAX_UNPACKED_BYTES - total)} under budget`
);
