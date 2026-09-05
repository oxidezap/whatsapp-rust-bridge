/**
 * Intentional core update: move the `whatsapp-rust` pin to the latest
 * `main` commit, or to an explicit full SHA for reproducibility.
 *
 * A `rev` pin never moves under `cargo update`, so the old
 * `cargo update -p whatsapp-rust` silently rebuilt the same commit. This
 * rewrites only the pin, refreshes the lockfile, and fails visibly when
 * resolution or the update errors instead of keeping a stale SHA.
 *
 * Run: bun run scripts/bump-wacore.ts [full-commit-sha]
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

/**
 * Rewrite only the `rev` value on the core dependency line. Anything else
 * (missing line, missing `rev`, more than one core line) fails instead of
 * guessing, so unrelated manifest content is never touched.
 */
export function rewritePin(manifest: string, sha: string): string {
  const lines = manifest.split("\n");
  const hits = lines.filter((line) => /^\s*whatsapp-rust\s*=/.test(line));
  if (hits.length !== 1) {
    throw new Error(
      `bump:wacore expected exactly one ${CORE_PACKAGE} dependency line, found ${hits.length}`
    );
  }
  const index = lines.indexOf(hits[0]);
  const rewritten = hits[0].replace(/rev\s*=\s*"[^"]*"/, `rev = "${sha}"`);
  if (rewritten === hits[0]) {
    throw new Error("bump:wacore found no rev pin to rewrite on the core line");
  }
  const next = [...lines];
  next[index] = rewritten;
  return next.join("\n");
}

async function main() {
  const target = parseBumpArgs(Bun.argv.slice(2));
  const root = join(import.meta.dir, "..");
  const manifestPath = join(root, "Cargo.toml");

  const status = Bun.spawnSync({ cmd: ["git", "diff", "--quiet", "Cargo.toml"], cwd: root });
  if (status.exitCode !== 0) {
    throw new Error("bump:wacore refuses to run with uncommitted Cargo.toml changes");
  }

  const sha = target.kind === "sha" ? target.sha : await resolveLatestMain();
  const before = readFileSync(manifestPath, "utf8");
  writeFileSync(manifestPath, rewritePin(before, sha));
  try {
    const update = Bun.spawnSync({
      cmd: ["cargo", "update", "-p", CORE_PACKAGE],
      cwd: root,
      stdout: "inherit",
      stderr: "inherit",
    });
    if (update.exitCode !== 0) {
      throw new Error("bump:wacore: cargo update -p whatsapp-rust failed");
    }
  } catch (error) {
    writeFileSync(manifestPath, before);
    throw error;
  }
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
