# Apps

`apps/` contains formal application entry points and the product-facing Web UI. These are composition roots: they select packages, configure storage and authentication, and expose the chosen interfaces.

| App | Responsibility |
|---|---|
| [server](server/README.md) | HTTP composition root: capability policy, package assembly, management interfaces and workspace-scoped local conversations |
| [web](web/README.md) | Formal framework-free UI for task interaction, persisted conversation navigation and settings |

The Web application's visual contract is maintained in [UI design](../docs/ui/DESIGN.zh-CN.md), [component contracts](../docs/ui/COMPONENTS.zh-CN.md), [design governance](../docs/ui/DESIGN-GOVERNANCE.zh-CN.md), and the [production surface inventory](../docs/ui/DESIGN-INVENTORY.zh-CN.md).

`examples/` is separate and contains runnable integration or composition samples. Examples help validate how packages are assembled; they are not automatically part of the formal application or its production dependency set.
