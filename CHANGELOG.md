# Changelog

## [0.7.2](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.7.1...v0.7.2) (2026-08-10)


### Bug Fixes

* **codegen:** unwrap Cow so the Rust name stops reaching TypeScript ([d55964c](https://github.com/oxidezap/whatsapp-rust-bridge/commit/d55964c3f68c34470e12a72b1dc80c9a4f890340))

## [0.7.1](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.7.0...v0.7.1) (2026-08-09)


### Bug Fixes

* **errors:** report a dropped connection as not-connected, not internal ([#34](https://github.com/oxidezap/whatsapp-rust-bridge/issues/34)) ([5c95d9b](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5c95d9b29ee0a9ab1d9eab8add66f80e99cb1e1e))

## [0.7.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.6.5...v0.7.0) (2026-08-08)


### Features

* **client:** expose app state label actions ([#21](https://github.com/oxidezap/whatsapp-rust-bridge/issues/21)) ([217a3f1](https://github.com/oxidezap/whatsapp-rust-bridge/commit/217a3f153f0d659ee33abb40948eab7c9b0f0e36))
* **client:** expose business catalog operations ([#24](https://github.com/oxidezap/whatsapp-rust-bridge/issues/24)) ([0bf06c4](https://github.com/oxidezap/whatsapp-rust-bridge/commit/0bf06c4aac9ee4b2922939b1a079e46f8d54e46d))
* **client:** expose lifecycle and server-side operations ([#20](https://github.com/oxidezap/whatsapp-rust-bridge/issues/20)) ([5a1742d](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5a1742d07ec393a3a3741c8bd4625dc05a5b0c2c))
* **client:** expose the remaining newsletter operations ([#23](https://github.com/oxidezap/whatsapp-rust-bridge/issues/23)) ([c07f5d3](https://github.com/oxidezap/whatsapp-rust-bridge/commit/c07f5d39a4af5bf28c13c632aa1f51f682b89be6))


### Bug Fixes

* **client:** reject a mistyped parameter instead of escaping the caller ([#25](https://github.com/oxidezap/whatsapp-rust-bridge/issues/25)) ([090b079](https://github.com/oxidezap/whatsapp-rust-bridge/commit/090b0797761ec8e5b7ef188de712b73b66d7c2ff))
* **client:** reject a non-stream instead of losing the call ([#30](https://github.com/oxidezap/whatsapp-rust-bridge/issues/30)) ([d9aa0dc](https://github.com/oxidezap/whatsapp-rust-bridge/commit/d9aa0dc985ee5534c6bfa81198eaa9cdc2cfe517))
* **client:** report a bad argument as invalid-argument, not internal ([#29](https://github.com/oxidezap/whatsapp-rust-bridge/issues/29)) ([be0e97d](https://github.com/oxidezap/whatsapp-rust-bridge/commit/be0e97d45977f947664cf942f22abc18fd4f2b81))
* **client:** stop deriving public strings from Debug ([#28](https://github.com/oxidezap/whatsapp-rust-bridge/issues/28)) ([5af4af3](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5af4af3364de9ada7d76879cd46a4188b2b35230))


### Refactors

* **client:** split the exported surface into per-domain modules ([#26](https://github.com/oxidezap/whatsapp-rust-bridge/issues/26)) ([f89bbc6](https://github.com/oxidezap/whatsapp-rust-bridge/commit/f89bbc6993eeeaddbd6d8f8322396804dcfcea5f))

## [0.6.5](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.6.4...v0.6.5) (2026-08-07)


### Performance

* **bridge:** coalesce the batches a single message produces ([#17](https://github.com/oxidezap/whatsapp-rust-bridge/issues/17)) ([35ddbba](https://github.com/oxidezap/whatsapp-rust-bridge/commit/35ddbba276e642226ecb05fb001252082527ab3f))
* **bridge:** let a host opt into borrowing the batch buffer ([#16](https://github.com/oxidezap/whatsapp-rust-bridge/issues/16)) ([5f1a989](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5f1a989e19bf41e9667b27e30f83a5ad17c16cd9))
* **bridge:** stop allocating per message on both sides of the boundary ([#13](https://github.com/oxidezap/whatsapp-rust-bridge/issues/13)) ([b1ce63e](https://github.com/oxidezap/whatsapp-rust-bridge/commit/b1ce63e486159bb4ef03e271095a33a6323b7e91))
* **deps:** compile curve25519 for speed instead of size ([#15](https://github.com/oxidezap/whatsapp-rust-bridge/issues/15)) ([18af7b2](https://github.com/oxidezap/whatsapp-rust-bridge/commit/18af7b205ae714dadf6b48296ada144bd2913429))

## [0.6.4](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.6.3...v0.6.4) (2026-08-07)


### Bug Fixes

* **codegen:** fail when the core sources are missing instead of emitting less ([#10](https://github.com/oxidezap/whatsapp-rust-bridge/issues/10)) ([7305764](https://github.com/oxidezap/whatsapp-rust-bridge/commit/7305764f360422ddecbae5e52afe01ace4bdf9c0))
* **deps:** take whatsapp-rust 0.7.0 from crates.io ([#12](https://github.com/oxidezap/whatsapp-rust-bridge/issues/12)) ([66c945d](https://github.com/oxidezap/whatsapp-rust-bridge/commit/66c945df0812b5c300e5a01521701210511c3b1f))

## [0.6.3](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.6.2...v0.6.3) (2026-08-07)


### Bug Fixes

* **ci:** correct the protoc pin, and build the package in CI ([c50c797](https://github.com/oxidezap/whatsapp-rust-bridge/commit/c50c7976e361cd3724a903dec44aa92f19042dbc))

## [0.6.2](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.6.1...v0.6.2) (2026-08-07)


### Bug Fixes

* **ci:** install protoc for the release build, and allow republishing a tag ([#4](https://github.com/oxidezap/whatsapp-rust-bridge/issues/4)) ([d6f5c44](https://github.com/oxidezap/whatsapp-rust-bridge/commit/d6f5c445acb9cc56524764b5abc61c5aa8d1e6f9))

## [0.6.1](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.6.0...v0.6.1) (2026-08-06)


### Bug Fixes

* **deps:** bump whatsapp-rust and correct two stale interop tests ([d01f0fa](https://github.com/oxidezap/whatsapp-rust-bridge/commit/d01f0fabaf92fdd6d36f83962b02fc8941d68c27))
* **deps:** bump whatsapp-rust and correct two stale interop tests ([61d8330](https://github.com/oxidezap/whatsapp-rust-bridge/commit/61d8330bc80c67ae9883dfd04edaaad7a2c3143c))
* **tests:** unbreak CI on main ([0c526d2](https://github.com/oxidezap/whatsapp-rust-bridge/commit/0c526d21c02616dcdbd4dcaf958fd582ef36e13a))
