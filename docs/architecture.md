# Architecture

## Input to suggestions

```text
zsh / fig.zsh → local hook socket → Rust desktop → WKWebView delivery
                                                  ↓
                                   completion model → React list
                                                  ↓
                              epoch/revision checked insertion → zsh
```

The shell owns its edit buffer. The desktop owns native windows, accessibility and shell connections. The WebView owns parsing, generation and selection; React renders that state. Settings use a separate WebView.

`packages/autocomplete-app/src/fig/services.ts` is the completion composition root. It injects process, filesystem, spec import, history, settings, storage, clock and effect adapters into core factories. `state/model.ts` owns completion transitions; `parser/coordinator.ts` suppresses superseded parse results. Parser/spec factories live under `packages/autocomplete-parser/src/`. Cores must not acquire React, desktop adapters or browser globals. `scripts/check-architecture.mjs` checks designated roots and their transitive workspace dependencies; it is not a proof about every module.

The native `ApplicationServices` owns shared backend services; `EventSender` is the main-loop command port. AppKit/WebView mutations execute on the main event loop. Slow focused-window revalidation runs serially on a worker, carries numeric identities only, and publishes results guarded by PID, window ID and focus generation.

## Consistency and lifetimes

| Boundary | Contract |
| --- | --- |
| Page reload | Page epochs reject effects and acknowledgements from obsolete WebViews. Runtime hello/ready and ACK/NACK coordinate delivery and resynchronization. ACK does not prove parsing, DOM commit or pixel presentation. |
| Input | Session and input revision distinguish edits. Parse/generator cancellation and generation checks prevent old results overwriting current state. |
| Shell insertion | Versioned edit messages carry expected revision; the shell accepts a matching edit or rejects it. |
| Catalog cache | Spec-source/index cache shared within the completion owner; bundled files are local. |
| Execution cache | Bounded scopes keyed by session, cwd, environment and spec-source settings. Version requests share in-flight work, separate from one input's cancellation. |
| Parser cache | Execution scope plus parser-relevant settings; cosmetic changes do not invalidate command facts. |
| Geometry | Latest pending caret geometry is merged per event-loop turn; size requests consume it immediately. Hide/show and dry-run behavior remain explicit. |
| Disposal | Completion owner aborts work, disposes model subscriptions and flushes recency persistence. |

Filesystem listing is a native request returning an array of names, preserving embedded newlines and optional directory suffixes. It has no persistent directory cache. External generator hosts retain the command-based fallback. Protobuf request/response and native WebView delivery have distinct roles; see [protocol schemas](../proto/README.md).

## Current boundaries and next work

- **Native thread ownership:** `WindowServer`/`AXObserver` lifetime assumes create, mutation and teardown on their owning run loop. Some `unsafe Send`/`Sync` declarations still rely on project invariants. Move observer ownership fully into main-loop state and carry identities/messages across tasks before removing those declarations. Concurrent AX calls and icon-conversion thread contracts also need dedicated validation; comments are not proof.
- **Input latency:** native focus validation is offloaded, but some AX caret work and AppKit/WebKit scheduling remain. Measure matched workloads with sufficient visible samples before further queue or protocol changes. Historical queue outliers remain unattributed; overall input-to-pixel acceleration is not proven.
- **Cache freshness:** replacing an executable at the same path or changing a version-manager file does not automatically invalidate every execution fact. Add explicit invalidation only with tests for affected scopes and bounded cost.

React is the current presentation adapter. The injected core permits another UI, but a Web Components/native rewrite has no demonstrated benefit from current measurements. Keep the current boundary until profiling identifies a rendering bottleneck.
