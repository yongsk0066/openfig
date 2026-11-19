#!/bin/bash
# Build a public arm64 ZIP from the committed source, never from the installed or local bundle.
set -euo pipefail
root="$(git rev-parse --show-toplevel)"
commit="$(git rev-parse --verify HEAD^{commit})"
identity="${FIG_SIGN_IDENTITY:-}"
profile="${FIG_NOTARY_PROFILE:-}"
if [[ "$identity" != "Developer ID Application: "* || -z "$profile" ]]; then
  echo 'Set FIG_SIGN_IDENTITY to the full Developer ID Application certificate name and FIG_NOTARY_PROFILE to a notarytool keychain profile.' >&2
  exit 2
fi
if ! security find-identity -v -p codesigning | awk -F'"' -v name="$identity" '$2 == name {found=1} END {exit !found}'; then
  echo 'Developer ID Application identity with private key was not found.' >&2
  exit 2
fi
dest="$root/build/releases/${commit:0:12}"
[[ ! -e "$dest" ]] || { echo "Release output already exists: $dest" >&2; exit 2; }
scratch="$(mktemp -d "${TMPDIR:-/tmp}/openfig-release.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
git clone --quiet --no-local --no-checkout "$root" "$scratch/source"
cd "$scratch/source"
git checkout --quiet --detach "$commit"
test ! -e bundle-id.local && test ! -e themes.local || exit 2
python3 scripts/check-publication.py
unset CARGO_TARGET_DIR
export FIG_SIGN_IDENTITY="$identity" FIG_DISTRIBUTION_SIGNING=1
just all
source branding.env
app="build/$APP_NAME.app"
test "$(plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist")" = "$BUNDLE_ID"
test -z "$(find "$app/Contents/Resources/themes" -type f -print -quit)"
codesign --verify --deep --strict -R='anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists' "$app"
/usr/bin/ditto -c -k --keepParent "$app" "$scratch/submission.zip"
xcrun notarytool submit "$scratch/submission.zip" --wait --keychain-profile "$profile" --output-format json > "$scratch/notary.json"
jq -e '.status == "Accepted"' "$scratch/notary.json" >/dev/null || { cat "$scratch/notary.json" >&2; exit 1; }
xcrun stapler staple "$app"
mkdir "$scratch/output"
zip="$APP_NAME-macOS-arm64.zip"
/usr/bin/ditto -c -k --keepParent "$app" "$scratch/output/$zip"
/usr/bin/ditto -x -k "$scratch/output/$zip" "$scratch/unpacked"
final="$scratch/unpacked/$APP_NAME.app"
codesign --verify --deep --strict "$final"
xcrun stapler validate "$final"
spctl --assess --type execute --verbose=2 "$final"
(cd "$scratch/output" && shasum -a 256 "$zip" > SHA256SUMS)
mkdir -p "$(dirname "$dest")"
# Publish the complete pair locally only after the delivered archive passed assessment.
mkdir "$dest"
if ! cp "$scratch/output/$zip" "$scratch/output/SHA256SUMS" "$dest/"; then rm -rf "$dest"; exit 1; fi
echo "Verified release for $commit: $dest"
