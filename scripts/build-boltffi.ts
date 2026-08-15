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

// `Bun.spawnSync` throws rather than reporting a non-zero exit when the
// executable is not on PATH, so every command this script probes for goes
// through `Bun.which` first. Reaching `exitCode` on a missing binary never
// happens — the throw comes first, and it turns a supported skip into a crash.
const resolve = (command: string) => Bun.which(command);

const RUSTC = resolve("rustc");
const HOST_TARGET =
  process.env.BOLTFFI_HOST_TARGET ??
  (RUSTC === null
    ? undefined
    : Bun.spawnSync({ cmd: [RUSTC, "-vV"] })
        .stdout.toString()
        .match(/^host:\s*(\S+)$/m)?.[1]) ??
  "x86_64-unknown-linux-gnu";

/**
 * The generator and the `boltffi` macro agree on wasm symbol names only within
 * a version. Mixing them produces a module whose exports the emitted
 * JavaScript looks for and does not find — at runtime, not at build time.
 *
 * Both sides are read from the manifest rather than repeated here, so moving
 * the pin is one edit. While this pinned a main revision, the version could not
 * tell the two apart — every revision reported the same `0.29.3` — and the
 * revision from cargo's install record was the only usable fingerprint. On a
 * released version the version string is the fingerprint again; the revision
 * check stays for whenever the pin goes back to a `rev`.
 */
const CRATE_MANIFEST = readFileSync(join(CRATE, "Cargo.toml"), "utf8");
const REQUIRED_CLI_VERSION = CRATE_MANIFEST.match(
  /^boltffi\s*=\s*\{[^}]*\bversion\s*=\s*"([^"]+)"/m,
)?.[1];
const PINNED_REV = CRATE_MANIFEST.match(/^boltffi\s*=\s*\{[^}]*\brev\s*=\s*"([0-9a-f]{7,40})"/m)?.[1];
if (REQUIRED_CLI_VERSION === undefined && PINNED_REV === undefined) {
  throw new Error(
    "crates/bridge-boltffi/Cargo.toml pins `boltffi` by neither version nor " +
      "revision, so the installed CLI cannot be checked against it.",
  );
}

// Cleared before anything can bail out. A skipped or rejected build that left
// the previous `dist/boltffi` behind would hand the tests and `check-pack` an
// artifact built from older Rust sources, with nothing to say it was stale.
rmSync(OUT, { recursive: true, force: true });

// The record exists only for a `cargo install`. A CLI put on PATH some other
// way leaves nothing to compare against, so that stays a warning — CI installs
// with `--git --rev`, which is the case this has to catch.
const CARGO_HOME = process.env.CARGO_HOME ?? join(homedir(), ".cargo");
const installedRev = (() => {
  const record = join(CARGO_HOME, ".crates.toml");
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

// The record describes the binary cargo installed, not whatever `boltffi`
// happens to resolve to first on PATH. Checking one and running the other
// would pass while packing with an unrelated build, so when the record vouches
// for a binary, that binary is the one invoked.
const CARGO_BIN = join(CARGO_HOME, "bin", "boltffi");
const CLI =
  installedRev !== null && existsSync(CARGO_BIN) ? CARGO_BIN : resolve("boltffi");
if (CLI === null) {
  console.log("boltffi not installed — skipping the BoltFFI artifact");
  process.exit(0);
}

const available = Bun.spawnSync({
  cmd: [CLI, "--version"],
  stdout: "pipe",
  stderr: "pipe",
});
if (available.exitCode !== 0) {
  console.log("boltffi not installed — skipping the BoltFFI artifact");
  process.exit(0);
}

const version = available.stdout.toString().trim().split(/\s+/).pop();
if (REQUIRED_CLI_VERSION !== undefined && version !== REQUIRED_CLI_VERSION) {
  throw new Error(
    `boltffi ${version} is installed, but this backend compiles against ` +
      `${REQUIRED_CLI_VERSION}. Mismatched versions emit JavaScript that looks ` +
      `for wasm exports the module does not have. Install it with ` +
      `\`cargo install boltffi_cli --version ${REQUIRED_CLI_VERSION} --locked\`.`,
  );
}
// Cargo may record a short revision where the manifest pins a full one, or the
// reverse, so they agree when one is a prefix of the other.
const sameRevision = (a: string, b: string) =>
  a.startsWith(b.slice(0, Math.min(a.length, b.length)));
if (PINNED_REV === undefined) {
  // Pinned by version, which the check above already enforced.
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
  cmd: [CLI, "pack", "wasm", "--release", "--deny-skipped"],
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

/**
 * Loading it is the last check, and the only one that can catch the failure
 * this backend actually hits.
 *
 * The module declares a wasm ABI version and `@boltffi/runtime` refuses to
 * instantiate anything else, so a `package.json` pinning a runtime from a
 * different release than `crates/bridge-boltffi/Cargo.toml` produces an
 * artifact that packs, passes every file check, and then dies on import with
 * `BoltFFI ABI version mismatch`. Nothing else on the publish path opens it:
 * `prepublishOnly` runs the build, `prepack` runs `check-pack`, and neither
 * imports the result. The suites do, but a publish does not run them.
 */
const smoke = Bun.spawnSync({
  cmd: [process.execPath, "-e", `await import(${JSON.stringify(join(PKG, "node.js"))})`],
  stdout: "pipe",
  stderr: "pipe",
});
if (smoke.exitCode !== 0) {
  throw new Error(
    `the artifact does not load. The \`@boltffi/runtime\` version in ` +
      `package.json and the \`boltffi\` pin in crates/bridge-boltffi/Cargo.toml ` +
      `have to come from the same release.\n${smoke.stderr.toString()}`,
  );
}

console.log(
  `BoltFFI artifact written to dist/boltffi/pkg ` +
    `(${Bun.file(join(PKG, `${MODULE}_bg.wasm`)).size} bytes of wasm)`,
);
