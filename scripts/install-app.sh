#!/usr/bin/env bash
set -euo pipefail

verify_app() {
  local app="$1" cli="$2" desktop="$3"
  codesign --verify --deep --strict "$app"
  test -x "$app/Contents/MacOS/$desktop"
  test -x "$app/Contents/MacOS/fig_native_worker"
  test -x "$app/Contents/MacOS/$cli"
  test -x "$app/Contents/Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method"
}

# Prepare a new, unique bundle without touching running code, input clients or registration.
stage_app() (
  set -euo pipefail
  local source="$1" destination="$2" cli="$3" desktop="$4" stage
  [[ -d "$source" && ! -e "$destination" && ! -L "$destination" ]] || exit 2
  mkdir -p "$(dirname "$destination")"
  stage="$(mktemp -d "$(dirname "$destination")/.fig-stage.XXXXXX")"
  trap 'rm -rf "$stage"' EXIT
  ditto "$source" "$stage/new.app"
  verify_app "$stage/new.app" "$cli" "$desktop"
  mv "$stage/new.app" "$destination"
  echo "Prepared update (not activated): $destination"
)

same_input_method() {
  local previous_hash next_hash
  previous_hash="$(codesign -d --verbose=4 "$1" 2>&1 | awk -F= '/^CDHash=/{print $2}')" || true
  next_hash="$(codesign -d --verbose=4 "$2" 2>&1 | awk -F= '/^CDHash=/{print $2}')" || true
  # Signing timestamps/CMS bytes can change without changing sealed code or bundle metadata.
  if [[ -n "$previous_hash" && -n "$next_hash" ]]; then
    [[ "$previous_hash" == "$next_hash" ]]
  else
    cmp -s "$1" "$2"
  fi
}

# Stage and verify before stopping the app. Preserve the previous signed bundle;
# a copy/rename/launch failure restores it. Enrollment is a separate transaction.
replace_app() (
  set -euo pipefail
  local source="$1" destination="$2" cli="$3" desktop="$4" stage backup backup_dir moved=0 installed=0 helper_changed=0
  [[ -d "$source" && ! -L "$destination" ]] || { echo 'Invalid bundle source or symlink destination' >&2; exit 2; }
  stage="$(mktemp -d "$(dirname "$destination")/.fig-install.XXXXXX")"
  backup_dir="$(dirname "$destination")/.fig-backups"
  mkdir -p "$backup_dir"
  backup="$backup_dir/$(basename "${destination%.app}")-$(date +%Y%m%d-%H%M%S)-$$.app"
  cleanup() {
    local result=$?
    trap - EXIT
    if (( result != 0 && moved )); then
      if (( installed )); then mv "$destination" "$stage/failed.app"; fi
      mv "$backup" "$destination"
      open -g "$destination" --args --no-dashboard || true
      echo "Restored previous app: $destination" >&2
    fi
    rm -rf "$stage"
    exit "$result"
  }
  trap cleanup EXIT
  ditto "$source" "$stage/new.app"
  verify_app "$stage/new.app" "$cli" "$desktop"
  if [[ -d "$destination" ]]; then
    if ! same_input_method "$destination/Contents/Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method" \
        "$stage/new.app/Contents/Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method"; then
      helper_changed=1
    fi
    "$destination/Contents/MacOS/$cli" quit || true
    for (( attempt=0; attempt<50; attempt++ )); do
      if ! pgrep -x "$desktop" >/dev/null; then break; fi
      sleep 0.1
    done
    if pgrep -x "$desktop" >/dev/null; then
      echo 'Desktop did not exit; original app has been preserved' >&2
      exit 1
    fi
    mv "$destination" "$backup"
    moved=1
  fi
  mv "$stage/new.app" "$destination"
  installed=1
  open -g "$destination" --args --no-dashboard
  if (( helper_changed )); then
    if "$destination/Contents/Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method" --stop-running; then
      if open -g "$destination/Contents/Helpers/FigInputMethod.app"; then
        echo 'Input method replaced; approved terminal clients reconnect automatically.'
      else
        echo 'Input method launch pending; rerun enrollment' >&2
      fi
    else
      echo 'Input method replacement is pending; the desktop remains installed and running' >&2
    fi
  fi
  if (( moved )); then echo "Previous app retained: $backup"; fi
)

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  if [[ "${1:-}" == --stage-only ]]; then
    shift
    stage_app "$@"
  else
    replace_app "$@"
  fi
fi
