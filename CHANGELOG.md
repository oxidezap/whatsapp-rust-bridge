# Changelog

## [0.16.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.15.0...v0.16.0) (2026-08-19)


### ⚠ BREAKING CHANGES

* **deps:** update whatsapp-rust so a stranded group member is keyed again ([#73](https://github.com/oxidezap/whatsapp-rust-bridge/issues/73))

### Bug Fixes

* **deps:** update whatsapp-rust so a stranded group member is keyed again ([#73](https://github.com/oxidezap/whatsapp-rust-bridge/issues/73)) ([c2e5d33](https://github.com/oxidezap/whatsapp-rust-bridge/commit/c2e5d33e95c1922cc7762ecadeb779ee2e72a307))

## [0.15.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.14.0...v0.15.0) (2026-08-18)


### Features

* **messaging:** let an edit carry a caller-supplied stanza id ([#70](https://github.com/oxidezap/whatsapp-rust-bridge/issues/70)) ([4f4ba2e](https://github.com/oxidezap/whatsapp-rust-bridge/commit/4f4ba2e4e92011b5943f2e90f24ef2df23c5bd70))

## [0.14.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.13.0...v0.14.0) (2026-08-17)


### ⚠ BREAKING CHANGES

* **deps:** a message's envelope type and mediatype cross typed, and a build retirement deadline becomes an event ([#67](https://github.com/oxidezap/whatsapp-rust-bridge/issues/67))

### Features

* **deps:** a message's envelope type and mediatype cross typed, and a build retirement deadline becomes an event ([#67](https://github.com/oxidezap/whatsapp-rust-bridge/issues/67)) ([7df72f8](https://github.com/oxidezap/whatsapp-rust-bridge/commit/7df72f8e2e104954829d5d4879e0d91e967b8b83))
* **errors:** carry the server's retry delay on a rejection ([#66](https://github.com/oxidezap/whatsapp-rust-bridge/issues/66)) ([e95d64c](https://github.com/oxidezap/whatsapp-rust-bridge/commit/e95d64c371f42d08009b187c207ead1f9edabca9))

## [0.13.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.12.0...v0.13.0) (2026-08-14)


### ⚠ BREAKING CHANGES

* **deps:** update whatsapp-rust so a stranded participant recovers, and report a send that reached nobody ([#64](https://github.com/oxidezap/whatsapp-rust-bridge/issues/64))

### Bug Fixes

* **deps:** update whatsapp-rust so a stranded participant recovers, and report a send that reached nobody ([#64](https://github.com/oxidezap/whatsapp-rust-bridge/issues/64)) ([472e6d8](https://github.com/oxidezap/whatsapp-rust-bridge/commit/472e6d80acf45d7e63e2f34f19ad64522a6ed571))

## [0.12.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.11.0...v0.12.0) (2026-08-13)


### ⚠ BREAKING CHANGES

* **wire:** hold the message string table across batches, as the receipt one does ([#58](https://github.com/oxidezap/whatsapp-rust-bridge/issues/58))

### Bug Fixes

* **wire:** read the packed string region in the unit the wire writes ([#57](https://github.com/oxidezap/whatsapp-rust-bridge/issues/57)) ([c1c0fb0](https://github.com/oxidezap/whatsapp-rust-bridge/commit/c1c0fb081d895d818a88ecaca0a39b2f0ef89d5e))


### Performance

* let a source build drop the domains it does not use ([#62](https://github.com/oxidezap/whatsapp-rust-bridge/issues/62)) ([a7aabec](https://github.com/oxidezap/whatsapp-rust-bridge/commit/a7aabec500c02af4b012a1ce36080d490fec6a31))
* **proto:** measure what the 657 generated codecs cost, and which fix pays ([#60](https://github.com/oxidezap/whatsapp-rust-bridge/issues/60)) ([5383497](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5383497a0abd5bd3faac551cc983333a6ebce636))
* pull the inlining lever [#61](https://github.com/oxidezap/whatsapp-rust-bridge/issues/61) measured, and take the core's msg_secrets bound ([#63](https://github.com/oxidezap/whatsapp-rust-bridge/issues/63)) ([80c36cc](https://github.com/oxidezap/whatsapp-rust-bridge/commit/80c36cc957dd28fdc01a77fd4c11a0b8ed1613b6))
* **wasm:** name the three functions that dominate V8's compile-zone peak, and price the one lever ([#61](https://github.com/oxidezap/whatsapp-rust-bridge/issues/61)) ([5d10541](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5d105418bf88e37e5372d7e5836287857a87c1b9))
* **wire:** hold the message string table across batches, as the receipt one does ([#58](https://github.com/oxidezap/whatsapp-rust-bridge/issues/58)) ([d5b6b38](https://github.com/oxidezap/whatsapp-rust-bridge/commit/d5b6b38c5028fef53d0d5cdd7c262b89afe4d2ef))

## [0.11.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.10.0...v0.11.0) (2026-08-12)


### ⚠ BREAKING CHANGES

* **deps:** update whatsapp-rust so a caller's <biz> no longer duplicates the derived one ([#55](https://github.com/oxidezap/whatsapp-rust-bridge/issues/55))

### Bug Fixes

* **deps:** update whatsapp-rust so a caller's &lt;biz&gt; no longer duplicates the derived one ([#55](https://github.com/oxidezap/whatsapp-rust-bridge/issues/55)) ([7355996](https://github.com/oxidezap/whatsapp-rust-bridge/commit/7355996ab80348edf19d6fc2a25b30ac37de4434))

## [0.10.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.9.0...v0.10.0) (2026-08-12)


### ⚠ BREAKING CHANGES

* **events:** deliver the core events the bridge was dropping ([#54](https://github.com/oxidezap/whatsapp-rust-bridge/issues/54))
* **deps:** update whatsapp-rust so a degraded sync still announces the connection ([#51](https://github.com/oxidezap/whatsapp-rust-bridge/issues/51))

### Features

* **events:** deliver the core events the bridge was dropping ([#54](https://github.com/oxidezap/whatsapp-rust-bridge/issues/54)) ([2a27217](https://github.com/oxidezap/whatsapp-rust-bridge/commit/2a27217587292e044be7b5647d661e065090cd62))


### Bug Fixes

* **codegen:** point generated declarations at the proto types they name ([#52](https://github.com/oxidezap/whatsapp-rust-bridge/issues/52)) ([4f5eccd](https://github.com/oxidezap/whatsapp-rust-bridge/commit/4f5eccdbc9dee649da6b9770ccaccee9ec994c83))
* **deps:** update whatsapp-rust so a degraded sync still announces the connection ([#51](https://github.com/oxidezap/whatsapp-rust-bridge/issues/51)) ([5b2a9d0](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5b2a9d00c1b600972d92b6c2764e645fb17018de))

## [0.9.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.8.1...v0.9.0) (2026-08-11)


### ⚠ BREAKING CHANGES

* **proto:** agree on where a field ends and how deep a message goes ([#49](https://github.com/oxidezap/whatsapp-rust-bridge/issues/49))

### Bug Fixes

* **proto:** agree on where a field ends and how deep a message goes ([#49](https://github.com/oxidezap/whatsapp-rust-bridge/issues/49)) ([0bddd55](https://github.com/oxidezap/whatsapp-rust-bridge/commit/0bddd558eadb33806d493622abb9e940d5e16999))

## [0.8.1](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.8.0...v0.8.1) (2026-08-10)


### Features

* **build:** add a BoltFFI-generated WASM artifact alongside wasm-bindgen ([#33](https://github.com/oxidezap/whatsapp-rust-bridge/issues/33)) ([5ca9cf9](https://github.com/oxidezap/whatsapp-rust-bridge/commit/5ca9cf94b8cc10888663de9a69eff9a897d058e9))


### Bug Fixes

* **proto:** read framed payloads the way the wire format defines them ([#44](https://github.com/oxidezap/whatsapp-rust-bridge/issues/44)) ([470d554](https://github.com/oxidezap/whatsapp-rust-bridge/commit/470d554ba23e007be0c0b26911bb49b00d8d74d1))


### Chores

* release 0.8.1 ([13e18f6](https://github.com/oxidezap/whatsapp-rust-bridge/commit/13e18f61d3975bb42eb438caa55f89ba2f4d02c8))

## [0.8.0](https://github.com/oxidezap/whatsapp-rust-bridge/compare/v0.7.2...v0.8.0) (2026-08-10)


### ⚠ BREAKING CHANGES

* **proto:** decode 64-bit fields outside the safe range instead of failing the message ([#42](https://github.com/oxidezap/whatsapp-rust-bridge/issues/42))

### Bug Fixes

* **proto:** decide and document what the codec does with invalid UTF-8 ([#39](https://github.com/oxidezap/whatsapp-rust-bridge/issues/39)) ([41618fa](https://github.com/oxidezap/whatsapp-rust-bridge/commit/41618fa481d737c05e98c05595fa7f911e383768))
* **proto:** decode 64-bit fields outside the safe range instead of failing the message ([#42](https://github.com/oxidezap/whatsapp-rust-bridge/issues/42)) ([f12e8c9](https://github.com/oxidezap/whatsapp-rust-bridge/commit/f12e8c932f916a96ae8e33264f8d72a3ad3679fe))
* **proto:** make the numeric input contract consistent and documented ([#38](https://github.com/oxidezap/whatsapp-rust-bridge/issues/38)) ([79ca1f2](https://github.com/oxidezap/whatsapp-rust-bridge/commit/79ca1f25afff12bb8774e1d0168358a55105e03a))

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
