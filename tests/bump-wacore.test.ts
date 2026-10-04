/**
 * The core updater resolves before it rewrites, and rewrites only the pin.
 *
 * Default resolution shells nothing (argv arrays), explicit SHAs are
 * validated before use, and a manifest the updater does not recognize
 * fails instead of drifting. None of these touch the real manifest,
 * lockfile, or network: resolution runs through an injected seam and
 * rewriting through string fixtures.
 *
 * Run: bun test tests/bump-wacore.test.ts
 */

import { describe, test, expect } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  parseBumpArgs,
  resolveLatestMain,
  rewritePin,
  runBump,
  isManifestDirty,
  type BumpDeps,
  type CommandRunner,
} from "../scripts/bump-wacore";

const MANIFEST = [
  `[dependencies]`,
  `serde = "1"`,
  `whatsapp-rust = { git = "https://github.com/oxidezap/whatsapp-rust", rev = "0ca19b249daff66d196fea988bd5643951f47ffc", default-features = false }`,
  ``,
  `[features]`,
  `legacy-session = ["whatsapp-rust/legacy-session-interop"]`,
  ``,
].join("\n");

const NEXT = "f".repeat(40);

describe("bump:wacore argument parsing", () => {
  test("no arguments resolve latest main", () => {
    expect(parseBumpArgs([])).toEqual({ kind: "latest" });
  });

  test("one full SHA is accepted and normalized", () => {
    expect(parseBumpArgs([NEXT.toUpperCase()])).toEqual({ kind: "sha", sha: NEXT });
  });

  test.each([
    ["short SHA", ["abc123"]],
    ["non-hex", ["z".repeat(40)]],
    ["empty string", [""]],
    ["two arguments", [NEXT, NEXT]],
    ["flag", ["--latest"]],
  ])("invalid input fails: %s", (_name, argv) => {
    expect(() => parseBumpArgs(argv)).toThrow(/bump:wacore/);
  });
});

describe("bump:wacore pin rewriting", () => {
  test("only the rev value changes", () => {
    const rewritten = rewritePin(MANIFEST, NEXT);
    expect(rewritten).toContain(`rev = "${NEXT}"`);
    expect(rewritten).not.toContain("0ca19b2");
    // Feature lines naming the core package are untouched.
    expect(rewritten).toContain(`legacy-session = ["whatsapp-rust/legacy-session-interop"]`);
    expect(rewritten.split("\n").length).toBe(MANIFEST.split("\n").length);
  });

  test.each([
    ["no core line", `[dependencies]\nserde = "1"\n`],
    ["two core lines", `whatsapp-rust = "1"\nwhatsapp-rust = "2"\n`],
    ["branch pin without rev", `whatsapp-rust = { git = "https://x", branch = "main" }\n`],
  ])("unrecognized manifests fail: %s", (_name, manifest) => {
    expect(() => rewritePin(manifest, NEXT)).toThrow(/bump:wacore/);
  });
});

describe("bump:wacore latest-main resolution", () => {  test("the HEAD symref line does not win over refs/heads/main", async () => {
    const seen: Array<{ command: string; args: string[] }> = [];
    const run: CommandRunner = async (command, args) => {
      seen.push({ command, args });
      return { stdout: `${"a".repeat(40)}\tHEAD\n${NEXT}\trefs/heads/main\n` };
    };
    await expect(resolveLatestMain(run)).resolves.toBe(NEXT);
    expect(seen).toEqual([
      {
        command: "git",
        args: ["ls-remote", "https://github.com/oxidezap/whatsapp-rust", "refs/heads/main"],
      },
    ]);
  });

  test("unparseable output fails instead of keeping a stale SHA", async () => {
    const run: CommandRunner = async () => ({ stdout: "nothing useful\n" });
    await expect(resolveLatestMain(run)).rejects.toThrow(/could not resolve/);
  });

  test("runner failure propagates", async () => {
    const run: CommandRunner = async () => {
      throw new Error("network down");
    };
    await expect(resolveLatestMain(run)).rejects.toThrow("network down");
  });
});

describe("bump:wacore orchestration", () => {
  function fixtureDeps(overrides: Partial<BumpDeps> = {}): BumpDeps & {
    files: { manifest: string };
    calls: { cargoUpdate: number; runBuild: number; resolved: string[] };
  } {
    const files = { manifest: MANIFEST };
    const calls = { cargoUpdate: 0, runBuild: 0, resolved: [] as string[] };
    return {
      files,
      calls,
      readManifest: () => files.manifest,
      writeManifest: (content) => {
        files.manifest = content;
      },
      manifestDirty: () => false,
      resolveSha: async (target) => {
        if (target.kind === "sha") return target.sha;
        calls.resolved.push("latest");
        return NEXT;
      },
      cargoUpdate: () => {
        calls.cargoUpdate += 1;
      },
      runBuild: () => {
        calls.runBuild += 1;
      },
      ...overrides,
    };
  }

  test("an explicit SHA reaches the rewrite, never the build", async () => {
    const deps = fixtureDeps();
    const sha = await runBump([NEXT], deps);
    expect(sha).toBe(NEXT);
    expect(deps.files.manifest).toContain(`rev = "${NEXT}"`);
    expect(deps.calls.cargoUpdate).toBe(1);
    // The build runs with no arguments by construction: an explicit SHA
    // cannot leak into it the way `cmd arg && build` chains leak argv.
    expect(deps.calls.runBuild).toBe(1);
    expect(deps.calls.resolved).toEqual([]);
  });

  test("default resolves latest main before rewriting", async () => {
    const deps = fixtureDeps();
    await runBump([], deps);
    expect(deps.calls.resolved).toEqual(["latest"]);
    expect(deps.files.manifest).toContain(`rev = "${NEXT}"`);
  });

  test("a dirty manifest refuses before any write or resolution", async () => {
    const deps = fixtureDeps({ manifestDirty: () => true });
    await expect(runBump([NEXT], deps)).rejects.toThrow(/uncommitted/);
    expect(deps.files.manifest).toBe(MANIFEST);
    expect(deps.calls.cargoUpdate).toBe(0);
    expect(deps.calls.runBuild).toBe(0);
  });

  test("failed update restores an untouched manifest", async () => {
    const deps = fixtureDeps({
      cargoUpdate: () => {
        deps.calls.cargoUpdate += 1;
        throw new Error("cargo update failed");
      },
    });
    await expect(runBump([NEXT], deps)).rejects.toThrow(/restored/);
    expect(deps.files.manifest).toBe(MANIFEST);
    expect(deps.calls.runBuild).toBe(0);
  });

  test("failed update leaves concurrent edits in place and says so", async () => {
    const deps = fixtureDeps();
    const concurrent = MANIFEST + "# concurrent edit\n";
    deps.cargoUpdate = () => {
      deps.files.manifest = concurrent;
      throw new Error("cargo update failed");
    };
    await expect(runBump([NEXT], deps)).rejects.toThrow(/concurrent edits/);
    expect(deps.files.manifest).toBe(concurrent);
  });

  test("build failure keeps the coherent pin and lock", async () => {
    const deps = fixtureDeps({
      runBuild: () => {
        throw new Error("build broke");
      },
    });
    await expect(runBump([NEXT], deps)).rejects.toThrow("build broke");
    expect(deps.files.manifest).toContain(`rev = "${NEXT}"`);
  });

  test("rewriting the already-pinned SHA is idempotent", () => {
    const pinned = rewritePin(MANIFEST, NEXT);
    expect(rewritePin(pinned, NEXT)).toBe(pinned);
  });

  test("the package entrypoint runs only the script, never a chained build", async () => {
    // The previous command (`script args && build`) forwarded an explicit
    // SHA to the build instead of the parser. The command must stay a
    // single entrypoint; behavior is covered by the orchestration tests.
    const pkg = await Bun.file(
      join(import.meta.dir, "..", "package.json")
    ).json();
    expect(pkg.scripts["bump:wacore"]).toBe("bun run scripts/bump-wacore.ts");
  });
});

describe("bump:wacore dirty detection", () => {
  function fixtureRepo(): string {
    const dir = mkdtempSync(join(tmpdir(), "bump-wacore-dirty-"));
    const git = (args: string[]) => {
      const proc = Bun.spawnSync({ cmd: ["git", ...args], cwd: dir });
      if ((proc.exitCode ?? 1) !== 0) {
        throw new Error(`fixture git ${args.join(" ")} failed`);
      }
    };
    git(["init", "-q"]);
    git(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
    writeFileSync(join(dir, "Cargo.toml"), MANIFEST);
    git(["add", "Cargo.toml"]);
    git(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "manifest"]);
    return dir;
  }

  test("clean tree is not dirty", () => {
    expect(isManifestDirty(fixtureRepo())).toBe(false);
  });

  test("unstaged edits are dirty", () => {
    const dir = fixtureRepo();
    writeFileSync(join(dir, "Cargo.toml"), MANIFEST + "# edit\n");
    expect(isManifestDirty(dir)).toBe(true);
  });

  test("staged edits are dirty", () => {
    const dir = fixtureRepo();
    writeFileSync(join(dir, "Cargo.toml"), MANIFEST + "# edit\n");
    const proc = Bun.spawnSync({ cmd: ["git", "add", "Cargo.toml"], cwd: dir });
    expect(proc.exitCode).toBe(0);
    expect(isManifestDirty(dir)).toBe(true);
  });
});
