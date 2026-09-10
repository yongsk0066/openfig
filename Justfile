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
specs_src  := env("FIG_SPECS_SRC", home_directory() / "oss/fig/autocomplete")
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

# ~/.local/bin is dropped from PATH because autocomplete-tools calls `q`/`cw`/`fig settings`
# on every compile and this machine still has the Kiro CLI links there.
# Completion specs: <specs_src>/src/**/*.ts -> build/specs (index.json + <name>.js)
specs:
    cd "{{specs_src}}" && pnpm install --frozen-lockfile --ignore-scripts
    rm -rf "{{build}}/specs"
    cd "{{specs_src}}" && \
        PATH="$(echo "$PATH" | tr ':' '\n' | grep -vFx "$HOME/.local/bin" | paste -sd: -)" \
        pnpm exec autocomplete-tools compile -o "{{build}}/specs"
    test -f "{{build}}/specs/index.json"

# The filters keep turbo away from the packages the bundle does not need. They are
# paths, not package names, so the 6단계 npm scope rename cannot silently empty them.
# Web: proto -> api-bindings -> autocomplete-app, dashboard-app
web:
    pnpm install --frozen-lockfile --ignore-scripts
    pnpm exec turbo build --filter=./packages/autocomplete-app --filter=./packages/dashboard-app
    test -f packages/autocomplete-app/dist/index.html
    test -f packages/dashboard-app/dist/index.html

# Only the three binaries that go into the bundle.
rust:
    cargo build --locked --release --target {{target}} -p fig_desktop -p q_cli -p figterm

# Assemble build/<APP_NAME>.app
bundle:
    rm -rf "{{app}}"
    mkdir -p "{{app}}/Contents/MacOS" "{{app}}/Contents/Resources"
    cp "{{tdir}}/fig_desktop" "{{app}}/Contents/MacOS/$DESKTOP_BIN"
    cp "{{tdir}}/q_cli"       "{{app}}/Contents/MacOS/$CLI_BIN"
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
    cp -R "{{build}}/specs/"              "{{app}}/Contents/Resources/specs/"

# Inner executables first and the bundle last, because the top level seals what is under it.
# ad-hoc signing (01-decisions #17): no certificate, cdhash only.
sign:
    codesign --force --sign - --identifier "$BUNDLE_ID.cli" "{{app}}/Contents/MacOS/$CLI_BIN"
    codesign --force --sign - --identifier "$BUNDLE_ID.pty" "{{app}}/Contents/MacOS/$PTY_BIN"
    codesign --force --sign - --identifier "$BUNDLE_ID"     "{{app}}"
    codesign --verify --deep --strict --verbose=2 "{{app}}"

# app_bundle_path() falls back to /Applications, not ~/Applications, so the bundle goes there.
# The first `open` registers it with LaunchServices; the app then re-creates the ~/.local/bin
# links and the "<shell> (<PTY_BIN>)" copies itself (crates/fig_desktop/src/install.rs). It also
# writes a LaunchAgent, but the label there is still the hardcoded
# "com.amazon.codewhisperer.launcher" (install.rs:279) until the 6단계 branding flip derives it
# from BUNDLE_ID. A pre-existing regular file at a ~/.local/bin target (another vendor's CLI,
# not one of our symlinks) is moved aside instead of being clobbered.
# Install into /Applications and run for the first time
install:
    -"/Applications/$APP_NAME.app/Contents/MacOS/$CLI_BIN" quit 2>/dev/null
    rm -rf "/Applications/$APP_NAME.app"
    ditto "{{app}}" "/Applications/$APP_NAME.app"
    mkdir -p "$HOME/.local/bin"
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

# Run only to refresh them.
# ~/oss/fig/themes -> resources/themes (committed)
vendor-themes:
    mkdir -p resources/themes
    rsync -a --delete --include='*.json' --exclude='*' "{{themes_src}}/themes/" resources/themes/

clean:
    cargo clean
    rm -rf "{{build}}"
    pnpm run clean
