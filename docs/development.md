# Development

## Toolchain and validation

Use the pinned Rust, Node and pnpm versions from `rust-toolchain.toml`, `.node-version` and `package.json`. Building also needs Xcode Command Line Tools, `just`, `jq` and `protoc` >= 3.12. The local gate additionally needs `cargo-nextest` and nightly rustfmt (`rustup toolchain install nightly --component rustfmt`).

```sh
just deps          # verify build prerequisites
just all           # specs, web, Rust, bundle, signing
CI=1 just gate     # complete local validation
just hooks         # optional pre-push gate
```

The gate checks formatting, zero-warning lint, core dependency boundaries, protobuf regeneration, types, JS/Rust tests, installer failure recovery, input-method geometry and a real WKWebView smoke test. It needs macOS with an interactive GUI session for native checks. The native-process tests inject hung, crashed, partial, oversized and mismatched replies, verify automatic recovery/reaping and bounded icon admission, and exercise the real helper PNG output. The popup-root regression test checks fresh layout with no `ResizeObserver` callbacks. The mocked installer tests cover backup retention, helper replacement versus signature-only updates, and copy, signature, missing-helper, process-stuck and launch failures. Staging tests verify that copy/signature failures preserve the live app and never launch or stop a process. GitHub Actions builds an ad-hoc-signed public bundle on macOS arm64 from a fresh checkout and runs the same gate with `--headless`. This explicit mode skips only the WKWebView runtime smoke and reports that omission; `CI=1` alone never skips it. Hosted CI does not validate real terminal placement, login-item approval or input-method enrollment. A successful push alone is not a validation result.

For development, run `just dev-web` in one terminal and `just dev` in another. The latter stops the installed app and runs the native host against autocomplete on port 3124 and settings on port 3433.

## Installation and signing

`just stage` copies and verifies a uniquely named update under `/Applications/.fig-updates` without stopping, replacing or launching the installed app, or changing input-method registration. It does not activate the update; use the regular installation path when a restart is permitted.

`just install` stages the signed bundle, verifies its signature and required executables (including the separately signed `fig_native_worker`), then asks the installed app to quit and waits for it to stop. Copy/signature failures or a process that refuses to stop leave the old bundle in place. It then moves the old bundle to `/Applications/.fig-backups`, swaps the staged bundle into `/Applications/Fig.app` and launches it. Replacement or launch-command failures trigger an attempt to restore and relaunch the old bundle. A successful `open` command only means launch acceptance; runtime health needs a separate check. It also links `~/.local/bin/q` and installs zsh startup hooks. Existing regular files at the CLI link target are moved aside. The launch-at-login setting expresses intent to `SMAppService.mainAppService` (macOS 13+). Registration and approval are distinct. A successfully applied intent is recorded before the old LaunchAgent file is removed; failures preserve a retryable installation. The old running job is not unloaded, since it may own the desktop. Unchanged intent never overrides a later System Settings change. Toggle off/on to request registration again. Login launches keep the dashboard hidden; normal launches and reopening still show it. Only the installed bundle can manage login items, and uninstallation unregisters the service before deleting the app.

[Input-method enrollment](macos-input-method.md) runs after replacement. Pending approval does not remove or leave the desktop stopped. Mocked recovery tests do not validate macOS consent prompts.

`just sign` selects an available Apple Development / Developer ID identity, or falls back to ad-hoc signing. Override with `FIG_SIGN_IDENTITY`. Reuse the same identity to preserve Accessibility authorization across builds; ad-hoc rebuilds may require reauthorizing Fig. `branding.env` supplies public defaults. An ignored `bundle-id.local` file containing only an existing installation's bundle ID overrides that ID consistently in Justfile, Rust and the input-method bundle. Keep it to preserve that installation's existing grants; do not publish it. App/CLI names and data paths remain in `branding.env`. Optional private theme JSON files in `themes.local/` are copied only into local builds. Do not redistribute those builds without reviewing those themes.

Shells that have loaded the current hook automatically reconnect after desktop replacement, including an unchanged prompt. Already-running shells with an older hook retain that older code until reloaded or replaced by a new shell. After activation, verify `git ` suggestions, arrows/Tab, click then type, rapid edit/delete, app switching, window move/resize, Ghostty split switching and description popups. Verify settings and shortcuts persist after reopening. Automated tests do not replace this actual terminal check.

## Changes and diagnostics

Edit schemas in `proto/` and run `pnpm exec turbo codegen`; commit generated TypeScript. Rust generates its bindings during compilation. Keep field numbers compatible with existing clients. See [API bindings](../packages/api-bindings/README.md) and [protocol](../proto/README.md).

Use `q doctor --all` for shell/socket/permission diagnostics and [latency diagnostics](latency.md) for timing. Keep logs, profiles, local validation outputs and app backups outside the repository. General trace logs may include commands or environment data.

Comments should explain ownership, invariants, compatibility and non-obvious decisions. Keep `SAFETY` obligations next to unsafe operations; avoid historical task numbers, deleted-document references and comments that merely restate code. Leave vendored license/copyright notices intact.

For license inventory, asset provenance and history checks, see [source publication](publication.md).
