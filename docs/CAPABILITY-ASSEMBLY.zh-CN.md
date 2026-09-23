# Agent 组件与工具装配管理

## 目标

最小 Agent 通过三层入口完成装配：集成开发者使用 Rust 公共 API 组合组件与工具；部署者使用宿主配置决定交付策略；最终用户在 Web UI 的独立页面中管理允许调整的可选组件和可选工具。管理能力属于正式宿主，不改变 Runtime 的执行循环或公共任务协议。

## 部署配置

`apps/server` 默认读取启动目录下的 `agent.toml`。也可以使用 `--config <path>` 或 `AGENT_CONFIG_PATH` 指定文件。以下示例列出常用装配项；未显式覆盖的项使用内置默认值：

```toml
version = 1

[capabilities.model-management]
enabled = true
user_configurable = false

[capabilities.instructions]
enabled = true
user_configurable = true

[capabilities.skills]
enabled = false
user_configurable = true

[capabilities.compaction]
enabled = true
user_configurable = true

[capabilities.spill]
enabled = true
user_configurable = true

[capabilities.read]
enabled = true
user_configurable = false

[capabilities.shell]
enabled = true
user_configurable = false

[capabilities.edit]
enabled = true
user_configurable = false

[capabilities.write]
enabled = true
user_configurable = false

[capabilities.grep]
enabled = false
user_configurable = true

[capabilities.find]
enabled = false
user_configurable = true

[capabilities.ls]
enabled = false
user_configurable = true
```

- `enabled` 是没有用户选择时的默认装配状态。
- `user_configurable` 决定最终用户能否在对应的“Agent 组件”或“Agent 工具”页面改变下一次启动的状态。
- Cargo feature 决定能力是否进入二进制；配置文件不能启用未编译的能力。
- `read/shell/edit/write` 是正式 Agent 的固定基础工具；部署配置必须保持启用且不可由用户关闭，因此不出现在工具设置页。`grep/find/ls` 是可选工具，默认关闭，可独立启用并在重启后进入模型工具列表。
- `shell` 使用宿主解析的 `ShellConfig`，并在工具描述中声明实际后端语法。Windows 优先 `pwsh.exe`，否则使用系统 Windows PowerShell；Linux/Unix 优先 `bash`，否则使用 `sh`。Windows 默认不依赖 Git Bash。
- `AGENT_SHELL_PATH` 是部署覆盖；`AGENT_BASH_PATH` 仅用于旧部署兼容，二者同时存在时显式的新配置优先。测试可用 `SHELL_PATH` 覆盖 shell 可执行文件，不作为部署配置。
- Server 发行版默认编译 Skills，但仓库配置和无配置时的内置默认均为关闭；只有用户显式启用并重启后才扫描技能根和发布摘要目录。Skills 通过固定 `read` 加载，不增加工具数量。
- `AGENT_MODEL_MANAGEMENT=0`、`AGENT_SKILLS=0`、`AGENT_INSTRUCTIONS=0`、`AGENT_COMPACTION=0` 与 `AGENT_SPILL=0` 可把对应能力锁定为关闭。
- `instructions` 是始终编译、可关闭的宿主项目规则模块，内置默认开启；自动读取的路径、体积、刷新和写前检查见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)。它不扩大工具权限。

不指定配置且启动目录没有 `agent.toml` 时，使用内置默认值。生产部署应把选定配置作为部署资产显式传入，不依赖进程工作目录。

## 用户设置与覆盖顺序

用户选择保存在宿主状态目录的 `capabilities.json`，可用 `AGENT_CAPABILITY_STATE_PATH` 指定位置。它不包含凭据。生效顺序为：

1. 能力必须已经编译进 Server。
2. `user_configurable = false` 时始终使用部署配置。
3. `user_configurable = true` 时使用已保存的用户选择；没有选择时使用部署默认值（Skills 默认关闭）。

保存成功只改变下一次启动的装配。宿主内部保留当前进程实际状态，并在管理视图中按 `components` 与 `tools` 分组返回 `enabled`（下次启动）和 `restart_required`；页面只在确实需要重启时提示。正在执行的任务不会因设置变化而改变组件或工具。

## 宿主接口

能力管理接口与任务接口使用同一 Bearer 权限域，但不注册成模型可调用工具：

- `GET /api/capabilities`：返回 `{ revision, components, tools }`。清单只包含已经编译且 `user_configurable = true` 的项目；固定基础项、部署锁定项和未编译项不会暴露为用户设置。
- `PUT /api/capabilities/{id}`：保存 `{ revision, enabled }`；仅允许修改 `user_configurable` 的组件或工具。

写入使用配置版本防止并发覆盖，并在持久化成功后才发布新状态。能力状态文件按单宿主进程设计；多进程部署需要替换为共享事务存储。

## Web UI

最终用户入口拆为“设置 → Agent 组件”和“设置 → Agent 工具”。组件页只展示可开关的可选组件，工具页只展示可开关的可选工具；两页都不显示“当前已启用”“可由当前用户管理”等对操作没有帮助的标签。固定基础项、最小运行必备项和部署锁定项直接隐藏。“模型设置”独立负责供应商、凭据、模型目录和默认模型，不混入组件清单。

正式 Web 组合固定提供四个基础工具，默认关闭三个检索工具，默认启用上下文压缩和长结果归档。归档通过固定 `read` 取回，不增加第五个默认工具。Skills 随发行版编译但默认关闭，显式启用并重启后才扫描目录和发布目录投影。

首版不提供动态库下载、插件市场或运行时热装卸。新增 Rust 能力仍由集成开发者接入并重新构建；部署者和最终用户只能选择该 Server 已经包含的能力。

## 启动示例

```sh
cargo run -p server -- --config agent.toml
```

本地 Web 开发继续使用 `npm run dev:web`，启动器会为用户能力选择提供独立状态文件。
