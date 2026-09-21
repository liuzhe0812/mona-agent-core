# Apps

`apps/` contains formal application entry points and the product-facing Web UI. These are composition roots: they select packages, configure storage and authentication, and expose the chosen interfaces.

| App | Responsibility |
|---|---|
| `server` | HTTP service composition root that assembles the runtime, application layer, bridges and optional model management |
| `web` | Formal framework-free Web UI for task interaction and settings |

`examples/` is separate and contains runnable integration or composition samples. Examples help validate how packages are assembled; they are not automatically part of the formal application or its production dependency set.
