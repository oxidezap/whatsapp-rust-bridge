#!/bin/sh
# Guard: the published bridge artifact must verify the Noise cert chain.
#
# Fails if `danger-skip-cert-chain-verify` reaches the default-feature build,
# either written directly on the normal dependency or unified in through
# another edge (`default-features = false` on one edge does not cancel a
# feature enabled on another). There is no mock edge for this bypass: any
# occurrence is the production profile carrying it.
#
# Suggested CI invocation (package.json is owned by BR-02, so this stays a
# standalone executable until the coordinator wires it in):
#   sh scripts/check-noise-cert-profile.sh
set -eu

cd "$(dirname "$0")/.."

fail() {
    echo "check-noise-cert-profile: FAIL: $1" >&2
    exit 1
}

# 1. The manifest must not name the bypass at all.
if grep -q "danger-skip-cert-chain-verify" Cargo.toml; then
    grep -n "danger-skip-cert-chain-verify" Cargo.toml >&2
    fail "Cargo.toml enables danger-skip-cert-chain-verify"
fi
if [ -f Cargo.lock ] && grep -q "danger-skip-cert-chain-verify" Cargo.lock; then
    fail "Cargo.lock still resolves danger-skip-cert-chain-verify"
fi

# 2. The resolved normal-dependency graph for the shipped target must not
# contain the bypass feature, whatever edge would enable it.
TREE=$(cargo tree --offline --target wasm32-unknown-unknown -e normal -f "{p} {f}" 2>/dev/null) \
    || fail "cargo tree for wasm32-unknown-unknown did not run"
if printf '%s\n' "$TREE" | grep -q "danger-skip-cert-chain-verify"; then
    printf '%s\n' "$TREE" | grep "danger-skip-cert-chain-verify" >&2
    fail "resolved graph enables danger-skip-cert-chain-verify"
fi

# 3. The fix must not shrink `default`: every shipped domain stays on.
DEFAULT_BLOCK=$(sed -n '/^default = \[/,/^\]/p' Cargo.toml | tr -d '",' | tr '\n' ' ')
for feature in client-business client-chat-actions client-contacts client-groups \
    client-media client-newsletter client-signal legacy-session; do
    case " $DEFAULT_BLOCK " in
        *" $feature "*) ;;
        *) fail "default lost $feature" ;;
    esac
done

echo "check-noise-cert-profile: OK"
