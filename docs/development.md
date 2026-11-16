# Development

## Toolchain and validation

Use the pinned Rust, Node and pnpm versions from `rust-toolchain.toml`, `.node-version` and `package.json`. Building also needs Xcode Command Line Tools, `just`, `jq` and `protoc` >= 3.12. The local gate additionally needs `cargo-nextest` and nightly rustfmt (`rustup toolchain install nightly --component rustfmt`).

```sh
just deps          # verify build prerequisites
just all           # specs, web, Rust, bundle, signing
CI=1 just gate     # complete local validation
just hooks         # optional pre-push gate
```

The gate checks formatting, zero-warning lint, core dependency boundaries, protobuf regeneration, types, JS/Rust tests and a real WKWebView smoke test. It needs macOS with an interactive GUI session for native checks. There is no GitHub Actions workflow; a successful push alone is not a validation result.

For development, run `just dev-web` in one terminal and `just dev` in another. The latter stops the installed app and runs the native host against autocomplete on port 3124 and settings on port 3433.

## Installation and signing

`just install` replaces `/Applications/Fig.app`, links `~/.local/bin/q` and installs zsh startup hooks. Keep a copy of the existing app when you need rollback. Existing regular files at the CLI link target are moved aside. The app's launch-on-startup setting controls its login launcher.

`just sign` selects an available Apple Development / Developer ID identity, or falls back to ad-hoc signing. Override with `FIG_SIGN_IDENTITY`. Reuse the same identity to preserve Accessibility authorization across builds; ad-hoc rebuilds may require reauthorizing Fig. `branding.env` controls bundle ID, app/CLI names and data paths: renaming the repository does not change them.

After installation, open a new terminal tab and verify `git ` suggestions, arrows/Tab, click then type, rapid edit/delete, app switching, window move/resize and description popups. Verify settings and shortcuts persist after reopening. Automated tests do not replace this actual terminal check.

## Changes and diagnostics

Edit schemas in `proto/` and run `pnpm exec turbo codegen`; commit generated TypeScript. Rust generates its bindings during compilation. Keep field numbers compatible with existing clients. See [API bindings](../packages/api-bindings/README.md) and [protocol](../proto/README.md).

Use `q doctor --all` for shell/socket/permission diagnostics and [latency diagnostics](latency.md) for timing. Keep logs, profiles, local validation outputs and app backups outside the repository. General trace logs may include commands or environment data.

Comments should explain ownership, invariants, compatibility and non-obvious decisions. Keep `SAFETY` obligations next to unsafe operations; avoid historical task numbers, deleted-document references and comments that merely restate code. Leave vendored license/copyright notices intact.
