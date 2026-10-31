#!/usr/bin/env bash
# CI and release use the same optimizer, ahead of wasm-pack's bundled version.
set -euo pipefail
archive="${RUNNER_TEMP:?}/binaryen-133.tar.gz"
destination="$RUNNER_TEMP/binaryen-133"
curl --fail --location --retry 3 \
  https://github.com/WebAssembly/binaryen/releases/download/version_133/binaryen-version_133-x86_64-linux.tar.gz \
  --output "$archive"
printf '%s  %s\n' 2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489 "$archive" | sha256sum --check
mkdir -p "$destination"
tar -xzf "$archive" --strip-components=1 -C "$destination"
"$destination/bin/wasm-opt" --version
printf '%s\n' "$destination/bin" >> "${GITHUB_PATH:?}"
