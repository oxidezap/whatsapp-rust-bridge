/**
 * The BoltFFI artifact must be as strongly typed as the wasm-bindgen one.
 *
 * Two checks, because they fail for different reasons: the declarations must
 * name real types (not `any`), and a program that misuses them must not
 * compile. The second is what catches a surface that silently widened.
 */
import { describe, expect, test } from "bun:test";
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BOLTFFI_DTS, boltffiAvailable } from "./boltffi-artifact";

const ROOT = join(import.meta.dir, "..");
const DTS = BOLTFFI_DTS;

const declarations = boltffiAvailable ? readFileSync(DTS, "utf8") : "";

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

describe.skipIf(!boltffiAvailable)("BoltFFI type safety", () => {
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

  // Each of these asserts the diagnostic, not just a non-zero exit. A renamed
  // declaration file, an unresolvable import or a rejected `tsc` flag also
  // exits non-zero, and would leave all three passing while proving nothing
  // about the type surface.
  test("passing a string where bytes are required fails to compile", () => {
    // If this ever compiles, the surface has widened to `any` somewhere.
    const result = typeCheck(`boltffi.md5("not bytes");`);
    expect(result.output).toContain("is not assignable to parameter of type 'Uint8Array");
    expect(result.ok).toBe(false);
  });

  test("treating a byte return as a string fails to compile", () => {
    const result = typeCheck(`const wrong: string = boltffi.md5(new Uint8Array([1]));`);
    expect(result.output).toContain("is not assignable to type 'string'");
    expect(result.ok).toBe(false);
  });

  test("a missing required argument fails to compile", () => {
    const result = typeCheck(`boltffi.calculateAgreement(new Uint8Array(33));`);
    expect(result.output).toContain("Expected 2 arguments, but got 1");
    expect(result.ok).toBe(false);
  });

  // The checks above import the declaration file by path, which is not how a
  // consumer reaches it. Resolving `@oxidezap/whatsapp-rust-bridge/boltffi`
  // goes through `exports`, so this is what fails if the generated `.d.ts` is
  // renamed or the subpath's `types` condition stops pointing at it — the
  // artifact would still be there and every other test would still pass.
  test("the published subpath resolves to the declarations", () => {
    const dir = mkdtempSync(join(tmpdir(), "boltffi-subpath-"));
    try {
      const scope = join(dir, "node_modules", "@oxidezap");
      mkdirSync(scope, { recursive: true });
      symlinkSync(ROOT, join(scope, "whatsapp-rust-bridge"), "dir");
      writeFileSync(join(dir, "package.json"), `{"type":"module"}`);
      writeFileSync(
        join(dir, "case.ts"),
        `import { md5 } from "@oxidezap/whatsapp-rust-bridge/boltffi";\n` +
          // Assigning to the wrong type: resolving to `any` would compile, and
          // compiling is the failure this is looking for.
          `const wrong: string = md5(new Uint8Array([1]));\nvoid wrong;\n`,
      );
      const result = Bun.spawnSync({
        cmd: [
          join(ROOT, "node_modules", ".bin", "tsc"),
          "--ignoreConfig",
          "--noEmit",
          "--strict",
          "--skipLibCheck",
          "--module", "nodenext",
          "--moduleResolution", "nodenext",
          "--target", "es2022",
          "--types", "node",
          join(dir, "case.ts"),
        ],
        stdout: "pipe",
        stderr: "pipe",
      });
      const output = `${result.stdout.toString()}${result.stderr.toString()}`;
      expect(output).toContain("is not assignable to type 'string'");
      expect(result.exitCode).not.toBe(0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
