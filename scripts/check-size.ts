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
import { execSync } from "node:child_process";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");

/**
 * Headroom over 0.6.5's 7,133,345 bytes, at roughly five times the per-release
 * drift above — enough that ordinary work does not trip it, tight enough that
 * a jump does.
 */
const MAX_UNPACKED_BYTES = 7_500_000;

type PackedFile = { path: string; size: number };

const packed = JSON.parse(
  execSync("npm pack --dry-run --json --ignore-scripts", { cwd: ROOT, encoding: "utf8" })
);
const files: PackedFile[] = packed[0].files;
const total: number = packed[0].unpackedSize;

const mb = (bytes: number) => `${(bytes / 1_000_000).toFixed(2)} MB`;

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
