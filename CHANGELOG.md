# Changelog

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
