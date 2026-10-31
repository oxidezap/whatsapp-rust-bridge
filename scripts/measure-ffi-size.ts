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
run(["wasm-bindgen", "--target", "web", "--out-dir", bindgen, join(root, "target/wasm32-unknown-unknown/release/ffi_size_bindgen.wasm")]);
// Match the production bridge's web bindings plus a synchronous Node file loader.
writeFileSync(join(bindgen, "node.js"), `import { readFileSync } from "node:fs";
import { initSync } from "./ffi_size_bindgen.js";
initSync({ module: readFileSync(new URL("./ffi_size_bindgen_bg.wasm", import.meta.url)) });
export * from "./ffi_size_bindgen.js";
`);
const bolt = join(out, "boltffi");
cpSync(join(root, "dist/boltffi/pkg"), bolt, { recursive: true });
// Resolve the runtime from the repository when bundling the copied package.
const metrics = (data: Uint8Array) => ({ raw: data.byteLength,
  gzip: gzipSync(data, { level: 9 }).byteLength,
  brotli: brotliCompressSync(data, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).byteLength });
async function validateRuntime(bindings: Record<string, any>) {
  const { hkdfSync, createPrivateKey, createPublicKey, diffieHellman } = await import("node:crypto");
  const sameBytes = (actual: Uint8Array, expected: Uint8Array, name: string) => {
    if (Buffer.from(actual).toString("hex") !== Buffer.from(expected).toString("hex")) {
      throw new Error(`${name} differs from the independent expected result`);
    }
  };
  const secret = new Uint8Array(32).fill(0x42);
  secret[0] &= 248; secret[31] = (secret[31] & 127) | 64;
  const other = new Uint8Array(32).fill(0x24);
  other[0] &= 248; other[31] = (other[31] & 127) | 64;
  const key = (bytes: Uint8Array) => createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b656e04220420", "hex"), Buffer.from(bytes)]),
    format: "der", type: "pkcs8",
  });
  const publicKey = bindings.getPublicFromPrivateKey(secret);
  const expectedPublic = createPublicKey(key(secret)).export({ format: "der", type: "spki" }).subarray(-32);
  sameBytes(publicKey, new Uint8Array([5, ...expectedPublic]), "public key");
  sameBytes(bindings.calculateAgreement(bindings.getPublicFromPrivateKey(other), secret),
    diffieHellman({ privateKey: key(secret), publicKey: createPublicKey(key(other)) }), "agreement");
  sameBytes(bindings.md5(new Uint8Array()), Buffer.from("d41d8cd98f00b204e9800998ecf8427e", "hex"), "MD5");
  const salt = new Uint8Array(16).fill(1);
  sameBytes(bindings.hkdf(secret, 64, { salt, info: "wa" }),
    new Uint8Array(hkdfSync("sha256", secret, salt, Buffer.from("wa"), 64)), "HKDF");
  if (bindings.verifySignature(publicKey, new Uint8Array([1, 2, 3]), new Uint8Array(64)) !== false) {
    throw new Error("invalid signature was accepted");
  }
  const compressed = new Uint8Array([120,156,203,72,205,201,201,7,0,6,44,2,21]);
  sameBytes(bindings.inflateZlib(compressed, null), new TextEncoder().encode("hello"), "inflate");
  const reject = (name: string, args: unknown[]) => {
    let rejected = false;
    try { bindings[name](...args); } catch { rejected = true; }
    if (!rejected) throw new Error(`${name} accepted malformed input`);
  };
  reject("inflateZlib", [compressed, 4]);
  for (const name of ["decryptPollVotePayload", "decryptEventResponsePayload"]) {
    reject(name, [new Uint8Array([1,2,3]), new Uint8Array(12), new Uint8Array(3),
      "stanza", "5511@s.whatsapp.net", "5512@s.whatsapp.net"]);
  }
  return { md5: true, hkdf: true, publicKey: true, agreement: true,
    invalidSignatureRejected: true, inflateAndLimit: true, malformedAuthenticatedPayloadsRejected: true };
}

const rows: Record<string, unknown> = {};
for (const [name, dir, wasm, entry, compiler] of [
  ["wasm-bindgen", bindgen, "ffi_size_bindgen_bg.wasm", "node.js", "ffi_size_bindgen.wasm"],
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
  const bunChecks = await validateRuntime(bindings);
  const nodeChecks = JSON.parse(run(["node", "--input-type=module", "-e", `
    const m = await import(${JSON.stringify(bundle)});
    const api = typeof m.md5 === "function" ? m : m.default;
    console.log(JSON.stringify(await (${validateRuntime.toString()})(api)));
  `]));
  const wasmSize = metrics(readFileSync(path));
  const js = metrics(readFileSync(bundle));
  const declarationFiles = readdirSync(dir!).filter(f => f.endsWith(".d.ts")).map(f => metrics(readFileSync(join(dir!, f))));
  const declarations = Object.fromEntries(["raw", "gzip", "brotli"].map(key =>
    [key, declarationFiles.reduce((sum, sizes) => sum + sizes[key as keyof typeof sizes], 0)]));
  rows[name!] = {
    runtimeChecks: { bun: bunChecks, node: nodeChecks },
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
  targets: { bindgen: "web bindings + production-style synchronous Node ESM loader", boltffi: "generated Node ESM loader" },
  wasmOptFlags: flags, compression: "gzip level 9 / Brotli quality 11; per-file sizes summed", rows, commands };
writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify(report, null, 2));
