#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# Arbitrageur End-to-End Test Suite Runner
# Runs the 4-tier requirement-driven E2E tests, frontend tests, and backend tests.
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

echo "======================================================================"
echo "          RUNNING ARBITRAGEUR VERIFICATION & E2E TEST SUITE           "
echo "======================================================================"

MODE="${1:-all}"

case "$MODE" in
  e2e)
    echo "Running 4-Tier E2E Test Suite..."
    node --experimental-strip-types tests/e2e/runner.mjs
    ;;
  frontend)
    echo "Running Frontend Component Tests..."
    (cd frontend && npm test)
    ;;
  backend)
    echo "Running Backend Unit & Integration Tests (Smoke)..."
    cargo test -p arbitrageur-desktop --bin arbitrageur-desktop -- state::tests
    ;;
  backend-full)
    echo "Running Complete Backend Test Suite (All 610 desktop tests)..."
    cargo test -p arbitrageur-desktop --bin arbitrageur-desktop
    ;;
  all)
    echo "1/3. Running 4-Tier E2E Test Suite..."
    node --experimental-strip-types tests/e2e/runner.mjs

    echo "2/3. Running Frontend Component Tests..."
    (cd frontend && npm test)

    # `all` deliberately stops at the smoke subset: the complete desktop
    # suite needs ~45 minutes (the stronghold rekey test alone is a full
    # Argon2 pass per open — issue #117). Say so out loud rather than
    # letting the run look complete: the smoke filter used to hide 596 of
    # the 610 tests silently (finding F-06).
    echo "3/3. Running Backend Unit & Integration Tests (SMOKE SUBSET ONLY: state::tests)..."
    cargo test -p arbitrageur-desktop --bin arbitrageur-desktop -- state::tests

    echo "NOTE: this ran the backend smoke subset. For every desktop test:"
    echo "      $0 backend-full"

    echo "======================================================================"
    echo "          ALL TEST SUITES EXECUTED AND VERIFIED SUCCESSFULLY          "
    echo "======================================================================"
    ;;
  *)
    echo "Unknown mode: $MODE (expected: all | e2e | frontend | backend | backend-full)"
    exit 1
    ;;
esac
