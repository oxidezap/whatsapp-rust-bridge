/**
 * The BoltFFI artifact must be as strongly typed as the wasm-bindgen one.
 *
 * Two checks, because they fail for different reasons: the declarations must
 * name real types (not `any`), and a program that misuses them must not
 * compile. The second is what catches a surface that silently widened.
 */
import { describe, expect, test } from "bun:test";
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const DTS = join(ROOT, "dist", "boltffi", "pkg", "whatsapp_rust_bridge_boltffi_node.d.ts");

const declarations = readFileSync(DTS, "utf8");

/** Type-check a snippet against the emitted declarations. */
function typeCheck(body: string): { ok: boolean; output: string } {
  const dir = mkdtempSync(join(tmpdir(), "boltffi-types-"));
  try {
    return runTsc(dir, body);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function runTsc(dir: string, body: string): { ok: boolean; output: string } {
  const file = join(dir, "case.ts");
  writeFileSync(file, `import * as boltffi from ${JSON.stringify(DTS.replace(/\.d\.ts$/, ""))};\n${body}\n`);
  const result = Bun.spawnSync({
    cmd: [
      join(ROOT, "node_modules", ".bin", "tsc"),
      "--ignoreConfig",
      "--noEmit",
      "--strict",
      "--skipLibCheck",
      "--module", "esnext",
      "--target", "es2022",
      "--moduleResolution", "bundler",
      "--types", "node",
      file,
    ],
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    ok: result.exitCode === 0,
    output: `${result.stdout.toString()}${result.stderr.toString()}`,
  };
}

describe("BoltFFI type safety", () => {
  test("the public surface declares no `any`", () => {
    // Comments are stripped first: a doc line mentioning the word "any" is not
    // a widened type, and letting prose trip the gate makes it flaky.
    const offenders = declarations
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .split("\n")
      .map((line) => line.replace(/\/\/.*$/, ""))
      .filter((line) => /\bany\b/.test(line));
    expect(offenders).toEqual([]);
  });

  test("byte-carrying operations are declared as Uint8Array, not a loose type", () => {
    expect(declarations).toContain("export declare function md5(input: Uint8Array): Uint8Array;");
    expect(declarations).toContain(
      "export declare function calculateAgreement(publicKey: Uint8Array, privateKey: Uint8Array): Uint8Array;",
    );
  });

  test("a fallible operation declares a typed error class", () => {
    expect(declarations).toContain("class BridgeUtilErrorException extends Error");
  });

  test("correct usage compiles", () => {
    const result = typeCheck(`
      const digest: Uint8Array = boltffi.md5(new Uint8Array([1]));
      const pub: Uint8Array = boltffi.getPublicFromPrivateKey(new Uint8Array(32));
      const ok: boolean = boltffi.verifySignature(pub, digest, new Uint8Array(64));
      void ok;
    `);
    expect(result.output).toBe("");
    expect(result.ok).toBe(true);
  });

  test("passing a string where bytes are required fails to compile", () => {
    // If this ever compiles, the surface has widened to `any` somewhere.
    expect(typeCheck(`boltffi.md5("not bytes");`).ok).toBe(false);
  });

  test("treating a byte return as a string fails to compile", () => {
    expect(typeCheck(`const wrong: string = boltffi.md5(new Uint8Array([1]));`).ok).toBe(false);
  });

  test("a missing required argument fails to compile", () => {
    expect(typeCheck(`boltffi.calculateAgreement(new Uint8Array(33));`).ok).toBe(false);
  });
});
