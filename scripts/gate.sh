#!/usr/bin/env bash
# Complete local validation, also used by the optional pre-push hook.
# Run with CI=1 to check non-interactive behavior; stop at the first failure.

set -euo pipefail

native_ui=1
case "${1:-}" in
  "") ;;
  --headless) native_ui=0 ;;
  *) echo "usage: $0 [--headless]" >&2; exit 2 ;;
esac
if [ "$#" -gt 1 ]; then echo "usage: $0 [--headless]" >&2; exit 2; fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Disable build-tool telemetry and use output suitable for a non-interactive pre-push hook.
export TURBO_TELEMETRY_DISABLED=1
export TURBO_UI=stream
export DO_NOT_TRACK=1

scratch="$(mktemp -d "${TMPDIR:-/tmp}/gate.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

summary=()

step() {
  local name="$1"
  shift
  printf '\n\033[1m==> %s\033[0m\n' "$name"
  local start=$SECONDS
  local status=0
  "$@" </dev/null || status=$?
  local took=$((SECONDS - start))
  if [ "$status" -eq 0 ]; then
    summary+=("$(printf 'ok    %4ss  %s' "$took" "$name")")
  else
    summary+=("$(printf 'FAIL  %4ss  %s (exit %s)' "$took" "$name" "$status")")
    report
    exit "$status"
  fi
}

report() {
  printf '\n\033[1mgate summary\033[0m (CI=%s)\n' "${CI:-}"
  printf '  %s\n' "${summary[@]}"
}

# Content hash of every file git would show: tracked (present) and untracked-but-not-ignored, plus
# a marker for tracked files that are gone. Compared before and after codegen, so an already dirty
# work tree is fine as long as codegen does not change it further.
tree_snapshot() {
  local out="$1"
  git ls-files -z --cached --others --exclude-standard | tr '\0' '\n' | LC_ALL=C sort -u >"$out.names"
  : >"$out.present"
  : >"$out.missing"
  while IFS= read -r path; do
    if [ -f "$path" ]; then
      printf '%s\n' "$path" >>"$out.present"
    else
      printf 'missing %s\n' "$path" >>"$out.missing"
    fi
  done <"$out.names"
  { git hash-object --stdin-paths <"$out.present" | paste -d ' ' - "$out.present"; cat "$out.missing"; } >"$out"
}

codegen_zero_diff() {
  tree_snapshot "$scratch/before"
  pnpm exec turbo codegen
  tree_snapshot "$scratch/after"
  if ! diff -u "$scratch/before" "$scratch/after" >"$scratch/codegen.diff"; then
    echo "codegen changed the tree; commit the regenerated files:"
    grep '^[-+][^-+]' "$scratch/codegen.diff" | cut -d ' ' -f 2- | LC_ALL=C sort -u | sed 's/^/  /'
    return 1
  fi
}

step "pnpm install (frozen lockfile)" pnpm install --frozen-lockfile --prefer-offline
step "locked Rust sources for license checks" cargo fetch --locked --target aarch64-apple-darwin
step "rustfmt --check" cargo +nightly fmt --check
step "oxfmt --check" pnpm exec oxfmt --check
step "oxlint --type-aware (zero warnings)" pnpm exec oxlint --type-aware --max-warnings 0
step "core architecture boundaries" node scripts/check-architecture.mjs
step "documentation links" node scripts/check-docs.mjs
step "third-party notices" python3 scripts/license-notices.py --check
step "publication hygiene" python3 scripts/check-publication.py
step "public/private bundle identity" python3 scripts/test-branding.py
step "turbo boundaries" pnpm exec turbo boundaries
step "turbo codegen zero-diff" codegen_zero_diff
step "clippy --all-targets -D warnings" cargo clippy --locked --workspace --all-targets -- -D warnings
step "turbo typecheck" pnpm exec turbo typecheck
step "vitest run" pnpm exec vitest run
step "nextest run --workspace" cargo nextest run --locked --workspace
step "FFI safety doctests" cargo test --locked -p macos-utils --doc
step "app installation rollback" python3 scripts/test-app-install.py
step "input method compile and geometry" bash scripts/check-input-method.sh
if [ "$native_ui" -eq 1 ]; then
  step "WKWebView runtime resize smoke" cargo run --locked -p fig_desktop --example webview_runtime_smoke
else
  summary+=("SKIP         WKWebView runtime resize smoke (--headless; run just gate in a GUI session)")
fi

report
