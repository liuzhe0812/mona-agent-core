# Packages

`packages/` contains reusable harness components. A component owns its capability and implementation; its public API defines the boundary, and the host chooses which components to assemble.

The runtime does not require every component to be a Plugin. A package may expose a normal API for direct use and, when runtime registration or shared lifecycle is needed, a thin Plugin entry point that delegates to the same implementation.

| Package | Responsibility |
|---|---|
| `api` | Public model, tool, run, event and Plugin contracts |
| `runtime` | The default execution loop, model gateway, tool policy and lifecycle |
| `providers` | Concrete model protocol adapters |
| `models` | Optional provider settings, catalog, routing and persistence |
| `application` | Task admission, idempotency, subscriptions, replay and retention |
| `http-bridge` | HTTP/SSE transport adapter |
| `tauri-bridge` | Tauri commands, Channel transport and ACK handling |
| `client` | Framework-free JavaScript protocol clients and `RunView` |
| `memory`, `planner` | Optional extension components |

Package names intentionally stay short. Rust crate names match their directories except `tauri-bridge`, whose Cargo package is `tauri-plugin-bridge`; the workspace dependency alias remains `tauri-bridge`. The JavaScript package name is `client`.
