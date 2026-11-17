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
# Read the same identity override as Justfile and the Rust build script.
python3 - "$output/Contents/Info.plist" <<'PYTHON'
from pathlib import Path
import plistlib, re, shlex, sys
local = Path('bundle-id.local')
if local.exists():
    identity = local.read_text().strip()
else:
    value = next(match.group(1) for line in Path('branding.env').read_text().splitlines()
                 if (match := re.fullmatch(r'(?:export\s+)?BUNDLE_ID\s*=\s*(.*)', line.strip())))
    fields = shlex.split(value, comments=True)
    if len(fields) != 1:
        raise SystemExit('Invalid bundle identity')
    identity = fields[0]
if not re.fullmatch(r'[A-Za-z0-9.-]+', identity):
    raise SystemExit('Invalid bundle identity')
path = Path(sys.argv[1])
info = plistlib.loads(path.read_bytes())
info['CFBundleIdentifier'] = info['TISInputSourceID'] = identity + '.inputmethod'
info['InputMethodConnectionName'] = identity + '.inputmethod.connection'
path.write_bytes(plistlib.dumps(info, sort_keys=False))
PYTHON
plutil -lint "$output/Contents/Info.plist"
