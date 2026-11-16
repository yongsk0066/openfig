# IPC schemas

| Schema | Boundary |
| --- | --- |
| `local.proto` | CLI/local processes → desktop, including shell hook events |
| `fig.proto` | WebView ↔ desktop API requests/responses and notifications |
| `shell.proto` | Desktop → shell operations; the zsh hook receives a versioned text encoding rather than raw protobuf |
| `fig_common.proto` | Shared message definitions |

Install `protoc` before building. After changing schemas, run `pnpm exec turbo codegen` and commit generated TypeScript under `src/`. Rust bindings are generated during compilation. The gate verifies regeneration leaves the tree unchanged.

Preserve field numbers and wire compatibility. Update native and TypeScript handlers together, including snapshots/round trips. A schema edit alone does not update the zsh text protocol: inspect `crates/fig_shell_ipc/src/hook/` and `crates/fig_integrations/src/shell/scripts/fig.zsh` when changing shell operations. Page epochs, input revisions and acknowledgements are different guards; see [architecture](../docs/architecture.md).
