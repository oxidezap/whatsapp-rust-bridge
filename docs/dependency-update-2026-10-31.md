# Dependency audit, 2026-10-31

The update starts at bridge `5b9f22fd91a2f9f833986d023fd94bd9364ebeef`.
Stable versions were checked against the crates.io sparse index, npm `latest`
dist-tags, official release tags, Rust's stable channel manifest and Node's
release index. Prereleases and yanked crates were excluded.

## Direct dependencies

The versions below are manifest minimums, with the corresponding versions in
both Cargo lockfiles and both JavaScript lockfiles. Cargo features stay unchanged.

| Registry | Dependency | Version |
| --- | --- | --- |
| crate | `anyhow` | 1.0.104 |
| crate | `async-channel` | 2.5.0 |
| crate | `async-lock` | 3.4.2 |
| crate | `async-trait` | 0.1.92 |
| crate | `base64` | 0.23.1 |
| crate | `bytes` | 1.12.1 |
| crate | `console_error_panic_hook` | 0.1.7 |
| crate | `console_log` | 1.1.0 |
| crate | `futures` | 0.3.34 |
| crate | `getrandom` | 0.4.3 |
| crate | `image` | 0.25.10 |
| crate | `img-parts` | 0.4.1 |
| crate | `js-sys` | 0.3.107 |
| crate | `log` | 0.4.35 |
| crate | `serde` | 1.0.229 |
| crate | `serde-wasm-bindgen` | 0.6.5 |
| crate | `serde_bytes` | 0.11.19 |
| crate | `serde_json` | 1.0.154 |
| crate | `symphonia` | 0.6.1 |
| crate | `thiserror` | 2.0.21 |
| crate | `tracing` | 0.1.44 |
| crate | `tracing-core` | 0.1.36 |
| crate | `tsify` | 0.5.8 |
| crate | `uuid` | 1.29.0 |
| crate | `wasm-bindgen` | 0.2.130 |
| crate | `wasm-bindgen-futures` | 0.4.80 |
| crate | `wasm-streams` | 0.7.0 |
| crate | `web-sys` | 0.3.107 |
| crate | `flate2` | 1.1.10 |
| crate | `wasm-bindgen-test` | 0.3.80 |
| crate | `proc-macro2` | 1.0.107 |
| crate | `quote` | 1.0.47 |
| crate | `syn` | 3.0.9 |
| crate | `walkdir` | 2.5.0 |
| npm | `@bufbuild/protobuf` | 2.16.0 |
| npm | `@types/bun` | 1.4.4 |
| npm | `@types/node` | 26.6.6 |
| npm | `pkg-pr-new` | 0.0.93 |
| npm | `ts-proto` | 2.18.3 |
| npm | `typescript` | 7.0.2 |

`whatsapp-rust` and its workspace crates use main commit
`4e3c8fb12f006284415b60287eeae8fdab7e5a22`, replacing
`633f01480fe3a95510172c6312d01484563dcf3c`.
[Upstream changes](https://github.com/oxidezap/whatsapp-rust/compare/633f01480fe3a95510172c6312d01484563dcf3c...4e3c8fb12f006284415b60287eeae8fdab7e5a22)
include bounded app-state buffers and receive queues, history-sync recovery,
newsletter poll fixes, Signal skipped-key retention and group compatibility.
VoIP, SQLite, native HTTP and upstream tooling changes do not enable those
backends in the bridge's existing feature set.

## Toolchain and actions

| Tool | Version |
| --- | --- |
| Rust, rustfmt, clippy | 1.99.1 stable |
| Node | 26.13.0 |
| Bun | 1.4.4 |
| npm for trusted publishing | 12.2.1 |
| wasm-pack | 0.15.1 |
| protoc | 36.2, install-action notation `3.36.2` |
| Binaryen / wasm-opt | 133 |
| actions/checkout | 7.0.1 |
| actions/setup-node | 7.1.0 |
| actions/create-github-app-token | 3.3.0 |
| actions/upload-artifact | 7.0.2 |
| actions/download-artifact | 8.0.2 |
| googleapis/release-please-action | 5.0.0 |
| Swatinem/rust-cache | 2.9.3 |
| taiki-e/install-action | 2.88.1 |
| oven-sh/setup-bun | 2.2.0 |
| autofix-ci/action | 1.3.4 |

The existing dtolnay/rust-toolchain master SHA remains current at
`683b588c487a69d01f6c6ddf40f036ff682cc2c8`. Actions already at their latest
stable version remain unchanged. Binaryen 133 is installed with a checked
archive digest because wasm-pack 0.15.1 otherwise downloads Binaryen 130.
CI and release use the same producer toolchain.

## Compatibility changes

TypeScript 7 no longer exports the JavaScript compiler API from its root.
Build tools and declaration tests now use the native asynchronous API and AST
exports. The asynchronous service works under Bun without the Node-only file
descriptors required by the synchronous API. Every service is closed after its
project is consumed. Strict consumer checks still cover Bundler and NodeNext.
The scanner's EOF enum and property optionality representation also changed.

The newer js-sys Promise type needs explicit `JsValue` payloads in two host
adapters. The generated clear-chat wrapper translates the existing two JS
booleans to the core's typed options. Public teardown methods retain their
existing void result; the new core shutdown/logout reports are not exported.
Generated core declarations include new vocabulary and an optional per-user
call error. The protobuf schema itself is unchanged; generator metadata and
the new generated `protobufFileName` constant changed.

## Transitive restrictions

Every resolvable lockfile package was refreshed. These older lines remain
required by the latest direct or upstream dependencies. They are not a claim
that every transitive package equals its registry's newest version.

| Older locked line | Latest stable | Parent constraint |
| --- | --- | --- |
| base64 0.22.1 | 0.23.1 | buffa 0.9.2 uses the 0.22 API |
| foldhash 0.1.5 | 0.2.0 | buffa 0.9.2 and hashbrown 0.15.5 |
| hashbrown 0.15.5 | 0.17.1 | buffa 0.9.2 |
| minicov 0.3.8 | 0.3.9 | wasm-bindgen-test 0.3.80 requires `=0.3.8` |
| miniz_oxide 0.8.9 | 0.9.1 | png 0.18.1, optional image feature |
| moxcms 0.8.1 | 0.9.1 | image 0.25.10, optional image feature |
| prettyplease 0.2.37 | 0.3.0 | buffa-codegen 0.9.2 |
| r-efi 6.0.0 | 7.1.0 | getrandom 0.4.3, UEFI target |
| serde_derive_internals 0.29.1 | 0.30.0 | tsify-macros 0.5.8 |
| syn 2.0.119 | 3.0.9 | buffa-codegen, curve25519-dalek-derive, prettyplease and tsify-macros |
| windows-link 0.2.1 | 0.100.0 | windows-sys 0.61.2, Windows host tooling |

Forcing minicov 0.3.9 fails Cargo resolution with `minicov = "=0.3.8"`
required by wasm-bindgen-test. The other older minor/major lines similarly
cannot be upgraded across their parents' semver constraints without patching
third-party crates. Both old and current lines coexist where required.
