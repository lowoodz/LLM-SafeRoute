#!/usr/bin/env bash
# Build SecureModelRoute-*-Setup.exe on UTM Windows guest (IExpress, fully local).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VM_ID="${SMR_UTM_VM:-Windows}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
DIST="${ROOT}/dist"
VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
STAGE_GUEST="C:/Users/Public/smr-setup-stage"
OUT_GUEST="C:/Users/Public/smr-setup-out"
BUILD_PS1_GUEST="C:/Users/Public/build-windows-setup.ps1"
SETUP_NAME="SecureModelRoute-${VERSION}-x64-Setup.exe"

mkdir -p "$DIST"

CLI="${DIST}/smr.exe"
GUI="${DIST}/windows-desktop/SecureModelRoute.exe"
[[ -s "$CLI" ]] || CLI="${DIST}/target/x86_64-pc-windows-gnu/release/smr.exe"
[[ -s "$GUI" ]] || {
  echo "Missing GUI exe. Run: ./scripts/package-windows-desktop.sh" >&2
  exit 1
}
[[ -s "$CLI" ]] || {
  echo "Missing smr.exe. Run: ./scripts/package-windows.sh" >&2
  exit 1
}

echo "==> Upload installer payload to guest"
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "mkdir C:\\Users\\Public\\smr-setup-stage 2>nul" 2>/dev/null || true
cat "$CLI" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/smr.exe"
cat "$GUI" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/SecureModelRoute.exe"
cat "${ROOT}/config/smr.example.yaml" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/smr.example.yaml"
cat "${ROOT}/scripts/install.ps1" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/install.ps1"
cat "${ROOT}/scripts/windows/build-setup.ps1" | "$UTMCTL" file push "$VM_ID" "$BUILD_PS1_GUEST"

echo "==> Build Setup.exe on guest (IExpress)..."
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "del /q C:\\Users\\Public\\smr-setup-out\\build-setup.log 2>nul & del /q C:\\Users\\Public\\smr-setup-out\\SecureModelRoute-*-Setup.exe 2>nul" 2>/dev/null || true
"$UTMCTL" exec "$VM_ID" --cmd powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:/Users/Public/build-windows-setup.ps1" -Version "$VERSION" >/dev/null 2>&1 || true

LOG_GUEST="${OUT_GUEST}/build-setup.log"
LOG_LOCAL="${DIST}/windows-setup-build.log"
OUT_LOCAL="${DIST}/${SETUP_NAME}"
rm -f "$OUT_LOCAL"

echo "    Waiting for guest build..."
for _ in $(seq 1 120); do
  "$UTMCTL" file pull "$VM_ID" "$LOG_GUEST" > "$LOG_LOCAL" 2>/dev/null || true
  if [[ -s "$LOG_LOCAL" ]] && grep -q "==> Setup:" "$LOG_LOCAL"; then
    break
  fi
  sleep 2
done

if [[ -s "$LOG_LOCAL" ]]; then
  tail -3 "$LOG_LOCAL"
fi

"$UTMCTL" file pull "$VM_ID" "${OUT_GUEST}/${SETUP_NAME}" > "$OUT_LOCAL" 2>/dev/null || true
if [[ ! -s "$OUT_LOCAL" ]]; then
  echo "Failed to pull ${SETUP_NAME}" >&2
  [[ -s "$LOG_LOCAL" ]] && cat "$LOG_LOCAL" >&2
  exit 1
fi

FULL_ZIP="${DIST}/smr-${VERSION}-windows-x86_64-full.zip"
rm -f "$FULL_ZIP"
(
  cd "$DIST"
  zip -q "$FULL_ZIP" "$SETUP_NAME"
)

echo "==> Windows one-click installer: $OUT_LOCAL"
ls -lh "$OUT_LOCAL" "$FULL_ZIP"
