/**
 * One-shot crypto boundary benchmark. Run after `bun run build`:
 *
 *   bun run benches/crypto.ts
 *   CRYPTO_PROVIDER=host bun run benches/crypto.ts
 *
 * The host mode exercises the same callback provider used by applications;
 * the default mode measures the Rust provider.
 */

import { createCipheriv, createDecipheriv } from "node:crypto";
import { aesGcm256Decrypt, aesGcm256Encrypt, initWasmEngine, sha256 } from "../dist/index.js";

const host = process.env.CRYPTO_PROVIDER === "host";
if (host) {
  initWasmEngine(undefined, {
    aesCbc256Encrypt() {
      return new Uint8Array();
    },
    aesCbc256Decrypt() {
      return new Uint8Array();
    },
    aesGcm256Encrypt(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, plaintext: Uint8Array) {
      const cipher = createCipheriv("aes-256-gcm", key, nonce);
      cipher.setAAD(aad);
      return new Uint8Array(
        Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()])
      );
    },
    aesGcm256Decrypt(
      key: Uint8Array,
      nonce: Uint8Array,
      aad: Uint8Array,
      ciphertext: Uint8Array
    ) {
      const decipher = createDecipheriv("aes-256-gcm", key, nonce);
      decipher.setAAD(aad);
      decipher.setAuthTag(ciphertext.slice(-16));
      return new Uint8Array(
        Buffer.concat([decipher.update(ciphertext.slice(0, -16)), decipher.final()])
      );
    },
    hmacSha256() {
      return new Uint8Array(32);
    },
  });
} else {
  initWasmEngine();
}

const key = new Uint8Array(32).fill(7);
const nonce = new Uint8Array(12).fill(8);
const aad = new Uint8Array([1, 2, 3]);
const sizes = [64, 2 ** 10, 2 ** 14, 2 ** 20];
function measure(name: string, operation: () => void, bytes: number) {
  const iterations = bytes >= 2 ** 20 ? 2 : bytes >= 2 ** 14 ? 10 : 100;
  for (let i = 0; i < Math.min(5, iterations); i++) operation();
  const start = performance.now();
  for (let i = 0; i < iterations; i++) operation();
  const elapsed = performance.now() - start;
  const mib = (bytes * iterations) / 2 ** 20;
  console.log(`${host ? "host" : "rust"} ${name.padEnd(7)} ${bytes} B: ${(elapsed / iterations).toFixed(3)} ms/op, ${(mib / (elapsed / 1000)).toFixed(1)} MiB/s`);
}

for (const size of sizes) {
  const plaintext = new Uint8Array(size).fill(9);
  const sealed = aesGcm256Encrypt(key, nonce, aad, plaintext);
  measure("encrypt", () => aesGcm256Encrypt(key, nonce, aad, plaintext), size);
  measure("decrypt", () => aesGcm256Decrypt(key, nonce, aad, sealed), size);
  measure("sha256", () => sha256(plaintext), size);
}
