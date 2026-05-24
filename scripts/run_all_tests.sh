#!/usr/bin/env bash
# Run unit tests, smoke verify, black-box scenarios, and stress tests.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

echo "========== 1/4 Unit + smoke (verify.sh) =========="
bash scripts/verify.sh

if [[ ! -f test_model_api_key.txt ]]; then
  echo "Skip live tests: test_model_api_key.txt not found"
  exit 0
fi

echo ""
echo "========== 2/4 Black-box scenarios =========="
python3 scripts/blackbox_test.py

echo ""
echo "========== 3/4 Stress tests =========="
python3 scripts/live_test.py

echo ""
echo "========== All test suites passed =========="
