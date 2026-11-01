/**
 * Seam gate: the control-only build must carry the call-control plane without
 * the media engine.
 *
 * The default core artifact carries `client-calls-media` over the neutral
 * `client-voip-control` seam, not the resident engine. The three opt-in
 * engine carriers (`client-calls-audio`, `client-calls-pcm`,
 * `client-calls-mlow`) stay out of this scan. Release strips names, so the
 * gate builds a names-kept dev artifact from the default feature set.
 *
 * Three legs, all of which have been seen to fail:
 *
 * 1. `cargo tree` on the control-only set resolves `whatsapp-rust/voip-control`
 *    and `wacore/voip-control`, and no engine feature (`wacore/voip`,
 *    `voip-engine-wacore`, `voip-runtime`, `voip-encoded`, `voip-mlow`,
 *    `voip-libopus`, `voip`). Removing the explicit bridge feature, or wiring
 *    an engine carrier back in, fails here.
 * 2. The default dev artifact (names kept: dev profile never strips)
 *    carries no engine code symbol in its `name` custom section. An opt-in
 *    resident-engine build carries them; a stripped release artifact carries
 *    no names either way, so this scan refuses an artifact without a section.
 * 3. The control-only dependency set is a subset of the default one: enabling
 *    the seam explicitly must not add a runtime crate beyond
 *    `whatsapp-rust/voip-control` (which was already on transitively).
 *
 *   node scripts/check-voip-control-seam.mjs [artifact.wasm]
 *
 * An explicit artifact path skips the build and scans that file: it is how
 * the gate itself is validated (a default-features dev artifact must fail
 * leg 2) and how a human re-checks a stale build without recompiling.
 *
 * Builds the control-only artifact itself (dev profile, `target/` cache makes
 * repeats incremental), so CI needs only Rust and Node — no wasm-bindgen, no
 * wasm-opt.
 */
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");

// The default domains and the neutral seam, not the opt-in resident engine.
// This list is the gate: drift it and leg 1 or leg 2 says so.
const CONTROL_FEATURES = [
  "client-business",
  "client-calls",
  "client-calls-media",
  "client-chat-actions",
  "client-contacts",
  "client-groups",
  "client-media",
  "client-newsletter",
  "client-signal",
  "client-voip-control",
  "legacy-session",
];

const ENGINE_FEATURES = new Set([
  "voip",
  "voip-encoded",
  "voip-engine-wacore",
  "voip-libopus",
  "voip-mlow",
  "voip-runtime",
]);

// Code entities that exist only in the media engine. Matched against the
// `name` custom section, which holds code-derived function names — never
// strings, panics, or doc text — so a hit means engine code linked in.
const FORBIDDEN_SYMBOLS = [
  "CallEngine",
  "run_call",
  "SframeSession",
  "MlowEncoder",
  "MlowDecoder",
  "GroupMediaRegistry",
  "MediaPipeline",
];

function sh(cmd, args) {
  return execFileSync(cmd, args, { cwd: root, encoding: "utf8" });
}

function featureTokens(line) {
  const match = /\)\s+(.*)$/.exec(line.trim());
  if (!match) return [];
  return match[1].split(",").filter((t) => t.length > 0);
}

function findLine(tree, name) {
  const line = tree
    .split("\n")
    .find((l) => l.includes(`${name} v`) || l.includes(`${name} (`));
  if (!line) throw new Error(`check-voip-control-seam: ${name} missing from cargo tree`);
  return line;
}

// --- leg 1: the resolved feature graph -------------------------------------

const tree = sh("cargo", [
  "tree",
  "-e",
  "no-dev",
  "--no-default-features",
  "--features",
  CONTROL_FEATURES.join(","),
  "-f",
  "{p} {f}",
]);

const coreLine = findLine(tree, "whatsapp-rust");
const wacoreLine = findLine(tree, "wacore");
const coreFeatures = featureTokens(coreLine);
const wacoreFeatures = featureTokens(wacoreLine);

let failed = false;
const fail = (msg) => {
  console.error(`check-voip-control-seam: ${msg}`);
  failed = true;
};

if (!coreFeatures.includes("voip-control")) {
  fail(`whatsapp-rust lacks voip-control (has: ${coreFeatures.join(",")})`);
}
if (!wacoreFeatures.includes("voip-control")) {
  fail(`wacore lacks voip-control (has: ${wacoreFeatures.join(",")})`);
}
for (const token of [...coreFeatures, ...wacoreFeatures]) {
  if (token === "voip" || ENGINE_FEATURES.has(token)) {
    fail(`engine feature ${JSON.stringify(token)} resolved on the control-only graph`);
  }
}
// `wacore/voip` would appear as a bare `voip` token on the wacore line; the
// loop above covers it, but the name is worth asserting directly.
if (wacoreFeatures.includes("voip")) {
  fail("wacore/voip resolved on the control-only graph");
}
console.log(
  `check-voip-control-seam: graph ok (whatsapp-rust: ${coreFeatures.join(",")}; wacore: ${wacoreFeatures.join(",")})`
);

// --- leg 3: no new runtime crate --------------------------------------------

function packageNames(extraArgs) {
  const out = sh("cargo", ["tree", "-e", "no-dev", ...extraArgs, "--prefix", "none"]);
  const names = new Set();
  for (const line of out.split("\n")) {
    const match = /^(\S+) v\S+/.exec(line.trim());
    if (match) names.add(match[1]);
  }
  return names;
}

const controlPkgs = packageNames(["--no-default-features", "--features", CONTROL_FEATURES.join(",")]);
const defaultPkgs = packageNames([]);
const added = [...controlPkgs].filter((p) => !defaultPkgs.has(p));
if (added.length > 0) {
  fail(`control-only graph adds crates beyond default: ${added.join(", ")}`);
}
console.log(
  `check-voip-control-seam: deps ok (${controlPkgs.size} control-only crates, all within default's ${defaultPkgs.size})`
);

// --- leg 2: no engine code in the control-only artifact ---------------------

const artifactArg = process.argv[2];
const artifact = artifactArg
  ? join(root, artifactArg)
  : join(root, "target/wasm32-unknown-unknown/debug/whatsapp_rust_bridge.wasm");
if (!artifactArg) {
  // Warnings denied: this configuration compiles in no other gate, so a
  // helper the engine carriers took with them must fail here rather than
  // rot. (Only the local crate is affected; dependencies build capped.)
  process.env.RUSTFLAGS = `${process.env.RUSTFLAGS ?? ""} -D warnings`;
  sh("cargo", [
    "build",
    "--locked",
    "--target",
    "wasm32-unknown-unknown",
    "--no-default-features",
    "--features",
    CONTROL_FEATURES.join(","),
  ]);
}
if (!existsSync(artifact)) {
  fail(`build produced no artifact at ${artifact}`);
  process.exit(1);
}

const bytes = readFileSync(artifact);

// Minimal section walk: magic + version, then (id, size, payload) triples.
// Collects the payload of every custom section named "name" and refuses an
// artifact that has none — a stripped build would pass vacuously.
function readU32LEB(buf, pos) {
  let result = 0;
  let shift = 0;
  let i = pos;
  for (;;) {
    const byte = buf[i++];
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) break;
    shift += 7;
  }
  return [result, i];
}

function nameSectionPayloads(buf) {
  if (buf[0] !== 0x00 || buf[1] !== 0x61 || buf[2] !== 0x73 || buf[3] !== 0x6d) {
    throw new Error("not a wasm module");
  }
  let pos = 8;
  const payloads = [];
  let sawNameSection = false;
  while (pos < buf.length) {
    const id = buf[pos++];
    const [size, next] = readU32LEB(buf, pos);
    pos = next;
    const end = pos + size;
    if (id === 0) {
      const [nameLen, nameStart] = readU32LEB(buf, pos);
      const name = Buffer.from(buf.subarray(nameStart, nameStart + nameLen)).toString("utf8");
      if (name === "name") {
        sawNameSection = true;
        payloads.push(buf.subarray(pos, end));
      }
    }
    pos = end;
  }
  return { payloads, sawNameSection };
}

const { payloads, sawNameSection } = nameSectionPayloads(bytes);
if (!sawNameSection) {
  fail(
    "artifact carries no name section — a stripped build passes vacuously. " +
      "Scan the dev-profile control-only build, which keeps names."
  );
  process.exit(1);
}

const haystacks = payloads.map((p) => Buffer.from(p).toString("binary"));
for (const symbol of FORBIDDEN_SYMBOLS) {
  const hits = haystacks.reduce(
    (n, h) => n + (h.split(symbol).length - 1),
    0
  );
  console.log(`check-voip-control-seam: symbol ${symbol}: ${hits} hit(s)`);
  if (hits > 0) {
    fail(`engine symbol ${JSON.stringify(symbol)} linked into the control-only artifact (${hits} hits)`);
  }
}

if (failed) {
  console.error("check-voip-control-seam: FAILED");
  process.exit(1);
}
console.log("check-voip-control-seam: control plane without engine, ok");
