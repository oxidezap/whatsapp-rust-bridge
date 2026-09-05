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
import {
  parseBumpArgs,
  resolveLatestMain,
  rewritePin,
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

describe("bump:wacore latest-main resolution", () => {
  test("the HEAD symref line does not win over refs/heads/main", async () => {
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
