# Synchronous crypto bridge evidence

The bridge exports `aesGcm256Encrypt`, `aesGcm256Decrypt`, and `sha256` from
both package entrypoints through `ts/surface.ts`. AES-GCM calls the active
`SignalCryptoProvider`, so `initWasmEngine(undefined, callbacks)` keeps the
existing hybrid routing: small payloads stay in Rust and large payloads use the
host callbacks. The exports return `Uint8Array`; AES output is `ciphertext ||
16-byte tag`. Wrong key/nonce/tag shapes are `invalid-argument`, while provider
and authentication failures are structured `crypto` errors.

Known-answer, callback-provider parity, host-loaded smoke, and factory
initialization-barrier coverage:

```sh
bun run build
bun test tests/crypto-exports.test.ts tests/device-account-authority.test.ts
```

The provider parity test runs Rust and callback-backed instances in isolated
child processes because the core provider is process-global and install-once.

## Local measurements

On the release artifact built in this worktree (Bun 1.4.2, Linux arm64):

| artifact | bytes |
|---|---:|
| `pkg/whatsapp_rust_bridge_bg.wasm` | 5,940,536 |
| `dist/bridge.js` | 1,279,121 |

`bun run check:wasm-shape` passed with a 100,514-byte largest function against
the 110,000-byte budget. The package-size gate reported 8.37 MB against its
8.30 MB budget in this environment; that existing Bun/toolchain-sensitive gate
was not raised for these exports. Re-run `bun run check:size` with the
repository's CI toolchain before release.

The benchmark is reproducible with the same release build:

```sh
bun run benches/crypto.ts
CRYPTO_PROVIDER=host bun run benches/crypto.ts
```

At 1 MiB, this run measured Rust AES-GCM encrypt/decrypt at 20.055/19.724
ms per operation and host-routed encrypt/decrypt at 1.894/2.966 ms. SHA-256
remained Rust-routed and measured 6.402 ms per operation.
