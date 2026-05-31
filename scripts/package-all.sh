#!/usr/bin/env bash
# Build all platform packages: macOS (arm64 + x86_64) and Windows (x86_64).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "$(uname -s)" == "Darwin" ]]; then
  bash "${ROOT}/scripts/package-macos.sh"
else
  bash "${ROOT}/scripts/package.sh"
fi

echo ""
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
if [[ -x "${UTMCTL}" ]] && "${UTMCTL}" list 2>/dev/null | grep -q started; then
  echo "==> UTM VM running: building Windows desktop (native arch on guest)"
  bash "${ROOT}/scripts/vm/package-windows-gui.sh" || echo "Warning: Windows desktop build failed (see dist/windows-desktop-build.log)"
else
  echo "==> No UTM VM: skip Windows desktop (run .\\scripts\\package.ps1 on Windows, or start UTM VM)"
fi
bash "${ROOT}/scripts/package-windows.sh"

if [[ -x "${UTMCTL}" ]] && "${UTMCTL}" list 2>/dev/null | grep -q started; then
  if [[ -s "${ROOT}/dist/windows-desktop/SecureModelRoute.exe" ]]; then
    echo ""
    echo "==> Building Windows one-click Setup.exe"
    bash "${ROOT}/scripts/vm/package-windows-setup.sh" || echo "Warning: Setup.exe build failed"
  fi
fi

echo ""
echo "==> All packages in ${ROOT}/dist/"
ls -lh "${ROOT}/dist/"*.tar.gz "${ROOT}/dist/"*.zip 2>/dev/null || true
