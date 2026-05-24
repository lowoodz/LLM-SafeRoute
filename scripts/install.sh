#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

PREFIX="${SMR_INSTALL_PREFIX:-${HOME}/.local}"
BINDIR="${PREFIX}/bin"
CONFDIR="${PREFIX}/etc/securemodelroute"
INSTALL_SERVICE=false

for arg in "$@"; do
  case "$arg" in
    --service) INSTALL_SERVICE=true ;;
  esac
done

echo "==> Building release..."
cargo build --release

echo "==> Optional: build desktop GUI (requires npm)"
if command -v npm >/dev/null && [[ "${SMR_BUILD_GUI:-0}" == "1" ]]; then
  (cd gui/src-tauri && bash create_icon.sh)
  (cd gui && npm install --silent && CARGO_TARGET_DIR="${ROOT}/target" npm run build)
  APP_BUNDLE="${ROOT}/target/release/bundle/macos/SecureModelRoute.app"
  if [[ -d "$APP_BUNDLE" ]]; then
  echo "    GUI bundle: ${APP_BUNDLE}"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    DEST="${HOME}/Applications/SecureModelRoute.app"
    rm -rf "$DEST"
    cp -R "$APP_BUNDLE" "$DEST"
    echo "    Installed desktop app: ${DEST}"
  fi
  fi
fi

echo "==> Installing to ${PREFIX}"
mkdir -p "${BINDIR}" "${CONFDIR}"
install -m 755 "${ROOT}/target/release/smr" "${BINDIR}/smr"

if [[ ! -f "${CONFDIR}/smr.yaml" ]]; then
  install -m 644 "${ROOT}/config/smr.example.yaml" "${CONFDIR}/smr.yaml"
  echo "    Created ${CONFDIR}/smr.yaml"
fi

LAUNCHER="${BINDIR}/securemodelroute"
cat > "${LAUNCHER}" << EOF
#!/usr/bin/env bash
exec "${BINDIR}/smr" --config "${CONFDIR}/smr.yaml" --open "\$@"
EOF
chmod +x "${LAUNCHER}"

if [[ "$INSTALL_SERVICE" == true && "$(uname -s)" == "Darwin" ]]; then
  PLIST="${HOME}/Library/LaunchAgents/com.securemodelroute.smr.plist"
  mkdir -p "${HOME}/Library/LaunchAgents"
  cat > "${PLIST}" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.securemodelroute.smr</string>
  <key>ProgramArguments</key>
  <array>
    <string>${BINDIR}/smr</string>
    <string>--config</string>
    <string>${CONFDIR}/smr.yaml</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${CONFDIR}/smr.log</string>
  <key>StandardErrorPath</key><string>${CONFDIR}/smr.err.log</string>
</dict>
</plist>
EOF
  launchctl unload "${PLIST}" 2>/dev/null || true
  launchctl load "${PLIST}"
  echo "    LaunchAgent installed: ${PLIST}"
fi

echo ""
echo "Installed:"
echo "  binary:   ${BINDIR}/smr"
echo "  launcher: ${LAUNCHER}"
echo "  config:   ${CONFDIR}/smr.yaml"
echo "  GUI:      http://127.0.0.1:8080/ui"
echo ""
echo "Run:  securemodelroute"
echo "Or:   smr --config ${CONFDIR}/smr.yaml --open"
echo ""
echo "Background service (macOS): ./scripts/install.sh --service"
if [[ ":${PATH}:" != *":${BINDIR}:"* ]]; then
  echo "Add to PATH:  export PATH=\"${BINDIR}:\$PATH\""
fi
