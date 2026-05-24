#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

cd "$ROOT"
echo "==> cargo test"
cargo test --quiet

echo "==> cargo build --release"
cargo build --release --quiet

BIN="${ROOT}/target/release/smr"
PORT=18080
CFG="${ROOT}/config/smr.example.yaml"

# patch listen port for test
TMP_CFG=$(mktemp)
sed "s/127.0.0.1:8080/127.0.0.1:${PORT}/" "$CFG" > "$TMP_CFG"

"$BIN" --config "$TMP_CFG" &
PID=$!
sleep 1

cleanup() { kill "$PID" 2>/dev/null || true; rm -f "$TMP_CFG"; }
trap cleanup EXIT

echo "==> health"
curl -sf "http://127.0.0.1:${PORT}/health" | grep -q OK

echo "==> api status"
curl -sf "http://127.0.0.1:${PORT}/api/status" | grep -q proxy_url

echo "==> ui"
curl -sf "http://127.0.0.1:${PORT}/ui" | grep -q SecureModelRoute

echo ""
echo "All verification checks passed."
