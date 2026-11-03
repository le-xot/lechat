#!/usr/bin/env bash
set -euo pipefail

REPO="le-xot/lechat"
ARCH="$(uname -m)"
if [ "$ARCH" != "x86_64" ]; then
    echo "lechat: only x86_64 Linux builds are published (got $ARCH)" >&2
    exit 1
fi

BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}"

url="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -oE 'https://[^"]*x86_64-unknown-linux-gnu\.tar\.gz' | head -1)"
if [ -z "$url" ]; then
    echo "lechat: no linux asset in the latest release" >&2
    exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/lechat.tar.gz"
tar -xzf "$tmp/lechat.tar.gz" -C "$tmp"

install -Dm755 "$tmp/lechat" "$BIN_DIR/lechat"
for icon in "$tmp"/icons/*x*/apps/lechat.png; do
    size="$(basename "$(dirname "$(dirname "$icon")")")"
    install -Dm644 "$icon" "$DATA_DIR/icons/hicolor/${size}/apps/lechat.png"
done
mkdir -p "$DATA_DIR/applications"
sed "s|^Exec=.*|Exec=${BIN_DIR}/lechat|" "$tmp/lechat.desktop" \
    > "$DATA_DIR/applications/lechat.desktop"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "installed $(command -v lechat || echo "$BIN_DIR/lechat")"
echo "auto-update is enabled for user installs; run lechat and it will keep itself current"
