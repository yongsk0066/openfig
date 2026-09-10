#!/usr/bin/env bash
# Regenerates every app and tray icon in the repo from bundle/macos/icon-gen/icon.swift.
#
#   ./bundle/macos/icon-gen/generate.sh
#
# Overwrites, in place:
#   crates/fig_desktop/icons/{icon.png,128x128.png,128x128@2x.png,32x32.png,icon.icns}
#   crates/fig_desktop/icons/icon-monochrome.png
#   bundle/macos/AppIcon.icns
#
# The artwork is a placeholder (black disc + white "f"); the real icon is a
# separate track.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

swift "$here/icon.swift" "$work/png"

icons="$root/crates/fig_desktop/icons"
cp "$work/png/app-512.png" "$icons/icon.png"
cp "$work/png/app-128.png" "$icons/128x128.png"
cp "$work/png/app-256.png" "$icons/128x128@2x.png"
cp "$work/png/app-32.png" "$icons/32x32.png"
# Tray images are template images (tray_icon is built with icon_as_template),
# so macOS recolours the black glyph for the current menu-bar appearance.
cp "$work/png/tray-dark-36.png" "$icons/icon-monochrome.png"
# Only regenerate the light variant if this checkout still carries one.
if [ -f "$icons/icon-monochrome-light.png" ]; then
  cp "$work/png/tray-light-36.png" "$icons/icon-monochrome-light.png"
fi

set="$work/AppIcon.iconset"
mkdir -p "$set"
cp "$work/png/app-16.png" "$set/icon_16x16.png"
cp "$work/png/app-32.png" "$set/icon_16x16@2x.png"
cp "$work/png/app-32.png" "$set/icon_32x32.png"
cp "$work/png/app-64.png" "$set/icon_32x32@2x.png"
cp "$work/png/app-128.png" "$set/icon_128x128.png"
cp "$work/png/app-256.png" "$set/icon_128x128@2x.png"
cp "$work/png/app-256.png" "$set/icon_256x256.png"
cp "$work/png/app-512.png" "$set/icon_256x256@2x.png"
cp "$work/png/app-512.png" "$set/icon_512x512.png"
cp "$work/png/app-1024.png" "$set/icon_512x512@2x.png"
iconutil -c icns "$set" -o "$work/AppIcon.icns"

cp "$work/AppIcon.icns" "$root/bundle/macos/AppIcon.icns"
cp "$work/AppIcon.icns" "$icons/icon.icns"

echo "regenerated icons under $icons and $root/bundle/macos"
