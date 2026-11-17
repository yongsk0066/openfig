# OpenFig — terminal autocomplete

A personal macOS terminal-autocomplete fork of [Amazon Q Developer CLI Autocomplete](https://github.com/aws/amazon-q-developer-cli-autocomplete), originally built by Fig. It shows completion suggestions beside the terminal cursor and inserts the selected text into zsh.

Targets macOS 13 or later, Apple silicon and zsh. The installed app remains **Fig.app**, with CLI **q**, to preserve existing shell integration and macOS permissions; names are defined in `branding.env`.

AI/chat, account sign-in, product telemetry and automatic updates are removed. Completion specs are bundled locally. Specs and their generators execute commands with your user permissions; those commands may access the network. This is not a network sandbox. Developer mode can load specs from a local HTTP server.

## Build and run

Install Xcode Command Line Tools, Rust/rustup, Node (see `.node-version`), pnpm (see `package.json`), `just`, `jq` and `protoc` >= 3.12. Rust is pinned in `rust-toolchain.toml`. Full validation also needs
`cargo-nextest` and nightly rustfmt; see the development guide below.

```sh
just all       # build and sign build/Fig.app
just install   # install app, CLI link and zsh hooks
```

Grant Fig Accessibility access in System Settings → Privacy & Security → Accessibility. Open a new terminal tab and type `git `. Use `q doctor --all` to diagnose setup problems. Installation updates your shell startup files and registers the app's login launcher. See [development](docs/development.md) for signing, tests and installation details.

## Code map

| Area | Owner |
| --- | --- |
| Native event loop, WebViews, window positioning | `crates/fig_desktop/` |
| CLI and zsh hook | `crates/fig_cli/`, `crates/fig_integrations/` |
| Completion model and React UI | `packages/autocomplete-app/` |
| Dependency-injected parser/spec loading | `packages/autocomplete-parser/` |
| Settings UI | `packages/dashboard-app/` |
| API transport and schemas | `packages/api-bindings/`, `proto/` |
| Bundled completion specs | `specs/` |

Read [architecture](docs/architecture.md), [development](docs/development.md), [Ghostty input-method setup](docs/macos-input-method.md), or [latency diagnostics](docs/latency.md). There is no packaged release or automatic update channel; build locally.

## License

Project code is available under [MIT](LICENSE.MIT) or [Apache-2.0](LICENSE.APACHE). Original upstream notices and vendored component licenses are retained; see [NOTICE](NOTICE). This is an independent fork, not an Amazon or Fig supported product.

This is a personal project without a support or release schedule. See [source publication](docs/publication.md) for component licenses, asset provenance and distribution checks.

This terminal fork is unrelated to the [OpenFig.org Figma tools](https://openfig.org/). The shared name is not a claim of affiliation or exclusive branding rights.
