#!/usr/bin/env bash
# Registers Jifar's icon for development builds, so the GNOME dock / Alt-Tab
# show the logo while `pnpm tauri dev` runs.
#
# GNOME on Wayland takes a window's icon from the .desktop file whose name
# matches the window's app ID. Tauri uses the binary name (`jifar`), and a
# dev build has no installed .desktop file, so the dock shows a generic icon.
# The .deb/AppImage install their own entry; this is for dev only.
#
# Usage: scripts/dev-desktop-entry.sh [--remove]
# Writes only under ~/.local/share (your user), nothing system-wide.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
desktop=$data/applications/jifar.desktop
icons=$data/icons/hicolor

if [[ ${1:-} == --remove ]]; then
  rm -f "$desktop"
  for s in 32 64 128 256 512; do rm -f "$icons/${s}x${s}/apps/jifar.png"; done
  echo "removed the dev desktop entry and icons"
  exit 0
fi

declare -A src=(
  [32]=32x32.png [64]=64x64.png [128]=128x128.png [256]=128x128@2x.png [512]=icon.png
)
for s in "${!src[@]}"; do
  install -Dm644 "$repo/src-tauri/icons/${src[$s]}" "$icons/${s}x${s}/apps/jifar.png"
done

install -d "$(dirname "$desktop")"
cat > "$desktop" <<ENTRY
[Desktop Entry]
Type=Application
Name=Jifar (dev)
Comment=Meeting recorder (development build)
Exec=$repo/target/debug/jifar
Icon=jifar
StartupWMClass=jifar
Categories=AudioVideo;Office;
NoDisplay=true
ENTRY

command -v update-desktop-database >/dev/null && update-desktop-database -q "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
echo "installed $desktop (hidden from the app grid; used to match the dev window)"
