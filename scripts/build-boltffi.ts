/**
 * Builds the BoltFFI artifact into `dist/boltffi/pkg/`.
 *
 * `boltffi pack wasm` does the work; two things have to be arranged around it,
 * and both are properties of this repository rather than of BoltFFI:
 *
 * - `CARGO_BUILD_TARGET` is pinned to the host. The root `.cargo/config.toml`
 *   sets `build.target = wasm32-unknown-unknown` for every cargo invocation,
 *   and BoltFFI's binding-metadata step builds for the host and parses the
 *   resulting object file — inheriting that pin makes it read a wasm module and
 *   fail with "Unknown file magic". The wasm build passes `--target` itself, so
 *   it still targets wasm32. A nearer `.cargo/config.toml` cannot undo it:
 *   cargo has no syntax for unsetting an inherited `build.target`.
 * - A `tsc` shim goes first on `PATH`. BoltFFI names files on the tsc command
 *   line, which TypeScript 6 refuses while a `tsconfig.json` is in scope
 *   (TS5112); the shim adds `--ignoreConfig --types node`.
 *
 * Skipped when `boltffi` is absent, so the default build still works for a
 * contributor who does not have it installed.
 */
import { existsSync, rmSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const CRATE = join(ROOT, "crates", "bridge-boltffi");
const OUT = join(ROOT, "dist", "boltffi");
const PKG = join(OUT, "pkg");
const MODULE = "whatsapp_rust_bridge_boltffi";

const HOST_TARGET =
  process.env.BOLTFFI_HOST_TARGET ??
  Bun.spawnSync({ cmd: ["rustc", "-vV"] })
    .stdout.toString()
    .match(/^host:\s*(\S+)$/m)?.[1] ??
  "x86_64-unknown-linux-gnu";

const available = Bun.spawnSync({
  cmd: ["boltffi", "--version"],
  stdout: "pipe",
  stderr: "pipe",
});
if (available.exitCode !== 0) {
  console.log("boltffi not installed — skipping the BoltFFI artifact");
  process.exit(0);
}

rmSync(OUT, { recursive: true, force: true });

const packed = Bun.spawnSync({
  cmd: ["boltffi", "pack", "wasm", "--release"],
  cwd: CRATE,
  stdout: "inherit",
  stderr: "inherit",
  env: {
    ...process.env,
    CARGO_BUILD_TARGET: HOST_TARGET,
    PATH: `${join(ROOT, "scripts", "boltffi-tsc-shim")}:${process.env.PATH}`,
  },
});
if (packed.exitCode !== 0) {
  throw new Error(`boltffi pack wasm failed with ${packed.exitCode}`);
}

// The emitted `.ts` are inputs to the `.js`/`.d.ts` beside them; shipping both
// would put two copies of the surface in the tarball.
for (const stale of [`${MODULE}.ts`, `${MODULE}_node.ts`]) {
  rmSync(join(PKG, stale), { force: true });
}

const required = [`${MODULE}_bg.wasm`, `${MODULE}_node.js`, `${MODULE}_node.d.ts`, "node.js"];
const missing = required.filter((name) => !existsSync(join(PKG, name)));
if (missing.length > 0) {
  throw new Error(`boltffi pack did not emit: ${missing.join(", ")}`);
}

console.log(
  `BoltFFI artifact written to dist/boltffi/pkg ` +
    `(${Bun.file(join(PKG, `${MODULE}_bg.wasm`)).size} bytes of wasm)`,
);
