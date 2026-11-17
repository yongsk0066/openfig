#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/test-input-method-enrollment.py
scratch="$(mktemp -d "${TMPDIR:-/tmp}/fig-ime-check.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
# No registration, keyboard changes or terminal interaction in the ordinary gate.
bash scripts/build-input-method.sh "$scratch/FigInputMethod.app"
xcrun clang -fobjc-arc -Wall -Wextra -Werror -O2 \
  native/input-method/tests/geometry.m -framework Cocoa -framework Carbon -framework InputMethodKit \
  -o "$scratch/geometry"
"$scratch/geometry"
xcrun clang -fobjc-arc -Wall -Wextra -Werror -O2 \
  native/input-method/tests/lifecycle.m -framework Cocoa -framework Carbon -framework InputMethodKit \
  -o "$scratch/lifecycle"
"$scratch/lifecycle"
