# Desktop API bindings

Browser adapters for the desktop API: request/response transport, subscriptions, settings, filesystem and process operations. Schemas and generated message types live in [proto](../../proto/README.md).

Completion cores receive these operations through injected ports; do not import this package into core model/parser code. Request lifetimes, cancellation and subscription disposal belong to their calling owner. See [architecture](../../docs/architecture.md).
