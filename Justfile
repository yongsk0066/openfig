set shell := ["bash", "-euo", "pipefail", "-c"]
set dotenv-load := true
set dotenv-filename := "branding.env"
# branding.env is the single source of truth, so it wins over the ambient
# environment. crates/fig_util/build.rs reads the file directly and cannot see
# an exported APP_NAME; without this an `export APP_NAME=...` in the caller's
# shell would rename the bundle while the Rust constants kept the file's value.
set dotenv-override := true

target     := "aarch64-apple-darwin"
tdir       := "target" / target / "release"
build      := justfile_directory() / "build"
app        := build / env("APP_NAME") + ".app"
export BUNDLE_ID := if path_exists("bundle-id.local") == "true" { trim(read("bundle-id.local")) } else { env("BUNDLE_ID") }
version    := `cargo metadata --format-version 1 --no-deps --offline | jq -r '.packages[] | select(.name=="fig_desktop") | .version'`
min_macos  := "13.0"

default: all

all: deps specs web rust bundle sign

# rust-toolchain.toml pins the channel, so rustup installs it on demand.
# Toolchain check
deps:
    just --version
    rustup show active-toolchain
    rustup target list --installed | grep -Fxq "{{target}}" || rustup target add "{{target}}"
    node_version="$(node --version | tr -d 'v')"; \
        [ "$(printf '26.0.0\n%s\n' "$node_version" | sort -V | head -1)" = "26.0.0" ] \
        || { echo "node >= 26 required, found $node_version (fnm use)"; exit 1; }
    pin="$(node -p 'require("./package.json").packageManager')"; \
        [ "pnpm@$(pnpm --version)" = "${pin%%+*}" ] \
        || { echo "${pin%%+*} required, found pnpm@$(pnpm --version)"; exit 1; }
    protoc_version="$(protoc --version | awk '{print $2}')"; \
        [ "$(printf '3.12\n%s\n' "$protoc_version" | sort -V | head -1)" = "3.12" ] \
        || { echo "protoc >= 3.12 required, found $protoc_version (brew install protobuf)"; exit 1; }
    xcrun --sdk macosx --show-sdk-path
    command -v codesign plutil jq rsync perl

# Completion specs: specs/src/**/*.ts -> specs/dist (index.json + <name>.js)
specs:
    pnpm install --frozen-lockfile --ignore-scripts
    pnpm exec turbo build --filter=@fig/specs
    test -f specs/dist/index.json

# The filters keep turbo away from the packages the bundle does not need. They are
# paths, not package names, so package renames cannot silently empty them.
# Web: proto -> api-bindings -> autocomplete-app, dashboard-app
web:
    pnpm install --frozen-lockfile --ignore-scripts
    cargo fetch --locked --target {{target}}
    rm -rf packages/autocomplete-app/dist packages/dashboard-app/dist
    python3 scripts/license-notices.py --check
    pnpm exec turbo build --force --filter=./packages/autocomplete-app --filter=./packages/dashboard-app
    test -f packages/autocomplete-app/dist/index.html
    test -f packages/dashboard-app/dist/index.html

# Only the binaries that go into the bundle.
rust:
    cargo build --locked --release --target {{target}} -p fig_desktop -p fig_cli

# Assemble build/<APP_NAME>.app
bundle:
    rm -rf "{{app}}"
    mkdir -p "{{app}}/Contents/MacOS" "{{app}}/Contents/Resources"
    cp "{{tdir}}/fig_io_worker" "{{app}}/Contents/MacOS/fig_io_worker"
    cp "{{tdir}}/fig_native_worker" "{{app}}/Contents/MacOS/fig_native_worker"
    cp "{{tdir}}/fig_desktop" "{{app}}/Contents/MacOS/$DESKTOP_BIN"
    cp "{{tdir}}/fig_cli"     "{{app}}/Contents/MacOS/$CLI_BIN"
    VERSION='{{version}}' MIN_MACOS='{{min_macos}}' \
        perl -pe 's{\@(\w+)\@}{ my $v = $ENV{$1}; die "no value for \@$1\@\n" unless defined $v; $v =~ s/&/&amp;/g; $v =~ s/</&lt;/g; $v =~ s/>/&gt;/g; $v }ge' \
        bundle/macos/Info.plist.in > "{{app}}/Contents/Info.plist"
    plutil -lint "{{app}}/Contents/Info.plist"
    printf 'APPL????' > "{{app}}/Contents/PkgInfo"
    cp bundle/macos/AppIcon.icns "{{app}}/Contents/Resources/AppIcon.icns"
    cp -R packages/autocomplete-app/dist/ "{{app}}/Contents/Resources/autocomplete/"
    cp -R packages/dashboard-app/dist/    "{{app}}/Contents/Resources/dashboard/"
    mkdir -p "{{app}}/Contents/Resources/themes"
    for theme in resources/themes/*.json themes.local/*.json; do [ ! -f "$theme" ] || cp "$theme" "{{app}}/Contents/Resources/themes/"; done
    mkdir -p "{{app}}/Contents/Resources/licenses"
    cp LICENSE.MIT LICENSE.APACHE NOTICE "{{app}}/Contents/Resources/licenses/"
    cp -R specs/dist/                     "{{app}}/Contents/Resources/specs/"
    bash scripts/build-input-method.sh "{{app}}/Contents/Helpers/FigInputMethod.app"

# Inner executables first and the bundle last, because the top level seals what is under it.
# Signing identity: any "Apple Development" / "Developer ID Application" certificate in the
# login keychain keeps the app's designated requirement stable across rebuilds, so the
# Accessibility (TCC) grant survives re-signing. Override with FIG_SIGN_IDENTITY. Falls back
# to ad-hoc ("-", cdhash only) when no identity exists — then every re-sign needs
# `q debug accessibility refresh` and a new toggle in System Settings.
sign:
    identity="${FIG_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
        | awk -F'"' '/Developer ID Application|Apple Development|Mac Developer/ {print $2; exit}')}"; \
    identity="${identity:--}"; \
    echo "codesign identity: $identity"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID.inputmethod" "{{app}}/Contents/Helpers/FigInputMethod.app"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID.ioworker" "{{app}}/Contents/MacOS/fig_io_worker"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID.nativeworker" "{{app}}/Contents/MacOS/fig_native_worker"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID.cli" "{{app}}/Contents/MacOS/$CLI_BIN"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID"     "{{app}}"
    codesign --verify --deep --strict --verbose=2 "{{app}}"

# Prepare a signed update without stopping the running app or changing enrollment.
stage:
    bash scripts/install-app.sh --stage-only "{{app}}" "/Applications/.fig-updates/$APP_NAME-$(date +%Y%m%d-%H%M%S).app" "$CLI_BIN" "$DESKTOP_BIN"

# Install at the path used by app_bundle_path(), register with LaunchServices, and set up zsh.
# Preserve regular files at the CLI link target. See docs/development.md for signing/rollback.
install:
    bash scripts/install-app.sh "{{app}}" "/Applications/$APP_NAME.app" "$CLI_BIN" "$DESKTOP_BIN"
    mkdir -p "$HOME/.local/bin"
    link="$HOME/.local/bin/$CLI_BIN"; \
    if [ -f "$link" ] && [ ! -L "$link" ]; then \
        backup="$link.bak-$(date +%Y%m%d%H%M%S)"; \
        echo "moving existing $link aside to $backup"; \
        mv -n "$link" "$backup"; \
    fi; \
    ln -sf "/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" "$link"
    # Remove obsolete CLI links left by the old PTY integration.
    rm -f "$HOME/.local/bin/figterm" "$HOME/.local/bin/zsh (figterm)" "$HOME/.local/bin/bash (figterm)"
    "/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" setup --dotfiles --no-confirm
    bash scripts/install-input-method.sh "/Applications/$APP_NAME.app/Contents/Helpers/FigInputMethod.app"

# Two vite servers for the debug binary to point at.
dev-web:
    pnpm -C packages/autocomplete-app dev & pnpm -C packages/dashboard-app dev; wait

dev:
    cargo build --locked -p fig_desktop --bin fig_native_worker --bin fig_io_worker
    -"/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" quit 2>/dev/null
    AUTOCOMPLETE_URL=http://localhost:3124 DASHBOARD_URL=http://localhost:3433 cargo run --locked -p fig_desktop

lint:
    cargo clippy --locked --target {{target}} --workspace -- -D warnings
    cargo +nightly fmt --version >/dev/null 2>&1 \
        || { echo ".rustfmt.toml uses unstable options: rustup toolchain install nightly --component rustfmt"; exit 1; }
    cargo +nightly fmt --check
    pnpm exec oxlint --type-aware --max-warnings 0
    pnpm exec oxfmt --check

test:
    cargo nextest run --locked --workspace
    pnpm exec turbo typecheck
    pnpm exec vitest run

# Format, lint (clippy --all-targets, oxlint type-aware), turbo boundaries, codegen zero-diff,
# typecheck, vitest, real-pty zsh tests and WKWebView smoke. Also works with CI=1.
# Local merge gate (scripts/gate.sh)
gate:
    scripts/gate.sh

# Git hooks are not versioned. Every worktree shares the one hooks directory, so run this from
# the main checkout; a link into a linked worktree dangles once the worktree is removed. A
# pre-existing hook that is a regular file is moved aside.
# Link <git common dir>/hooks/pre-push -> scripts/hooks/pre-push (runs `just gate`)
hooks:
    hooks_dir="$(git rev-parse --path-format=absolute --git-common-dir)/hooks"; \
    target="{{justfile_directory()}}/scripts/hooks/pre-push"; \
    link="$hooks_dir/pre-push"; \
    mkdir -p "$hooks_dir"; \
    if [ -e "$link" ] && [ ! -L "$link" ]; then \
        backup="$link.bak-$(date +%Y%m%d%H%M%S)"; \
        echo "moving existing $link aside to $backup"; \
        mv "$link" "$backup"; \
    fi; \
    ln -sfn "$target" "$link"; \
    echo "$link -> $target"; \
    if [ "$(git rev-parse --path-format=absolute --git-dir)" != "$(git rev-parse --path-format=absolute --git-common-dir)" ]; then \
        echo "note: this is a linked worktree; re-run 'just hooks' from the main checkout before removing it"; \
    fi

# Keystroke -> edit buffer hook at a stub desktop, release build; prints median/p90 for zsh through
# the shell hook on hook.sock and for a bare zsh (docs/latency.md).
# Measure the zsh hook path in a real PTY.
latency:
    cargo build --locked --release -p fig_cli
    cargo nextest run --locked --cargo-profile release -p zsh-hook-tests --run-ignored only --no-capture -- latency_

# Re-records tests/zsh-hook/ref/fig_*/edit_buffer.json from a real /bin/zsh and replays every case
# into a zsh running the hook. The expected edit buffers are zsh's own $PREBUFFER$BUFFER/$CURSOR,
# so a diff in edit_buffer.json means zsh changed, not the hook. Pass a comma-separated subset to
# record only those (`just golden-record fig_plain,fig_ps2`).
# Record and replay edit-buffer goldens
golden-record cases="":
    cargo build --locked -p fig_cli
    FIG_GOLDEN_RECORD=1 FIG_GOLDEN_CASES='{{cases}}' cargo nextest run --locked -p zsh-hook-tests --run-ignored only --no-capture -- golden_record
    cargo nextest run --locked -p zsh-hook-tests -- hook_golden_

# Correlate opt-in latency samples by session and input revision (docs/latency.md).
latency-report log output:
    python3 scripts/latency-report.py "{{log}}" --output "{{output}}"

clean:
    cargo clean
    rm -rf "{{build}}"
    pnpm run clean
