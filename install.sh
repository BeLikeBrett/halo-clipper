#!/usr/bin/env bash
# Install Halo Clipper as a user-local Linux desktop app.
#
# What this does:
#   - Builds the release binary if it's not already present.
#   - Copies the binary to ~/.local/bin/halo-clipper
#   - Copies icons to ~/.local/share/icons/hicolor/<size>/apps/
#   - Copies the .desktop file to ~/.local/share/applications/
#   - Removes any leftover install under the app's old name (brett-clipper)
#   - Refreshes the desktop + icon caches so the menu picks it up.
#
# No sudo, no system-wide writes. Re-run safely to upgrade.
#
# Uninstall: ./install.sh uninstall

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_NAME="halo-clipper"
APP_ID="halo-clipper"
LEGACY_ID="brett-clipper"

BIN_DIR="${HOME}/.local/bin"
ICON_BASE="${HOME}/.local/share/icons/hicolor"
APP_DIR="${HOME}/.local/share/applications"

remove_app() {
    local id="$1"
    rm -f "${BIN_DIR}/${id}"
    rm -f "${APP_DIR}/${id}.desktop"
    rm -f "${ICON_BASE}/scalable/apps/${id}.svg"
    for size in 48 64 128 256 512; do
        rm -f "${ICON_BASE}/${size}x${size}/apps/${id}.png"
    done
}

case "${1:-install}" in
    uninstall)
        echo "Removing ${APP_ID}..."
        remove_app "${APP_ID}"
        remove_app "${LEGACY_ID}"
        update-desktop-database "${APP_DIR}" 2>/dev/null || true
        gtk-update-icon-cache "${ICON_BASE}" 2>/dev/null || true
        echo "Uninstalled."
        exit 0
        ;;
    install) ;;
    *)
        echo "usage: $0 [install|uninstall]"
        exit 1
        ;;
esac

# Build the binary if missing or stale.
BINARY="${REPO_DIR}/target/release/${BIN_NAME}"
echo "Building current release binary..."
(cd "${REPO_DIR}" && cargo build --release --locked)

# Clean up any install left behind by the app's previous name so two
# entries don't show up in the launcher.
remove_app "${LEGACY_ID}"

# Ensure target dirs exist.
mkdir -p "${BIN_DIR}"
mkdir -p "${APP_DIR}"
mkdir -p "${ICON_BASE}/scalable/apps"
for size in 48 64 128 256 512; do
    mkdir -p "${ICON_BASE}/${size}x${size}/apps"
done

# Binary.
install -m 755 "${BINARY}" "${BIN_DIR}/${BIN_NAME}"
echo "  → ${BIN_DIR}/${BIN_NAME}"

# Icons (vector + every rasterized size, named after the .desktop Icon= key).
install -m 644 "${REPO_DIR}/assets/icon.svg" \
    "${ICON_BASE}/scalable/apps/${APP_ID}.svg"
echo "  → ${ICON_BASE}/scalable/apps/${APP_ID}.svg"
for size in 48 64 128 256 512; do
    install -m 644 "${REPO_DIR}/assets/icon-${size}.png" \
        "${ICON_BASE}/${size}x${size}/apps/${APP_ID}.png"
done
echo "  → ${ICON_BASE}/{48,64,128,256,512}x.../apps/${APP_ID}.png"

# Desktop entry. We substitute the absolute binary path into Exec=
# and TryExec= so launchers (fuzzel/wofi/etc.) work regardless of the
# compositor's PATH — Niri and friends don't source .bashrc, so a bare
# `Exec=halo-clipper` is invisible to them when ~/.local/bin isn't in
# the session env. Hardcoding the absolute path keeps the entry usable
# from any launcher context.
ABSOLUTE_BIN="${BIN_DIR}/${BIN_NAME}"
sed -e "s|^Exec=${BIN_NAME}|Exec=${ABSOLUTE_BIN}|" \
    -e "s|^TryExec=${BIN_NAME}|TryExec=${ABSOLUTE_BIN}|" \
    "${REPO_DIR}/assets/${APP_ID}.desktop" \
    > "${APP_DIR}/${APP_ID}.desktop"
chmod 644 "${APP_DIR}/${APP_ID}.desktop"
echo "  → ${APP_DIR}/${APP_ID}.desktop (Exec=${ABSOLUTE_BIN})"

# Refresh caches (silent no-op if tools aren't present).
update-desktop-database "${APP_DIR}" 2>/dev/null || true
gtk-update-icon-cache "${ICON_BASE}" 2>/dev/null || true

cat <<EOF

Installed. Three things to check:

  1. Your application launcher (Wofi/Rofi/etc) should list "Halo Clipper"
     after a brief refresh — log out + back in if it doesn't show up immediately.

  2. ~/.local/bin needs to be on PATH for the .desktop's Exec= to find it. Most
     distros set this automatically; if running 'halo-clipper' in a shell fails:
         export PATH="\$HOME/.local/bin:\$PATH"
     (add to ~/.bashrc / ~/.zshrc).

  3. Right-clicking a .mp4 / .mkv / .mov in your file manager should now offer
     "Halo Clipper" in "Open With" — and it opens the file directly.

Uninstall any time with: ${REPO_DIR}/install.sh uninstall
EOF
