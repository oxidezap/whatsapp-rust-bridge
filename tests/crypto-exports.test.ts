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
  test("requires engine initialization before AES-GCM", () => {
    const result = Bun.spawnSync(
      [
        process.execPath,
        "-e",
        `import { aesGcm256Encrypt } from "./dist/index.js";
         try { aesGcm256Encrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), new Uint8Array()); }
         catch (error) { console.log(error.name, error.kind, error.field); }`,
      ],
      { cwd: process.cwd(), stdout: "pipe", stderr: "pipe" }
    );
    expect(result.exitCode).toBe(0);
    expect(result.stdout.toString().trim()).toBe(
      "WhatsAppError invalid-argument initWasmEngine"
    );
  });

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

  test("matches AES-GCM empty and AAD-bearing vectors", () => {
    expect(
      hex(aesGcm256Encrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), new Uint8Array()))
    ).toBe("530f8afbc74536b9a963b4f1c4cb738b");
    expect(
      hex(
        aesGcm256Encrypt(
          new Uint8Array(32),
          new Uint8Array(12),
          Uint8Array.from([1, 2, 3, 4, 5]),
          new Uint8Array(32)
        )
      )
    ).toBe(
      "cea7403d4d606b6e074ec5d3baf39d18726003ca37a62a74d1a2f58e7506358e524dae71bc2d7800d71655079fa5f137"
    );
  });

  test("matches SHA-256 known answers", () => {
    expect(hex(sha256(new Uint8Array()))).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    expect(hex(sha256(Uint8Array.from([0, 255, 16, 1])))).toBe(
      "250b952c460c97873c976711b4aa5cbb45f923ca81b762e2911bc6c1c07cd943"
    );
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

    const key = new Uint8Array(32);
    const nonce = new Uint8Array(12);
    const aad = Uint8Array.from([1, 2, 3]);
    const sealed = aesGcm256Encrypt(key, nonce, aad, new Uint8Array(32));
    for (const [label, mutate] of [
      ["ciphertext", (value: Uint8Array) => (value[0] ^= 1)],
      ["tag", (value: Uint8Array) => (value[value.length - 1] ^= 1)],
    ] as const) {
      const altered = sealed.slice();
      mutate(altered);
      expect(() => aesGcm256Decrypt(key, nonce, aad, altered), label).toThrow();
    }
    expect(() => aesGcm256Decrypt(key, nonce, Uint8Array.from([1, 2, 4]), sealed)).toThrow();
    expect(() => aesGcm256Decrypt(key, Uint8Array.from([1, ...nonce.slice(1)]), aad, sealed)).toThrow();
  });

  test("rejects malformed host provider output", () => {
    const script = `
      import { initWasmEngine, aesGcm256Encrypt, aesGcm256Decrypt } from "./dist/index.js";
      const bad = new Uint8Array(1);
      initWasmEngine(undefined, {
        aesCbc256Encrypt() { return bad; },
        aesCbc256Decrypt() { return bad; },
        aesGcm256Encrypt() { return bad; },
        aesGcm256Decrypt() { return bad; },
        hmacSha256() { return new Uint8Array(32); },
      });
      try {
        if (process.env.BAD_MODE === "encrypt")
          aesGcm256Encrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), new Uint8Array(2048));
        else
          aesGcm256Decrypt(new Uint8Array(32), new Uint8Array(12), new Uint8Array(), new Uint8Array(2064));
      } catch (error) {
        console.log(error.name, error.kind);
      }
    `;
    for (const mode of ["encrypt", "decrypt"]) {
      const result = Bun.spawnSync([process.execPath, "-e", script], {
        cwd: process.cwd(),
        env: { ...process.env, BAD_MODE: mode },
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(result.exitCode, result.stderr.toString()).toBe(0);
      expect(result.stdout.toString().trim()).toBe("WhatsAppError crypto");
    }
  });

  test("host AES provider agrees with the Rust provider", async () => {
    const script = `
      import { initWasmEngine, aesGcm256Encrypt, aesGcm256Decrypt } from "./dist/index.js";
      import { createCipheriv, createDecipheriv } from "node:crypto";
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
      const opened = aesGcm256Decrypt(key, nonce, aad, sealed);
      console.log(Buffer.from(sealed).toString("hex"), Buffer.from(opened).toString("hex"));
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
    const [hostSealed, hostOpened] = hostOutput.trim().split(" ");
    const [rustSealed, rustOpened] = rustOutput.trim().split(" ");
    expect(hostSealed).toBe(rustSealed);
    expect(hostOpened).toBe(rustOpened);
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
