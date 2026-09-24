#!/usr/bin/env bash
# verify_provenance.sh — functional provenance check for the vendored
# Lighter signer binary.
#
# What it proves:
#   The vendored liblighter_signer.so derives the same public key from a
#   fixed test private key and produces signatures that the PINNED
#   UPSTREAM SOURCE's own schnorr.Validate accepts. That demonstrates the
#   vendored binary implements the same key derivation, field/curve
#   arithmetic, and transaction hashing as the pinned commit.
#
# What it does NOT prove:
#   Byte-level reproduction — Go c-shared builds are not deterministic
#   across build environments (digests legitimately differ). A malicious
#   binary could still contain extra unexercised code; this check covers
#   the signing surface the adapter actually calls.
#
# Requirements: git, go >= 1.24, gcc (for the dlopen driver).
#
# Exit 0 = vendored binary verified against pinned source.

set -euo pipefail

# Pinned upstream source. Both must be updated together.
LIGHTER_REPO="https://github.com/elliottech/lighter-go.git"
LIGHTER_TAG="v1.0.10"
LIGHTER_COMMIT="9d38261d1a4cc5c7211b383ba07a4d6e41604708"

# Fixed throwaway test inputs (match dump_cancel.c).
TEST_KEY="0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
CHAIN_ID="304"
# Expected pubkey derived from TEST_KEY by the pinned implementation.
EXPECTED_PUBKEY="0xdd463e68f02bcf06a4a0d7af954adb68f436c7b409a2baae4d60dff5178f8f615f8f1a756d04d528"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENDORED_SO="${LIGHTER_VENDORED_SO:-$SCRIPT_DIR/../liblighter_signer.so}"
WORK="${LIGHTER_VERIFY_WORK:-$(mktemp -d)}"

command -v go >/dev/null || { echo "error: go not found (need >= 1.24)" >&2; exit 2; }
command -v gcc >/dev/null || { echo "error: gcc not found" >&2; exit 2; }
command -v git >/dev/null || { echo "error: git not found" >&2; exit 2; }
[ -f "$VENDORED_SO" ] || { echo "error: vendored .so not found at $VENDORED_SO" >&2; exit 2; }

echo "==> cloning $LIGHTER_REPO @ $LIGHTER_TAG ($LIGHTER_COMMIT)"
git clone --quiet "$LIGHTER_REPO" "$WORK/lighter-go"
git -C "$WORK/lighter-go" checkout --quiet "$LIGHTER_COMMIT"

echo "==> building signer from pinned source (c-shared, -trimpath)"
(
    cd "$WORK/lighter-go"
    CGO_ENABLED=1 GOOS=linux GOARCH=amd64 \
        go build -buildmode=c-shared -trimpath \
        -o "$WORK/rebuilt-signer.so" ./sharedlib
)

echo "==> compiling dlopen probes"
gcc -O1 -o "$WORK/dump_cancel" "$SCRIPT_DIR/dump_cancel.c" -ldl
gcc -O1 -o "$WORK/dump_create_order" "$SCRIPT_DIR/dump_create_order.c" -ldl

echo "==> signing fixed cancel-order tx with vendored binary"
"$WORK/dump_cancel" "$VENDORED_SO" "$WORK/vendored_tx.json"

echo "==> signing fixed cancel-order tx with rebuilt binary"
"$WORK/dump_cancel" "$WORK/rebuilt-signer.so" "$WORK/rebuilt_tx.json"

echo "==> signing fixed create-order tx with vendored binary"
"$WORK/dump_create_order" "$VENDORED_SO" "$WORK/vendored_create.json"

echo "==> signing fixed create-order tx with rebuilt binary"
"$WORK/dump_create_order" "$WORK/rebuilt-signer.so" "$WORK/rebuilt_create.json"

echo "==> verifying signatures against pinned source (schnorr.Validate)"
mkdir -p "$WORK/lighter-go/verifytool"
cp "$SCRIPT_DIR/verifytool/main.go" "$WORK/lighter-go/verifytool/main.go"

check_sig() { # <label> <txinfo> <kind>
    local out
    out=$(cd "$WORK/lighter-go" && go run ./verifytool "$2" "$TEST_KEY" "$CHAIN_ID" "$3")
    echo "$1: $out"
    case "$out" in
        *"VERIFY-OK pubkey=$EXPECTED_PUBKEY"*) ;;
        *) echo "FAIL: $1 $3 signature did not verify under expected pubkey" >&2; exit 1 ;;
    esac
}

check_sig vendored "$WORK/vendored_tx.json" cancel
check_sig rebuilt  "$WORK/rebuilt_tx.json" cancel
check_sig vendored "$WORK/vendored_create.json" create
check_sig rebuilt  "$WORK/rebuilt_create.json" create

# Structural equivalence: non-signature payload fields must be identical.
norm() { sed -E 's/"ExpiredAt":[0-9]+//; s/"Sig":"[^"]*"//; s/,,/,/g; s/\{,/\{/g; s/,\}/\}/g' "$1"; }
for kind in tx create; do
    if ! diff <(norm "$WORK/vendored_$kind.json") <(norm "$WORK/rebuilt_$kind.json") >/dev/null; then
        echo "FAIL: non-signature $kind fields differ between vendored and rebuilt" >&2
        diff <(norm "$WORK/vendored_$kind.json") <(norm "$WORK/rebuilt_$kind.json") >&2 || true
        exit 1
    fi
done

echo "PASS: vendored signer is functionally equivalent to $LIGHTER_TAG@$LIGHTER_COMMIT (cancel + create-order paths)"
[ "${LIGHTER_VERIFY_WORK:-}" ] || rm -rf "$WORK"
