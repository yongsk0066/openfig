#!/usr/bin/env bash
set -euo pipefail

is_installed_input_method_path() {
  case "$1" in
    /Applications/*|"$HOME/Applications/"*|"/Library/Input Methods/"*|"$HOME/Library/Input Methods/"*) return 0 ;;
    *) return 1 ;;
  esac
}

await_registered_input_method() {
  local helper="$1"
  for (( attempt=0; attempt<20; attempt++ )); do
    if "$helper" --status >/dev/null 2>&1; then return 0; fi
    sleep 0.25
  done
  echo "Registration did not become visible to a fresh process within 5 seconds" >&2
  return 3
}

activate_input_method() {
  local helper="$1" target="$2" timeout="$3" requester="${4:-}" deadline
  # IMK must be running before a terminal initializes its input context.
  open -g "$target" || return
  if "$helper" --check-active >/dev/null; then return 0; fi
  if ! "$helper" --check-enabled >/dev/null; then
    if [[ -z "$requester" || ! -x "$requester" ]]; then
      echo "Activation requires the installed desktop CLI; start Fig before enrollment" >&2
      return 2
    fi
    # TIS requests originate in the GUI desktop main loop. A headless helper
    # request is not equivalent to the GUI enrollment path verified on macOS.
    "$requester" debug enable-input-method || return
    deadline=$((SECONDS + timeout))
    # Activation can open an asynchronous approval UI. Inspect from a new
    # process each time: the requesting process can retain stale TIS state.
    until "$helper" --check-enabled >/dev/null; do
      if (( SECONDS >= deadline )); then
        "$helper" --status >&2 || true
        echo "Activation was requested but not confirmed within ${timeout}s; the input method remains unavailable." >&2
        echo "This does not establish an OS prohibition. Inspect the system activation result before retrying." >&2
        return 3
      fi
      sleep 1
    done
  fi
  if ! "$helper" --select || ! "$helper" --check-active; then
    echo "Input method is enabled but selection was not confirmed." >&2
    return 3
  fi
}

install_input_method() {
  local source_bundle mode helper target timeout requester
  source_bundle="$(cd "${1:?Pass the installed FigInputMethod.app path}" && pwd -P)"
  # Keyboard Settings cannot follow an Input Methods symlink into a development
  # directory, even when our own process can enumerate the registered source.
  if ! is_installed_input_method_path "$source_bundle"; then
    echo "Install Fig into Applications before registering its input method: $source_bundle" >&2
    return 2
  fi
  mode="${2:---activate}"
  timeout="${3:-120}"
  case "$mode" in
    --activate|--register-only) ;;
    *) echo "Expected --activate or --register-only" >&2; return 2 ;;
  esac
  if [[ ! "$timeout" =~ ^[0-9]{1,3}$ ]] || (( 10#$timeout > 120 )); then
    echo "Activation timeout must be an integer from 0 to 120 seconds" >&2
    return 2
  fi
  timeout=$((10#$timeout))
  helper="$source_bundle/Contents/MacOS/fig_input_method"
  [[ -x "$helper" ]] || { echo "Missing input method executable: $helper" >&2; return 1; }
  target="$HOME/Library/Input Methods/FigInputMethod.app"
  mkdir -p "$(dirname "$target")"
  if [[ -e "$target" && ! -L "$target" ]]; then
    echo "Refusing to replace a non-symlink input method: $target" >&2
    return 1
  fi
  # Preserve a correct registration path across repeated installs.
  if [[ ! -L "$target" ]] || [[ "$(readlink "$target")" != "$source_bundle" ]]; then
    ln -sfn "$source_bundle" "$target"
  fi
  "$helper" --register "$target" || return
  await_registered_input_method "$helper" || return
  if [[ "$mode" == --register-only ]]; then
    "$helper" --status
    return
  fi
  requester="$(dirname "$(dirname "$source_bundle")")/MacOS/${CLI_BIN:-q}"
  activate_input_method "$helper" "$target" "$timeout" "$requester"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  install_input_method "$@"
fi
