# Fig

A personal fork of the terminal autocomplete engine that started life as
[Fig](https://github.com/withfig) and was later maintained by Amazon as the
autocomplete half of the Amazon Q Developer CLI. It draws an IDE-style
completion dropdown over your terminal as you type.

This fork strips the project back to that one feature. There is no AI, no chat,
no telemetry, no account, and no auto-update: the app never talks to the
network. It targets macOS on Apple silicon only, is built and installed
locally, and is not published anywhere.

## What is in here

| Piece | Path |
| --- | --- |
| Desktop app (WebView host, tray, window positioning) | `crates/fig_desktop/` |
| CLI (`fig`) | `crates/fig_cli/` |
| PTY shim (`figterm`) that watches the edit buffer | `crates/figterm/` |
| Shell integration (zsh, bash) | `crates/fig_integrations/src/shell/` |
| Autocomplete UI (React) | `packages/autocomplete-app/` |
| Settings dashboard (React) | `packages/dashboard-app/` |
| Parsers, API bindings, shared utilities | `packages/*/` |
| IPC protocol | `proto/` |

Every product name — the bundle, the binaries, the URL scheme, the data
directory — is derived from `branding.env`, which is the single source of truth
for the Rust constants, the `Info.plist` and the `just` recipes alike.

## Building

Requirements: macOS on Apple silicon, Xcode command line tools, Rust (the
channel is pinned by `rust-toolchain.toml`), Node >= 26, pnpm (the version
pinned by `package.json`), `protoc` >= 3.12, and `just`.

```sh
just deps       # verify the toolchain
just all        # specs + web + rust + bundle + sign  ->  build/Fig.app
just install    # copy to /Applications, link ~/.local/bin, install shell hooks
```

Then grant Accessibility permission to `/Applications/Fig.app` in System
Settings → Privacy & Security → Accessibility, open a new terminal window, and
start typing a command.

Other useful recipes:

```sh
just lint       # clippy, rustfmt, oxlint, oxfmt
just test       # cargo nextest, turbo typecheck, vitest
just dev-web    # vite dev servers for the two web apps
just dev        # run the desktop binary against those dev servers
just clean
```

## Credits and license

This is a fork of
[aws/amazon-q-developer-cli-autocomplete](https://github.com/aws/amazon-q-developer-cli-autocomplete),
which is itself derived from the autocomplete engine originally built by Fig.
All of the original work is theirs; the changes here are subtractive.

Licensed under either of [Apache License, Version 2.0](LICENSE.APACHE) or
[MIT license](LICENSE.MIT) at your option. See [NOTICE](NOTICE) for the
attribution and for the licenses of the vendored third-party code.
