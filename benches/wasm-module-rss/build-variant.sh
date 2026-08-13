#!/usr/bin/env bash
# Build one artifact the way the published one is built — cargo release,
# wasm-bindgen, then the wasm-opt pass list from Cargo.toml — into
# benches/wasm-module-rss/artifacts/<name>.wasm.
#
# `wasm-pack` is not used: it downloads its own wasm-opt at build time, and the
# point here is to vary only the feature set between artifacts.
#
#   build-variant.sh <name> [--features …] [--config …]
#
# WASM_BINDGEN and WASM_OPT name the two binaries; set them if they are not on
# PATH. NAMED=1 keeps the name section (for attribute.mjs) — it changes the
# artifact, so never measure a NAMED build against a stripped one.
set -euo pipefail

name="$1"
shift

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="$repo/benches/wasm-module-rss/artifacts"
mkdir -p "$out"

wasm_bindgen="${WASM_BINDGEN:-wasm-bindgen}"
wasm_opt="${WASM_OPT:-wasm-opt}"

target_dir="$repo/target/variant-$name"
strip_args=()
if [ "${NAMED:-0}" = "1" ]; then
  strip_args=(--config 'profile.release.strip="none"')
fi

cargo build --release --target wasm32-unknown-unknown \
  --target-dir "$target_dir" "${strip_args[@]}" "$@"

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

bindgen_args=(--target web --out-dir "$staging")
[ "${NAMED:-0}" = "1" ] && bindgen_args+=(--keep-debug)
"$wasm_bindgen" "${bindgen_args[@]}" \
  "$target_dir/wasm32-unknown-unknown/release/whatsapp_rust_bridge.wasm"

# The pass list is [package.metadata.wasm-pack.profile.release] in Cargo.toml.
opt_passes=(
  -O4
  --gufa-optimizing
  --inlining-optimizing
  --ignore-implicit-traps
  --traps-never-happen
  --coalesce-locals-learning
  --converge
  --enable-simd
  --enable-bulk-memory
  --enable-nontrapping-float-to-int
  --enable-sign-ext
  --enable-mutable-globals
  --enable-multivalue
  --fast-math
  --zero-filled-memory
  --dce
  --vacuum
  --directize
  --optimize-stack-ir
  --merge-similar-functions
  --one-caller-inline-max-function-size 2000
)
[ "${NAMED:-0}" = "1" ] || opt_passes+=(--strip-debug)

"$wasm_opt" "${opt_passes[@]}" \
  "$staging/whatsapp_rust_bridge_bg.wasm" -o "$out/$name.wasm"

cp "$staging/whatsapp_rust_bridge.js" "$out/$name.glue.js"
cp "$staging/whatsapp_rust_bridge.d.ts" "$out/$name.d.ts"
ls -l "$out/$name.wasm"
