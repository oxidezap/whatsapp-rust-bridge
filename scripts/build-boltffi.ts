/** Builds and validates the optional Node BoltFFI artifact. */
import { existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const CRATE = join(ROOT, "crates", "bridge-boltffi");
const OUT = join(ROOT, "dist", "boltffi");
const PKG = join(OUT, "pkg");
const MODULE = "whatsapp_rust_bridge_boltffi";

// The macro, generator and runtime must use the same release.
const CRATE_MANIFEST = readFileSync(join(CRATE, "Cargo.toml"), "utf8");
const REQUIRED_CLI_VERSION = CRATE_MANIFEST.match(
  /^boltffi\s*=\s*\{[^}]*\bversion\s*=\s*"=([^"]+)"/m,
)?.[1];
if (REQUIRED_CLI_VERSION === undefined) {
  throw new Error("crates/bridge-boltffi/Cargo.toml must pin an exact BoltFFI release.");
}

// A skipped or rejected build must not leave an artifact from older sources.
rmSync(OUT, { recursive: true, force: true });

const CLI = Bun.which("boltffi");
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
if (version !== REQUIRED_CLI_VERSION) {
  throw new Error(
    `boltffi ${version} is installed, but this backend compiles against ` +
      `${REQUIRED_CLI_VERSION}. Mismatched versions emit JavaScript that looks ` +
      `for wasm exports the module does not have. Install it with ` +
      `\`cargo install boltffi_cli --version ${REQUIRED_CLI_VERSION} --locked\`.`,
  );
}
const runtimeVersion = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8"))
  .dependencies["@boltffi/runtime"];
const installedRuntime = JSON.parse(readFileSync(
  join(ROOT, "node_modules", "@boltffi", "runtime", "package.json"), "utf8",
)).version;
if (runtimeVersion !== REQUIRED_CLI_VERSION || installedRuntime !== runtimeVersion) {
  throw new Error(`BoltFFI crate/CLI ${REQUIRED_CLI_VERSION}, runtime pin ${runtimeVersion}, ` +
    `installed runtime ${installedRuntime} must all match; run bun install after updating the pins.`);
}

// Fail instead of shipping bindings with unsupported declarations omitted.
const packed = Bun.spawnSync({
  cmd: [CLI, "pack", "wasm", "--release", "--deny-skipped"],
  cwd: CRATE,
  stdout: "pipe",
  stderr: "pipe",
});
process.stdout.write(`${packed.stdout.toString()}${packed.stderr.toString()}`);
if (packed.exitCode !== 0) {
  throw new Error(`boltffi pack wasm failed with ${packed.exitCode}`);
}

// The emitted `.ts` are inputs to the `.js`/`.d.ts` beside them; shipping both
// would put two copies of the surface in the tarball.
for (const file of readdirSync(PKG)) {
  if (file.endsWith(".ts") && !file.endsWith(".d.ts")) rmSync(join(PKG, file));
}

// The Node-only pack also emits an unused browser loader and its source map.
// Keep declarations (the generated package references them), but do not ship
// a second loader that none of this target's entrypoints imports.
const config = Bun.TOML.parse(readFileSync(join(CRATE, "boltffi.toml"), "utf8")) as any;
if (JSON.stringify(config.targets.wasm.npm.targets) === '["nodejs"]') {
  const unused = new Set([`${MODULE}.js`, `${MODULE}.js.map`]);
  for (const file of unused) rmSync(join(PKG, file), { force: true });
  const packagePath = join(PKG, "package.json");
  const metadata = JSON.parse(readFileSync(packagePath, "utf8"));
  metadata.files = metadata.files.filter((file: string) => !unused.has(file));
  writeFileSync(packagePath, JSON.stringify(metadata, null, 2) + "\n");
}

const required = [`${MODULE}_bg.wasm`, `${MODULE}_node.js`, `${MODULE}_node.d.ts`, "node.js"];
const missing = required.filter((name) => !existsSync(join(PKG, name)));
if (missing.length > 0) {
  throw new Error(`boltffi pack did not emit: ${missing.join(", ")}`);
}

// Importing catches ABI mismatches that file and version checks cannot prove absent.
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
