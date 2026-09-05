#!/bin/sh
# Guard: the published bridge artifact must verify the Noise cert chain.
#
# Authority is the resolved normal-dependency feature graph for the shipped
# target (wasm32-unknown-unknown, default features), not manifest text: a
# comment, a doc mention, or an explicit non-default mock edge must not fail
# this, and Cargo.lock records versions rather than enabled features.
#
# Runs in CI (`build-and-test` in ci.yml, `verify` in release.yml) and locally
# as `sh scripts/check-noise-cert-profile.sh`.
set -eu

cd "$(dirname "$0")/.."

fail() {
    echo "check-noise-cert-profile: FAIL: $1" >&2
    exit 1
}

TREE=$(cargo tree --target wasm32-unknown-unknown -e normal -f "{p} {f}" 2>/dev/null) \
    || fail "cargo tree for wasm32-unknown-unknown did not run"
if printf '%s\n' "$TREE" | grep -q "danger-skip-cert-chain-verify"; then
    printf '%s\n' "$TREE" | grep "danger-skip-cert-chain-verify" >&2
    fail "resolved production graph enables danger-skip-cert-chain-verify"
fi

echo "check-noise-cert-profile: OK"
