#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
output="${1:-build/FigInputMethod.app}"
mkdir -p "$output/Contents/MacOS" "$output/Contents/Resources"
cp bundle/macos/AppIcon.icns "$output/Contents/Resources/AppIcon.icns"
xcrun clang -arch arm64 -mmacosx-version-min=13.0 -fobjc-arc -Wall -Wextra -Werror -O2 \
  native/input-method/main.m -framework Cocoa -framework Carbon -framework InputMethodKit \
  -o "$output/Contents/MacOS/fig_input_method"
cp native/input-method/Info.plist "$output/Contents/Info.plist"
plutil -lint "$output/Contents/Info.plist"
