/**
 * Intentional core update: move the `whatsapp-rust` pin to the latest
 * `main` commit, or to an explicit full SHA for reproducibility.
 *
 * A `rev` pin never moves under `cargo update`, so plain
 * `cargo update -p whatsapp-rust` silently rebuilds the same commit. This
 * is the single entrypoint: it resolves the SHA, rewrites both sets of pins,
 * refreshes both lockfiles, then runs the build itself, so an explicit SHA
 * reaches the parser and is never forwarded to the build.
 *
 * Run: bun run scripts/bump-wacore.ts [full-commit-sha]
 * (package.json `bump:wacore` runs exactly this and nothing after it.)
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export const CORE_REPO = "https://github.com/oxidezap/whatsapp-rust";
export const CORE_PACKAGE = "whatsapp-rust";

export type BumpTarget =
  | { kind: "latest" }
  | { kind: "sha"; sha: string };

/** Parse CLI arguments; anything but zero args or one full SHA fails. */
export function parseBumpArgs(argv: string[]): BumpTarget {
  if (argv.length === 0) return { kind: "latest" };
  if (argv.length > 1) {
    throw new Error("bump:wacore takes at most one argument: a full commit SHA");
  }
  const [sha] = argv;
  if (!/^[0-9a-f]{40}$/i.test(sha)) {
    throw new Error(
      `bump:wacore expects a full 40-hex commit SHA, got ${JSON.stringify(sha)}`
    );
  }
  return { kind: "sha", sha: sha.toLowerCase() };
}

export type CommandRunner = (
  command: string,
  args: string[]
) => Promise<{ stdout: string }>;

/** Resolve the latest `main` commit without a shell: argv array only. */
export async function resolveLatestMain(
  run: CommandRunner = defaultRunner
): Promise<string> {
  const { stdout } = await run("git", ["ls-remote", CORE_REPO, "refs/heads/main"]);
  for (const line of stdout.split("\n")) {
    const match = /^([0-9a-f]{40})\s+refs\/heads\/main\s*$/.exec(line.trim());
    if (match) return match[1].toLowerCase();
  }
  throw new Error("bump:wacore could not resolve the latest main commit");
}

function defaultRunner(command: string, args: string[]): Promise<{ stdout: string }> {
  const proc = Bun.spawnSync({ cmd: [command, ...args], stderr: "ignore" });
  if (proc.exitCode !== 0) {
    throw new Error(`bump:wacore: ${command} ${args.join(" ")} failed`);
  }
  return Promise.resolve({ stdout: proc.stdout.toString() });
}

const REV_RE = /rev\s*=\s*"([^"]*)"/;

/**
 * Rewrite only the `rev` value on the core dependency line. A missing line,
 * a second core line, or a line with no `rev` fails instead of guessing,
 * so unrelated manifest content is never touched. Rewriting the SHA that
 * is already pinned is a valid idempotent no-op.
 */
export function rewritePin(manifest: string, sha: string, dependency = CORE_PACKAGE): string {
  const lines = manifest.split("\n");
  const hits = lines.filter((line) => line.split("=", 1)[0].trim() === dependency);
  if (hits.length !== 1) {
    throw new Error(
      `bump:wacore expected exactly one ${dependency} dependency line, found ${hits.length}`
    );
  }
  if (!REV_RE.test(hits[0])) {
    throw new Error("bump:wacore found no rev pin to rewrite on the core line");
  }
  const next = [...lines];
  next[lines.indexOf(hits[0])] = hits[0].replace(REV_RE, `rev = "${sha}"`);
  return next.join("\n");
}

const CARGO_PROJECTS = [
  { manifest: "Cargo.toml", lock: "Cargo.lock", packages: [CORE_PACKAGE] },
  {
    manifest: "crates/voip-wasm/Cargo.toml",
    lock: "crates/voip-wasm/Cargo.lock",
    packages: ["wacore", "wacore-binary"],
  },
] as const;

const CARGO_FILES = CARGO_PROJECTS.flatMap(({ manifest, lock }) => [manifest, lock]);
type CargoFile = (typeof CARGO_FILES)[number];
type CargoManifest = (typeof CARGO_PROJECTS)[number]["manifest"];

export interface BumpDeps {
  readFile(path: CargoFile): string;
  writeFile(path: CargoFile, content: string): void;
  manifestDirty(): boolean;
  resolveSha(target: BumpTarget): Promise<string>;
  cargoUpdate(manifest: CargoManifest, packages: readonly string[]): void;
  runBuild(): void;
}

export async function runBump(argv: string[], deps: BumpDeps): Promise<string> {
  const target = parseBumpArgs(argv);
  const assertClean = () => {
    if (deps.manifestDirty()) {
      throw new Error("bump:wacore refuses to run with uncommitted Cargo manifest or lockfile changes");
    }
  };
  assertClean();
  const sha = await deps.resolveSha(target);
  assertClean();

  const before = new Map(CARGO_FILES.map((path) => [path, deps.readFile(path)] as const));
  const manifests = CARGO_PROJECTS.map(({ manifest, packages }) =>
    packages.reduce((content, name) => rewritePin(content, sha, name), before.get(manifest)!),
  );
  const written = new Map<CargoFile, string>();
  try {
    for (const [index, { manifest }] of CARGO_PROJECTS.entries()) {
      deps.writeFile(manifest, manifests[index]);
      written.set(manifest, manifests[index]);
    }
    for (const { manifest, lock, packages } of CARGO_PROJECTS) {
      try {
        deps.cargoUpdate(manifest, packages);
      } finally {
        written.set(lock, deps.readFile(lock));
      }
    }
  } catch (error) {
    const concurrent: CargoFile[] = [];
    for (const [path, content] of written) {
      if (deps.readFile(path) === content) {
        deps.writeFile(path, before.get(path)!);
      } else {
        concurrent.push(path);
      }
    }
    const outcome = concurrent.length ? `concurrent edits left in ${concurrent.join(", ")}` : "files restored";
    throw new Error(`bump:wacore: update failed (${outcome}): ${error instanceof Error ? error.message : error}`);
  }
  deps.runBuild();
  return sha;
}

function realDeps(root: string): BumpDeps {
  return {
    readFile: (path) => readFileSync(join(root, path), "utf8"),
    writeFile: (path, content) => writeFileSync(join(root, path), content),
    manifestDirty: () => isManifestDirty(root),
    resolveSha: async (target) =>
      target.kind === "sha" ? target.sha : await resolveLatestMain(),
    cargoUpdate: (manifest, packages) => {
      const args = ["update", "--manifest-path", manifest, ...packages.flatMap((name) => ["-p", name])];
      const proc = Bun.spawnSync({ cmd: ["cargo", ...args], cwd: root, stderr: "inherit" });
      if ((proc.exitCode ?? 1) !== 0) {
        throw new Error(`cargo ${args.join(" ")} failed`);
      }
    },
    runBuild: () => {
      const proc = Bun.spawnSync({
        cmd: ["bun", "run", "build"],
        cwd: root,
        stdout: "inherit",
        stderr: "inherit",
      });
      if ((proc.exitCode ?? 1) !== 0) {
        throw new Error("bump:wacore: bun run build failed");
      }
    },
  };
}

export function isManifestDirty(root: string): boolean {
  const proc = Bun.spawnSync({
    cmd: ["git", "status", "--porcelain", "--", ...CARGO_FILES],
    cwd: root,
  });
  if (proc.exitCode !== 0) throw new Error("bump:wacore could not check Git status");
  return proc.stdout.toString().trim().length > 0;
}

async function main() {
  const root = join(import.meta.dir, "..");
  const sha = await runBump(Bun.argv.slice(2), realDeps(root));
  console.log(`bump:wacore: pinned ${CORE_PACKAGE} at ${sha}`);
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
