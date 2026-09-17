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
themes_src := env("FIG_THEMES_SRC", home_directory() / "oss/fig/themes")
version    := `cargo metadata --format-version 1 --no-deps --offline | jq -r '.packages[] | select(.name=="fig_desktop") | .version'`
min_macos  := `rustc --print deployment-target --target aarch64-apple-darwin | sed 's/^.*=//'`

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
# paths, not package names, so the 6단계 npm scope rename cannot silently empty them.
# Web: proto -> api-bindings -> autocomplete-app, dashboard-app
web:
    pnpm install --frozen-lockfile --ignore-scripts
    rm -rf packages/autocomplete-app/dist packages/dashboard-app/dist
    pnpm exec turbo build --force --filter=./packages/autocomplete-app --filter=./packages/dashboard-app
    test -f packages/autocomplete-app/dist/index.html
    test -f packages/dashboard-app/dist/index.html

# Only the three binaries that go into the bundle.
rust:
    cargo build --locked --release --target {{target}} -p fig_desktop -p fig_cli -p figterm

# Assemble build/<APP_NAME>.app
bundle:
    rm -rf "{{app}}"
    mkdir -p "{{app}}/Contents/MacOS" "{{app}}/Contents/Resources"
    cp "{{tdir}}/fig_desktop" "{{app}}/Contents/MacOS/$DESKTOP_BIN"
    cp "{{tdir}}/fig_cli"     "{{app}}/Contents/MacOS/$CLI_BIN"
    cp "{{tdir}}/figterm"     "{{app}}/Contents/MacOS/$PTY_BIN"
    VERSION='{{version}}' MIN_MACOS='{{min_macos}}' \
        perl -pe 's{\@(\w+)\@}{ my $v = $ENV{$1}; die "no value for \@$1\@\n" unless defined $v; $v =~ s/&/&amp;/g; $v =~ s/</&lt;/g; $v =~ s/>/&gt;/g; $v }ge' \
        bundle/macos/Info.plist.in > "{{app}}/Contents/Info.plist"
    plutil -lint "{{app}}/Contents/Info.plist"
    printf 'APPL????' > "{{app}}/Contents/PkgInfo"
    cp bundle/macos/AppIcon.icns "{{app}}/Contents/Resources/AppIcon.icns"
    cp -R packages/autocomplete-app/dist/ "{{app}}/Contents/Resources/autocomplete/"
    cp -R packages/dashboard-app/dist/    "{{app}}/Contents/Resources/dashboard/"
    cp -R resources/themes/               "{{app}}/Contents/Resources/themes/"
    cp -R specs/dist/                     "{{app}}/Contents/Resources/specs/"

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
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID.cli" "{{app}}/Contents/MacOS/$CLI_BIN"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID.pty" "{{app}}/Contents/MacOS/$PTY_BIN"; \
    codesign --force --sign "$identity" --identifier "$BUNDLE_ID"     "{{app}}"
    codesign --verify --deep --strict --verbose=2 "{{app}}"

# app_bundle_path() falls back to /Applications, not ~/Applications, so the bundle goes there.
# The first `open` registers it with LaunchServices; the app then re-creates the ~/.local/bin
# links and the "<shell> (<PTY_BIN>)" copies itself (crates/fig_desktop/src/install.rs). It also
# writes a LaunchAgent labelled "$BUNDLE_ID.launcher" (fig_util::LAUNCH_AGENT_LABEL); making it
# conditional on app.launchOnStartup is 8단계. A pre-existing regular file at a ~/.local/bin
# target (another vendor's CLI,
# not one of our symlinks) is moved aside instead of being clobbered.
# Install into /Applications and run for the first time.
# The per-shell figterm copies are replaced by rename, never overwritten in place: an
# executable that is currently running (old terminal tabs) keeps its inode, and the kernel
# would otherwise mark the rewritten pages "Code Signature Invalid" and SIGKILL every new
# figterm (that was a real bug on 2026-09-11).
install:
    -"/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" quit 2>/dev/null
    for _ in $(seq 1 50); do pgrep -x "$DESKTOP_BIN" >/dev/null || break; sleep 0.1; done
    rm -rf "/Applications/$APP_NAME.app"
    ditto "{{app}}" "/Applications/$APP_NAME.app"
    mkdir -p "$HOME/.local/bin"
    for shell in zsh bash; do \
        cp "/Applications/$APP_NAME.app/Contents/MacOS/$PTY_BIN" "$HOME/.local/bin/.$shell ($PTY_BIN).new" \
            && mv -f "$HOME/.local/bin/.$shell ($PTY_BIN).new" "$HOME/.local/bin/$shell ($PTY_BIN)"; \
    done
    for bin in "$CLI_BIN" "$PTY_BIN"; do \
        link="$HOME/.local/bin/$bin"; \
        if [ -f "$link" ] && [ ! -L "$link" ]; then \
            backup="$link.bak-$(date +%Y%m%d%H%M%S)"; \
            echo "moving existing $link aside to $backup"; \
            mv -n "$link" "$backup"; \
        fi; \
        ln -sf "/Applications/$APP_NAME.app/Contents/MacOS/$bin" "$link"; \
    done
    open "/Applications/$APP_NAME.app" --args --no-dashboard
    "/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" setup --dotfiles --no-confirm

# Two vite servers for the debug binary to point at.
dev-web:
    pnpm -C packages/autocomplete-app dev & pnpm -C packages/dashboard-app dev; wait

dev:
    -"/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" quit 2>/dev/null
    AUTOCOMPLETE_URL=http://localhost:3124 DASHBOARD_URL=http://localhost:3433 cargo run -p fig_desktop

lint:
    cargo clippy --locked --target {{target}} --workspace -- -D warnings
    cargo +nightly fmt --version >/dev/null 2>&1 \
        || { echo ".rustfmt.toml uses unstable options: rustup toolchain install nightly --component rustfmt"; exit 1; }
    cargo +nightly fmt --check
    pnpm exec oxlint
    pnpm exec oxfmt --check

test:
    cargo nextest run --locked --workspace
    pnpm exec turbo typecheck
    pnpm exec vitest run

# Format, lint (clippy --all-targets, oxlint type-aware), turbo boundaries, codegen zero-diff,
# typecheck, vitest, every nextest test including the real-pty figterm2 suite. There is no CI;
# this is it, and it must also pass as `CI=1 just gate`.
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

# Keystroke -> edit buffer hook at a stub desktop, bash and zsh, release build; prints
# median/p90 per shell (docs/16-latency.md).
# Stage 0 latency, figterm half
latency:
    cargo build --locked --release -p figterm -p fig_cli
    cargo nextest run --locked --cargo-profile release -p figterm2 --run-ignored only --no-capture -- latency_

# Per-hop median/p90 from a desktop log written with FIG_LOG_LEVEL=trace (docs/16-latency.md).
# Stage 0 latency, desktop half
latency-trace log=(env("TMPDIR", "/tmp") / "figlog" / "fig_desktop.log"):
    scripts/latency-trace.sh "{{log}}"

# Run only to refresh them.
# ~/oss/fig/themes -> resources/themes (committed)
vendor-themes:
    mkdir -p resources/themes
    rsync -a --delete --include='*.json' --exclude='*' "{{themes_src}}/themes/" resources/themes/

clean:
    cargo clean
    rm -rf "{{build}}"
    pnpm run clean
