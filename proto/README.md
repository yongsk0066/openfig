# Proto

We use [protocol buffers](https://developers.google.com/protocol-buffers/) as a
message format for inter process communication.

This folder defines:

1. `local.proto` - Protocol for communication from local processes like the
   `fig` CLI to the desktop app, and the hooks shell sessions report
2. `fig.proto` - Protocol for communication between client Fig.js apps like
   autocomplete and the desktop app
3. `shell.proto` - What the desktop asks of a shell session; `fig.zsh` receives
   it as lines of text from `fig_shell_ipc::hook`, not as protobuf
4. `fig_common.proto` - Definitions shared by the others

## Setup

For any client, you must install the protobuf compiler:

```shell
brew install protobuf
```

**Client Installations**

| Client     | Command        |
| ---------- | -------------- |
| typescript | `pnpm install` |
| rust       | N/A\*          |

\* The rust build process handles the installation of the proto toolchain.

## Installation/Usage

The generated TypeScript lives in `src/` and is committed. To regenerate it
after editing a `.proto`, run:

```shell
pnpm exec turbo codegen
```

## Deprecating an API

1. Edit `fig.proto` and add the `[deprecated=true]` annotation to the relevant
   fields
2. Add an inline comment specifying the version when this was changed applies
   using the following format: `//deprecated: 1.2.3`

## Contributing

**Adding to protos**

Just edit the appropriate proto file.
