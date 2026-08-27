# @oxidezap/whatsapp-rust-bridge

[![npm version](https://img.shields.io/npm/v/@oxidezap/whatsapp-rust-bridge)](https://www.npmjs.com/package/@oxidezap/whatsapp-rust-bridge)
[![npm downloads](https://img.shields.io/npm/dm/@oxidezap/whatsapp-rust-bridge)](https://www.npmjs.com/package/@oxidezap/whatsapp-rust-bridge)
[![pkg.pr.new](https://pkg.pr.new/badge/oxidezap/whatsapp-rust-bridge)](https://pkg.pr.new/~/oxidezap/whatsapp-rust-bridge)

High-performance WhatsApp utilities powered by Rust and WebAssembly.

## Features

| Feature                        | Status |
| ------------------------------ | ------ |
| Binary Protocol                | ✅     |
| Libsignal                      | ✅     |
| App State Sync                 | ✅     |
| Audio (waveform, duration)     | ✅     |
| Image (thumbnails, conversion) | ✅     |
| Sticker Metadata               | ✅     |

## Preview builds

Every commit on `main` and every pull request that passes CI leaves an
installable build behind, through
[pkg.pr.new](https://github.com/stackblitz-labs/pkg.pr.new). Nothing is
published to npm — the tarball is served from a URL, so a fix can be tried
before it reaches a release:

```sh
# the head of a pull request, following it as new commits land
npm install https://pkg.pr.new/@oxidezap/whatsapp-rust-bridge@88

# one specific commit
npm install https://pkg.pr.new/@oxidezap/whatsapp-rust-bridge@7ef8db0
```

Every open pull request comments the URL for its own head, and
[pkg.pr.new/~/oxidezap/whatsapp-rust-bridge](https://pkg.pr.new/~/oxidezap/whatsapp-rust-bridge)
lists what is available.

The tarball is the one the run built and tested: the same `bun run build`
output, packed through the same `prepack` guard a real publish goes through.
What it is not is a release. Preview builds carry the version
`0.0.0-preview-<sha>`, which no range written for a real release can match — an
install is a deliberate pin, and it stays on that commit until you change it.
They are for trying a change, not for running one.
