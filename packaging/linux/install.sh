#!/bin/sh
# Installs gitgoo for the current user: the binary in ~/.local/bin, plus a launcher and its icon for the menu.
set -e
cd "$(dirname "$0")"
bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 gitgoo "$bin/gitgoo"
install -Dm644 gitgoo.svg "$share/icons/hicolor/scalable/apps/gitgoo.svg"
install -Dm644 gitgoo.png "$share/icons/hicolor/256x256/apps/gitgoo.png"
mkdir -p "$share/applications"
# The full path, as a graphical session's PATH often lacks ~/.local/bin.
sed "s|^Exec=gitgoo|Exec=$bin/gitgoo|" gitgoo.desktop > "$share/applications/gitgoo.desktop"
command -v update-desktop-database > /dev/null && update-desktop-database "$share/applications" || true
echo "Installed $bin/gitgoo; it's in the application menu as gitgoo."
