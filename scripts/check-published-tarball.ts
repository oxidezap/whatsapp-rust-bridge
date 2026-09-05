/**
 * Isolated-tarball contract check: pack the built package, install it in a
 * fresh directory outside the repository — so no parent `node_modules` can
 * leak in — and prove a consumer with `skipLibCheck: false` typechecks in
 * both Bundler and NodeNext modes and runs the real exports, using only the
 * package's declared dependencies plus explicit TypeScript/Node tooling.
 *
 * This is the check the in-tree `tests/published-dts.test.ts` cannot be: the
 * checkout's own `devDependencies` resolve `@bufbuild/protobuf` from the
 * repo's `node_modules`, masking the hole an isolated consumer falls into.
 * A name-only entry in `package.json` does not pass here either — the fixture
 * install would fail to provide the module, and `tsc` plus the `npm ls`
 * chain assertion below would fail with it.
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
import { basename, join } from "node:path";

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

const SCRATCH = mkdtempSync(join(tmpdir(), "published-tarball-"));
let failed = false;

interface RunResult {
  exit: number;
  output: string;
}

async function run(
  args: string[],
  cwd: string,
  timeoutMs: number,
): Promise<RunResult> {
  const proc = Bun.spawn(args, {
    cwd,
    stdout: "pipe",
    stderr: "pipe",
    env: process.env,
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
} from "${name}";
import { proto as protoSub } from "${name}/proto-types";

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
  const pack = await run(
    ["npm", "pack", "--pack-destination", SCRATCH],
    ROOT,
    120_000,
  );
  check(pack.exit === 0, `npm pack a tarball (${pack.output.trim().split("\n").at(-1) ?? ""})`);
  const tarballLine = pack.output
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.endsWith(".tgz"));
  if (tarballLine === undefined) throw new Error("npm pack printed no tarball name");
  const tarball = join(SCRATCH, basename(tarballLine));

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

  const ls = await run(
    ["npm", "ls", "@bufbuild/protobuf"],
    join(SCRATCH, "bundler"),
    120_000,
  );
  check(
    ls.exit === 0 && ls.output.includes(manifest.name),
    "@bufbuild/protobuf resolves through the package's declared dependencies",
  );
  if (ls.exit !== 0) console.error(ls.output);
} finally {
  if (!failed) rmSync(SCRATCH, { recursive: true, force: true });
  else console.error(`leaving scratch dir for inspection: ${SCRATCH}`);
}

if (failed) process.exit(1);
console.log("check:published-tarball: isolated install typechecks and runs in both modes");
