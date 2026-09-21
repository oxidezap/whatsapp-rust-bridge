/**
 * Generic synchronous crypto exports: known-answer behavior, provider parity,
 * structured validation, and the host-loaded entrypoint smoke.
 */

import { describe, expect, test, beforeAll } from "bun:test";
import {
  aesGcm256Decrypt,
  aesGcm256Encrypt,
  initWasmEngine,
  sha256,
} from "../dist/index.js";

const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex");
const bytes = (value: string) => Uint8Array.from(Buffer.from(value, "hex"));

beforeAll(() => initWasmEngine());

describe("synchronous crypto exports", () => {
  test("matches the AES-256-GCM known-answer vector", () => {
    const ciphertext = aesGcm256Encrypt(
      new Uint8Array(32),
      new Uint8Array(12),
      new Uint8Array(),
      new Uint8Array(16)
    );
    expect(hex(ciphertext)).toBe(
      "cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919"
    );
    expect(
      hex(aesGcm256Decrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), ciphertext))
    ).toBe("00000000000000000000000000000000");
  });

  test("matches SHA-256 known answers", () => {
    expect(hex(sha256(new TextEncoder().encode("abc")))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
  });

  test("rejects malformed inputs with structured errors", () => {
    const cases: Array<[string, () => unknown]> = [
      ["key", () => aesGcm256Encrypt(new Uint8Array(31), new Uint8Array(12), new Uint8Array(), new Uint8Array())],
      ["nonce", () => aesGcm256Encrypt(new Uint8Array(32), new Uint8Array(11), new Uint8Array(), new Uint8Array())],
      ["ciphertextWithTag", () => aesGcm256Decrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), new Uint8Array(15))],
    ];
    for (const [field, call] of cases) {
      expect(call).toThrow();
      try {
        call();
      } catch (error) {
        expect(error).toMatchObject({ name: "WhatsAppError", kind: "invalid-argument", field });
      }
    }

    const forged = bytes(
      "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bdde"
    );
    try {
      aesGcm256Decrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), forged);
      throw new Error("expected authentication failure");
    } catch (error) {
      expect(error).toMatchObject({ name: "WhatsAppError", kind: "crypto" });
    }
  });

  test("host AES provider agrees with the Rust provider", async () => {
    const script = `
      import { initWasmEngine, aesGcm256Encrypt, aesGcm256Decrypt } from "./dist/index.js";
      import { createCipheriv } from "node:crypto";
      const callbacks = {
        aesCbc256Encrypt() { return new Uint8Array(); },
        aesCbc256Decrypt() { return new Uint8Array(); },
        aesGcm256Encrypt(key, nonce, aad, plaintext) {
          const cipher = createCipheriv("aes-256-gcm", key, nonce);
          cipher.setAAD(aad);
          return new Uint8Array(Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]));
        },
        aesGcm256Decrypt(key, nonce, aad, ciphertext) {
          const decipher = createDecipheriv("aes-256-gcm", key, nonce);
          decipher.setAAD(aad);
          decipher.setAuthTag(ciphertext.slice(-16));
          return new Uint8Array(Buffer.concat([decipher.update(ciphertext.slice(0, -16)), decipher.final()]));
        },
        hmacSha256() { return new Uint8Array(32); },
      };
      if (process.env.CRYPTO_PROVIDER === "host") initWasmEngine(undefined, callbacks);
      else initWasmEngine();
      const key = new Uint8Array(32).fill(7);
      const nonce = new Uint8Array(12).fill(8);
      const aad = new Uint8Array([1, 2, 3]);
      const plaintext = new Uint8Array(4096).fill(9);
      const sealed = aesGcm256Encrypt(key, nonce, aad, plaintext);
      console.log(Buffer.from(sealed).toString("hex"), sealed.length);
    `;
    const run = (mode: string) =>
      Bun.spawn([process.execPath, "-e", script], {
        cwd: process.cwd(),
        env: { ...process.env, CRYPTO_PROVIDER: mode },
        stdout: "pipe",
        stderr: "pipe",
      });
    const rust = run("rust");
    const host = run("host");
    const [rustExit, hostExit, rustOutput, hostOutput, hostError] = await Promise.all([
      rust.exited,
      host.exited,
      new Response(rust.stdout).text(),
      new Response(host.stdout).text(),
      new Response(host.stderr).text(),
    ]);
    expect(rustExit).toBe(0);
    expect(hostExit, hostError).toBe(0);
    expect(hostOutput.trim()).toBe(rustOutput.trim());
    expect(hostOutput.trim().split(" ")[1]).toBe("4112");
  });

  test("host-loaded entrypoint can execute the crypto surface", () => {
    const script = `
      import { readFileSync } from "node:fs";
      import { initSync, initWasmEngine, sha256 } from "./dist/host.js";
      initSync({ module: readFileSync("./dist/whatsapp_rust_bridge_bg.wasm") });
      initWasmEngine();
      console.log(Buffer.from(sha256(new TextEncoder().encode("abc"))).toString("hex"));
    `;
    const result = Bun.spawnSync([process.execPath, "-e", script], {
      cwd: process.cwd(),
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(result.exitCode).toBe(0);
    expect(result.stdout.toString().trim()).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
  });
});
