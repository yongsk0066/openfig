#!/usr/bin/env bash
# The local merge gate. There is no remote and no CI (docs/15-architecture-study.md §2.4), so this
# is the only thing standing between a commit and `main`: `just gate` runs it, and the pre-push
# hook (`just hooks`) runs `just gate`.
#
# Cheap static checks first so a formatting slip fails in seconds, the test suites last. Stops at
# the first red step and prints how long each step took.
#
# It must pass with `CI=1` exported as well: nothing here may depend on the caller's environment.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# turbo phones home unless told not to; this fork is offline by policy. The TUI needs a terminal a
# pre-push hook does not have.
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
step "rustfmt --check" cargo +nightly fmt --check
step "oxfmt --check" pnpm exec oxfmt --check
step "oxlint --type-aware (zero warnings)" pnpm exec oxlint --type-aware --max-warnings 0
step "core architecture boundaries" node scripts/check-architecture.mjs
step "turbo boundaries" pnpm exec turbo boundaries
step "turbo codegen zero-diff" codegen_zero_diff
step "clippy --all-targets -D warnings" cargo clippy --locked --workspace --all-targets -- -D warnings
step "turbo typecheck" pnpm exec turbo typecheck
step "vitest run" pnpm exec vitest run
step "nextest run --workspace" cargo nextest run --locked --workspace
step "WKWebView runtime resize smoke" cargo run --locked -p fig_desktop --example webview_runtime_smoke

report
