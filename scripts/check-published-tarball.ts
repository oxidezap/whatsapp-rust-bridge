/**
 * Isolated-tarball contract check: pack the built package, install it in a
 * fresh directory outside the repository — so no parent `node_modules` can
 * leak in — and prove an ordinary consumer typechecks in both Bundler and
 * NodeNext modes and runs the real exports, using the package plus explicit
 * TypeScript/Node tooling. Published declarations reference
 * `@bufbuild/protobuf/wire`, so strict consumer checks prove the packed
 * manifest supplies that dependency rather than relying on this checkout.
 *
 * Run: `bun run check:published-tarball` (needs `dist/`, i.e. a build first).
 * Pack plus two installs plus two typechecks is too heavy for the per-test
 * clock gate, so CI runs this as its own job.
 */
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

const ROOT = join(import.meta.dir, "..");
const DIST_ENTRY = join(ROOT, "dist", "index.d.ts");

if (!existsSync(DIST_ENTRY)) {
  throw new Error("dist/index.d.ts is absent — run a build first");
}

interface Manifest {
  name: string;
  devDependencies?: Record<string, string>;
  dependencies?: Record<string, string>;
}

const manifest = JSON.parse(
  readFileSync(join(ROOT, "package.json"), "utf8"),
) as Manifest;

let failed = false;

check(
  manifest.dependencies?.["@bufbuild/protobuf"] === "^2.16.0",
  "package.json declares @bufbuild/protobuf for published declaration resolution",
);

const SCRATCH = mkdtempSync(join(tmpdir(), "published-tarball-"));

interface RunResult {
  exit: number;
  output: string;
}

async function run(
  args: string[],
  cwd: string,
  timeoutMs: number,
  extraEnv: Record<string, string> = {},
): Promise<RunResult> {
  const proc = Bun.spawn(args, {
    cwd,
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env, ...extraEnv },
  });
  const timeout = setTimeout(() => proc.kill(), timeoutMs);
  const [stdout, stderr, exit] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  clearTimeout(timeout);
  return { exit, output: `${stdout}\n${stderr}` };
}

function check(condition: boolean, message: string): void {
  if (condition) {
    console.log(`ok: ${message}`);
    return;
  }
  failed = true;
  console.error(`FAIL: ${message}`);
}

const consumer = (name: string): string => `import {
  BinaryReader,
  decodeProto,
  encodeProto,
  proto,
  type WasmWhatsAppClient,
  type WasmCallHandle,
  createCallControl,
  type DirectCallHostCallbacks,
  type WasmCallControl,
  UnpairedSurrogateError,
} from "${name}";
import { proto as protoSub } from "${name}/proto-types";
import { loadVoip, packetizeOpusForMlow } from "${name}/voip";
import { initVoipSync } from "${name}/voip/host";
import type { VoipRelayTransport } from "${name}";

export function voipContracts(transport: VoipRelayTransport, client: WasmWhatsAppClient) {
  const plugin = loadVoip(transport).voipBackend;
  const fromHost = initVoipSync(new Uint8Array(), transport).voipBackend;
  const packet: Uint8Array = packetizeOpusForMlow(new Uint8Array([0xbb, 3]));
  const accepted: Promise<WasmCallHandle> = client.acceptCallPcm("ringing", true);
  const dialed: Promise<WasmCallHandle> = client.dialCallPcm("peer@s.whatsapp.net", true);
  const pushed: boolean = client.callPushPcm16("ringing", new Int16Array(960));
  return { plugin, fromHost, packet, accepted, dialed, pushed };
}

export function directCallContracts(host: DirectCallHostCallbacks, transport: VoipRelayTransport): Promise<WasmCallControl> {
  return createCallControl(host, { onEvent() {} }, { voipBackend: loadVoip(transport).voipBackend });
}

// The run-observation contract, derived from the export itself: a
// declaration change that moves a field breaks this fixture at the access
// site instead of passing against a stale restatement.
export type RunCompletion = Awaited<
  ReturnType<WasmWhatsAppClient["waitForRunCompletion"]>
>;

export function describeCompletion(completion: RunCompletion): string {
  if (completion.reason !== "auto-reconnect-disabled") {
    // Branch-specific causes live only on the reconnect-disabled member.
    // @ts-expect-error - connectError is absent on every other branch
    void completion.connectError;
    return completion.reason;
  }
  return completion.connectError?.kind ?? "no-cause";
}

export function shutdownCompletion(): RunCompletion {
  return { reason: "shutdown-requested", generation: 0 };
}

export const stringEncodingError = new UnpairedSurrogateError(0, 0xd800);

export function roundtrip(): boolean {
  const bytes: Uint8Array = proto.Message.encode({
    conversation: "hello",
  }).finish();
  const decoded = proto.Message.decode(bytes);
  const viaSub = protoSub.Message.decode(bytes);
  void viaSub;
  const reader = new BinaryReader(bytes);
  void reader;
  const generic = encodeProto("Message", { conversation: "hi" });
  const back = decodeProto("Message", generic);
  void back;
  return decoded.conversation === "hello";
}
`;

const smoke = (name: string): string => `import * as root from "${name}";
import { proto } from "${name}/proto-types";
import { proto as rootProto } from "${name}";
import { loadVoip, packetizeOpusForMlow, MlowAudioDecoder } from "${name}/voip";

const assert = (cond, label) => {
  if (!cond) {
    console.error("smoke FAIL: " + label);
    process.exit(1);
  }
};
assert(typeof root.BinaryReader === "function", "root exports BinaryReader");
assert(rootProto === proto, "root and subpath share the proto namespace");
const bytes = proto.Message.encode({ conversation: "hello" }).finish();
assert(
  proto.Message.decode(bytes).conversation === "hello",
  "proto roundtrip",
);
assert(
  typeof root.encodeProto === "function" &&
    typeof root.decodeProto === "function",
  "generic codec exports",
);
const reader = new root.BinaryReader(bytes);
assert(reader.len === bytes.length, "BinaryReader over the wire bytes");
const voip = loadVoip({ connect: async () => { throw Error("offline"); } });
assert(typeof voip.voipBackend.sendFrame === "function", "voip plugin exports");
assert(packetizeOpusForMlow(new Uint8Array([0xbb, 3, 1, 2]))[0] === 0xdd, "voip codec");
const decoder = new MlowAudioDecoder();
assert(decoder.decode(new Uint8Array(), undefined).length === 960, "voip decoder");
decoder.free();
console.log("smoke: ok");
`;

const tsconfigs: Record<string, string> = {
  bundler: JSON.stringify({
    compilerOptions: {
      strict: true,
      skipLibCheck: false,
      noEmit: true,
      target: "ES2022",
      module: "ESNext",
      moduleResolution: "Bundler",
      lib: ["ES2022", "ES2024.String", "ESNext.TypedArrays", "ESNext.Disposable", "DOM"],
      types: ["node"],
    },
    include: ["consumer.ts"],
  }),
  nodenext: JSON.stringify({
    compilerOptions: {
      strict: true,
      skipLibCheck: false,
      noEmit: true,
      target: "ES2022",
      module: "NodeNext",
      moduleResolution: "NodeNext",
      types: ["node"],
    },
    include: ["consumer.ts"],
  }),
};

const typescriptRange = manifest.devDependencies?.["typescript"];
const typesNodeRange = manifest.devDependencies?.["@types/node"];
if (typescriptRange === undefined || typesNodeRange === undefined) {
  throw new Error(
    "package.json devDependencies must pin typescript and @types/node for the fixture tooling",
  );
}

try {
  let tarball: string;
  if (process.argv[2]) {
    tarball = resolve(process.argv[2]);
    if (!existsSync(tarball)) throw new Error(`tarball absent: ${tarball}`);
  } else {
    const pack = await run(["npm", "pack", "--pack-destination", SCRATCH], ROOT, 120_000);
    check(pack.exit === 0, `npm pack a tarball (${pack.output.trim().split("\n").at(-1) ?? ""})`);
    const tarballLine = pack.output.split("\n").map(line => line.trim()).find(line => line.endsWith(".tgz"));
    if (!tarballLine) throw new Error("npm pack printed no tarball name");
    tarball = join(SCRATCH, basename(tarballLine));
  }

  for (const mode of Object.keys(tsconfigs)) {
    const dir = join(SCRATCH, mode);
    const result = await (async () => {
      mkdirSync(dir, { recursive: true });
      writeFileSync(
        join(dir, "package.json"),
        JSON.stringify({ name: `fixture-${mode}`, version: "0.0.0", private: true, type: "module" }),
      );
      writeFileSync(join(dir, "consumer.ts"), consumer(manifest.name));
      writeFileSync(join(dir, "tsconfig.json"), tsconfigs[mode]!);

      const install = await run(
        [
          "npm",
          "install",
          "--no-audit",
          "--no-fund",
          tarball,
          `typescript@${typescriptRange}`,
          `@types/node@${typesNodeRange}`,
        ],
        dir,
        300_000,
      );
      if (install.exit !== 0) {
        return { ok: false as const, log: `npm install failed:\n${install.output}` };
      }
      const tsc = await run(
        [join(dir, "node_modules", ".bin", "tsc"), "-p", "tsconfig.json"],
        dir,
        300_000,
      );
      if (tsc.exit !== 0 || tsc.output.trim() !== "") {
        return { ok: false as const, log: `tsc (${mode}) failed:\n${tsc.output}` };
      }
      return { ok: true as const, log: "" };
    })();
    check(result.ok, `isolated ${mode} consumer typechecks with skipLibCheck:false`);
    if (!result.ok) console.error(result.log);
  }

  const smokeDir = join(SCRATCH, "bundler");
  writeFileSync(join(smokeDir, "smoke.mjs"), smoke(manifest.name));
  const smokeRun = await run(["node", "smoke.mjs"], smokeDir, 120_000);
  check(
    smokeRun.exit === 0 && smokeRun.output.includes("smoke: ok"),
    "runtime root/proto-types smoke over the isolated install",
  );
  if (!smokeRun.output.includes("smoke: ok")) console.error(smokeRun.output);

  writeFileSync(
    join(smokeDir, "client-lifecycle.test.mjs"),
    readFileSync(join(ROOT, "tests/fixtures/client-lifecycle.test.mjs"), "utf8"),
  );
  for (const runtime of ["node", process.execPath]) {
    for (const entry of ["root", "host"]) {
      const command = runtime === "node"
        ? [runtime, "--test", "client-lifecycle.test.mjs"]
        : [runtime, "test", "client-lifecycle.test.mjs"];
      const lifecycle = await run(command, smokeDir, 120_000, { BRIDGE_ENTRY: entry });
      check(lifecycle.exit === 0, `isolated production lifecycle: ${runtime} ${entry}`);
      if (lifecycle.exit !== 0) console.error(lifecycle.output);
    }
  }

  writeFileSync(join(smokeDir, "voip-lifecycle.mjs"), readFileSync(join(ROOT, "tests/fixtures/voip-lifecycle.mjs"), "utf8"));
  for (const runtime of ["node", process.execPath]) {
    for (const entry of ["root", "host"]) {
      const lifecycle = await run([runtime, "voip-lifecycle.mjs"], smokeDir, 30_000, { VOIP_ENTRY: entry });
      check(lifecycle.exit === 0 && lifecycle.output.includes("opened and closed"), `isolated VoIP codecs and lifecycle: ${runtime} ${entry}`);
      if (lifecycle.exit !== 0) console.error(lifecycle.output);
    }
  }

  writeFileSync(join(smokeDir, "voip-local-relay.mjs"), readFileSync(join(ROOT, "tests/fixtures/voip-local-relay.mjs"), "utf8"));
  writeFileSync(join(smokeDir, "voip-call-signaling.mjs"), readFileSync(join(ROOT, "tests/fixtures/voip-call-signaling.mjs"), "utf8"));
  writeFileSync(join(smokeDir, "voip-playout.mjs"), readFileSync(join(ROOT, "tests/fixtures/voip-playout.mjs"), "utf8"));
  for (const runtime of ["node", process.execPath]) {
    const playout = await run([runtime, "voip-playout.mjs"], smokeDir, 10_000);
    check(playout.exit === 0 && playout.output.includes("duplex PCM, timed playout and video delivered"), `isolated VoIP playout: ${runtime}`);
    if (playout.exit !== 0) console.error(playout.output);
  }

  writeFileSync(join(smokeDir, "voip-external-control.mjs"), readFileSync(join(ROOT, "tests/fixtures/voip-external-control.mjs"), "utf8"));
  for (const runtime of ["node", process.execPath]) {
    const direct = await run([runtime, "voip-external-control.mjs"], smokeDir, 15_000);
    check(direct.exit === 0 && direct.output.includes("standalone control delivered duplex PCM"), `isolated external call control: ${runtime}`);
    if (direct.exit !== 0) console.error(direct.output);
  }

  if (process.env.MOCK_SERVER_URL) {
    for (const file of ["voip-public-video.mjs", "voip-testsrc-idr.h264"]) {
      writeFileSync(join(smokeDir, file), readFileSync(join(ROOT, "tests/fixtures", file)));
    }
    for (const runtime of ["node", process.execPath]) {
      const video = await run([runtime, "voip-public-video.mjs"], smokeDir, 30_000, { NODE_TLS_REJECT_UNAUTHORIZED: "0" });
      check(video.exit === 0 && video.output.includes("public callPushVideo delivered matching H.264 through both WASMs"), `isolated public video: ${runtime}`);
      if (video.exit !== 0) console.error(video.output);
    }
  }

  const isolatedPackageJson = join(
    SCRATCH,
    "bundler",
    "node_modules",
    "@bufbuild",
    "protobuf",
    "package.json",
  );
  check(
    existsSync(isolatedPackageJson),
    "the isolated install carries @bufbuild/protobuf from the package dependency",
  );
} finally {
  if (!failed) rmSync(SCRATCH, { recursive: true, force: true });
  else console.error(`leaving scratch dir for inspection: ${SCRATCH}`);
}

if (failed) process.exit(1);
console.log("check:published-tarball: isolated install typechecks and runs in both modes");
