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
import { existsSync, readFileSync, rmSync } from "node:fs";
import { homedir } from "node:os";
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

/**
 * The generator and the `boltffi` macro agree on wasm symbol names only within
 * a revision. Mixing them produces a module whose exports the emitted
 * JavaScript looks for and does not find — at runtime, not at build time.
 *
 * `boltffi --version` reports the crate version, which is the same string on
 * either side of the pin, so the version alone cannot catch the case that
 * matters. What can is cargo's own install record: a `cargo install --git …
 * --rev X` writes that revision into `$CARGO_HOME/.crates.toml`.
 */
const REQUIRED_CLI_VERSION = "0.29.3";
const PINNED_REV = readFileSync(join(CRATE, "Cargo.toml"), "utf8").match(
  /rev\s*=\s*"([0-9a-f]{7,40})"/,
)?.[1];

// Cleared before anything can bail out. A skipped or rejected build that left
// the previous `dist/boltffi` behind would hand the tests and `check-pack` an
// artifact built from older Rust sources, with nothing to say it was stale.
rmSync(OUT, { recursive: true, force: true });

const available = Bun.spawnSync({
  cmd: ["boltffi", "--version"],
  stdout: "pipe",
  stderr: "pipe",
});
if (available.exitCode !== 0) {
  console.log("boltffi not installed — skipping the BoltFFI artifact");
  process.exit(0);
}

const version = available.stdout.toString().trim().split(/\s+/).pop();
if (version !== REQUIRED_CLI_VERSION) {
  throw new Error(
    `boltffi ${version} is installed, but this backend compiles against ` +
      `${REQUIRED_CLI_VERSION}. Mismatched versions emit JavaScript that looks ` +
      `for wasm exports the module does not have. ` +
      `Install it from the revision the workflows pin — see the ` +
      `\`boltffi\` dependency in crates/bridge-boltffi/Cargo.toml.`,
  );
}

// The record exists only for a `cargo install`. A CLI put on PATH some other
// way leaves nothing to compare against, so that stays a warning — CI installs
// with `--git --rev`, which is the case this has to catch.
const installedRev = (() => {
  const record = join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), ".crates.toml");
  if (!existsSync(record)) return null;
  const entry = readFileSync(record, "utf8").match(/^"boltffi_cli [^"]*"/m)?.[0];
  if (entry === undefined) return null;
  // `?rev=` is what was asked for; the `#` fragment is the commit cargo
  // resolved it to. Either identifies the revision.
  return {
    git: /\bgit\+/.test(entry),
    rev: (entry.match(/[?&]rev=([0-9a-f]{7,40})/) ?? entry.match(/#([0-9a-f]{7,40})"/))?.[1],
  };
})();
// Cargo may record a short revision where the manifest pins a full one, or the
// reverse, so they agree when one is a prefix of the other.
const sameRevision = (a: string, b: string) =>
  a.startsWith(b.slice(0, Math.min(a.length, b.length)));
if (PINNED_REV === undefined) {
  console.warn(
    "no `rev` in crates/bridge-boltffi/Cargo.toml — the installed CLI cannot be " +
      "checked against the pin.",
  );
} else if (installedRev === null || !installedRev.git) {
  console.warn(
    `boltffi was not installed by \`cargo install --git\`, so its revision is ` +
      `unknown. This backend compiles against ${PINNED_REV}; a CLI from any ` +
      `other revision emits JavaScript for wasm exports the module lacks.`,
  );
} else if (installedRev.rev === undefined || !sameRevision(PINNED_REV, installedRev.rev)) {
  throw new Error(
    `boltffi was installed from revision ${installedRev.rev ?? "(unrecorded)"}, ` +
      `but this backend compiles against ${PINNED_REV}. Reinstall with ` +
      `\`cargo install --git https://github.com/boltffi/boltffi --rev ${PINNED_REV} ` +
      `boltffi_cli --locked\`.`,
  );
}

// `--deny-skipped` makes the generator exit non-zero on a declaration it
// cannot render. Without it `pack` prints the skips as a table and still exits
// 0, and a surface that quietly loses an operation is the failure mode this
// backend is most exposed to.
const packed = Bun.spawnSync({
  cmd: ["boltffi", "pack", "wasm", "--release", "--deny-skipped"],
  cwd: CRATE,
  stdout: "pipe",
  stderr: "pipe",
  env: {
    ...process.env,
    CARGO_BUILD_TARGET: HOST_TARGET,
    PATH: `${join(ROOT, "scripts", "boltffi-tsc-shim")}:${process.env.PATH}`,
  },
});
process.stdout.write(`${packed.stdout.toString()}${packed.stderr.toString()}`);
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
