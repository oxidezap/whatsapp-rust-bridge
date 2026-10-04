/** Compare the same eight utilities with both FFI generators. Builds both artifacts. */
import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

const root = resolve(import.meta.dir, "..");
const out = join(root, "target", "ffi-size");
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const commands: string[][] = [];
function run(cmd: string[], cwd = root): string {
  commands.push(cmd);
  console.error(cmd.join(" "));
  const result = Bun.spawnSync(cmd, { cwd, stdout: "pipe", stderr: "inherit" });
  if (result.exitCode !== 0) throw new Error(`${cmd[0]} failed: ${result.exitCode}`);
  return result.stdout.toString().trim();
}
const api = ["md5", "hkdf", "getPublicFromPrivateKey", "calculateAgreement", "verifySignature",
  "inflateZlib", "decryptPollVotePayload", "decryptEventResponsePayload"];
// Fail if the fixture's copied wrappers no longer match the production boundary.
const production = readFileSync(join(root, "src/crypto.rs"), "utf8");
const fixture = readFileSync(join(root, "benches/ffi-size-bindgen/src/lib.rs"), "utf8");
function productionDeclaration(marker: string, attribute: string): string {
  const declaration = production.indexOf(marker);
  if (declaration < 0) throw new Error(`missing production declaration: ${marker}`);
  const start = production.lastIndexOf(attribute, declaration);
  const end = production.indexOf("\n}", declaration);
  if (start < 0 || end < 0) throw new Error(`incomplete production declaration: ${marker}`);
  return production.slice(start, end + 2);
}
for (const name of ["md5_digest", "hkdf_sha256", "public_from_private_key", "calculate_agreement", "verify_signature"]) {
  if (!fixture.includes(productionDeclaration(`pub fn ${name}(`, "#[wasm_bindgen"))) {
    throw new Error(`stale size fixture: ${name}`);
  }
}
if (!fixture.includes(productionDeclaration("pub struct HkdfInfo", "#[derive"))) {
  throw new Error("stale size fixture: HkdfInfo");
}
const manifest = Bun.TOML.parse(readFileSync(join(root, "Cargo.toml"), "utf8")) as any;
const flags: string[] = manifest.package.metadata["wasm-pack"].profile.release["wasm-opt"];
const versions = Object.fromEntries(["rustc", "wasm-bindgen", "wasm-opt", "boltffi", "bun", "node"].map(
  (tool) => [tool, run([tool, "--version"])],
));
const coreFeatures = Object.fromEntries(["ffi-size-bindgen", "whatsapp-rust-bridge-boltffi"].map(pkg =>
  [pkg, [...new Set(run(["cargo", "tree", "-p", pkg, "--target", "wasm32-unknown-unknown",
    "--prefix", "none", "--format", "{p}|{f}"]).split("\n")
    .filter(line => /^(whatsapp-rust v|wacore(?:-[\w-]+)? v)/.test(line))
    .map(line => line.replace(/ \(\*\)$/, "")))].sort()]));
if (JSON.stringify(coreFeatures["ffi-size-bindgen"]) !== JSON.stringify(coreFeatures["whatsapp-rust-bridge-boltffi"])) {
  throw new Error("size comparison has different core feature sets");
}
run(["bun", "run", "build:boltffi"]);
run(["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "-p", "ffi-size-bindgen"]);
const bindgen = join(out, "bindgen");
mkdirSync(bindgen, { recursive: true });
run(["wasm-bindgen", "--target", "nodejs", "--out-dir", bindgen, join(root, "target/wasm32-unknown-unknown/release/ffi_size_bindgen.wasm")]);
const bolt = join(out, "boltffi");
cpSync(join(root, "dist/boltffi/pkg"), bolt, { recursive: true });
// Resolve the runtime from the repository when bundling the copied package.
const metrics = (data: Uint8Array) => ({ raw: data.byteLength,
  gzip: gzipSync(data, { level: 9 }).byteLength,
  brotli: brotliCompressSync(data, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).byteLength });
const rows: Record<string, unknown> = {};
for (const [name, dir, wasm, entry, compiler] of [
  ["wasm-bindgen", bindgen, "ffi_size_bindgen_bg.wasm", "ffi_size_bindgen.js", "ffi_size_bindgen.wasm"],
  ["BoltFFI", bolt, "whatsapp_rust_bridge_boltffi_bg.wasm", "node.js", "whatsapp_rust_bridge_boltffi.wasm"],
]) {
  const path = join(dir!, wasm!);
  const before = metrics(readFileSync(path));
  run(["wasm-opt", path, ...flags, "-o", path]);
  const jsFiles = readdirSync(dir!).filter(f => f.endsWith(".js"));
  const generatedJS = Object.fromEntries(jsFiles.map(f => [f, metrics(readFileSync(join(dir!, f)))]));
  const bundle = join(dir!, "bundle.mjs");
  run(["bun", "build", join(dir!, entry!), "--target", "node", "--format", "esm", "--minify", "--outfile", bundle]);
  // Keep all emitted exports; do not tree-shake to a single digest function.
  const imported = await import(bundle);
  const bindings = typeof imported.md5 === "function" ? imported : imported.default;
  for (const fn of api) if (typeof bindings[fn] !== "function") throw new Error(`${name} omitted ${fn}`);
  if (Buffer.from(bindings.md5(new Uint8Array())).toString("hex") !== "d41d8cd98f00b204e9800998ecf8427e") throw new Error(`${name} smoke failed`);
  run(["node", "--input-type=module", "-e", `
    const m = await import(${JSON.stringify(bundle)});
    const api = typeof m.md5 === "function" ? m : m.default;
    if (Buffer.from(api.md5(new Uint8Array())).toString("hex") !== "d41d8cd98f00b204e9800998ecf8427e") throw new Error("Node smoke failed");
  `]);
  const wasmSize = metrics(readFileSync(path));
  const js = metrics(readFileSync(bundle));
  const declarationFiles = readdirSync(dir!).filter(f => f.endsWith(".d.ts")).map(f => metrics(readFileSync(join(dir!, f))));
  const declarations = Object.fromEntries(["raw", "gzip", "brotli"].map(key =>
    [key, declarationFiles.reduce((sum, sizes) => sum + sizes[key as keyof typeof sizes], 0)]));
  rows[name!] = {
    compilerWasm: metrics(readFileSync(join(root, "target/wasm32-unknown-unknown/release", compiler!))),
    postBindingsWasm: before, optimizedWasm: wasmSize, generatedJS, bundledJSIncludingRuntime: js,
    postBindingsRuntimeDistributable: Object.fromEntries(Object.keys(js).map(key => [key,
      js[key as keyof typeof js] + before[key as keyof typeof before]])),
    runtimeDistributable: Object.fromEntries(Object.keys(js).map(key => [key, js[key as keyof typeof js] + wasmSize[key as keyof typeof wasmSize]])),
    declarations,
    typedDistributable: Object.fromEntries(Object.keys(js).map(key => [key,
      js[key as keyof typeof js] + wasmSize[key as keyof typeof wasmSize] + declarations[key]!])),
  };
}
const report = { bridge: run(["git", "rev-parse", "HEAD"]), core: manifest.workspace.dependencies["whatsapp-rust"],
  versions, api, coreFeatures, rustflags: readFileSync(join(root, ".cargo/config.toml"), "utf8"), profile: manifest.profile.release,
  wasmOptFlags: flags, compression: "gzip level 9 / Brotli quality 11; per-file sizes summed", rows, commands };
writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify(report, null, 2));
