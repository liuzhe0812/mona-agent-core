# Mona Agent Harness

Mona 是一个以 Rust 为执行底座的可复用 Agent 项目，由**核心层、扩展层、应用层（宿主与界面）**组成。可以作为库嵌入自己的应用，也可以使用配套 Web 或 Tauri 桌面界面配置模型、执行任务和管理会话。

编程、运维、办公等场景通过组合模型、工具和扩展实现，不需要另一套执行器。当前处于 Demo 开发阶段，接口和存储格式仍会调整，暂不提供旧版迁移保证。

[快速开始](#快速开始) · [开发者文档](docs/README.zh-CN.md) · [组件目录](packages/README.md)

## 主要能力

| 能力 | 当前实现 |
|---|---|
| 任务执行 | 模型与工具循环、流式输出、运行预算、取消、工具参数校验和有界并发；可选可靠检查点与有限模型重试 |
| 模型接入 | Chat Completions、OpenAI Responses、Anthropic Messages；自定义端点、API Key、模型能力配置与切换前历史检查 |
| 工具扩展 | 可复用的文件、命令和检索工具；通过公共接口接入业务工具，按宿主权限执行 |
| MCP 接入 | 官方 Rust SDK 的 stdio / Streamable HTTP 客户端；外部工具、按需资源与宿主管理的加密连接配置 |
| 会话与记忆 | 当前运行历史、本地会话保存和重启续聊、SQLite 历史原文检索、精选 Markdown 长期记忆 |
| 上下文处理 | 长会话摘要、项目规则与 Skills 按需加载、长工具输出归档及受控取回 |
| 计划管理 | 同一 Agent 按需维护结构化计划和进度；可选仅规划模式，由宿主显式发起后续执行 |
| 子任务委派 | 可继续的异步 Subagent，独立或继承上下文、宿主角色与模型配置、共享预算、消息与停止；子记录持久保存并由 Web 查看 |
| 本地沙箱 | 只读、工作区可写和不受限模式；原生进程后端与文件修改使用同一策略，模式按会话保存 |
| Web 与客户端 | 按宿主能力装配的 UI 模块与标准插槽，业务扩展和界面解耦；支持会话、模型、记忆、计划和工作区交互；独立 HTTP/SSE、Tauri 桥接与无框架客户端 |

扩展可按场景独立装配。没有安装长期 Memory 时，Agent 仍能使用当前运行的上下文；多轮历史由宿主传递或通过 Sessions 保存恢复。

## 技术栈与运行形态

| 部分 | 技术与职责 |
|---|---|
| Agent 执行与业务服务 | Rust；`api`/`runtime`、扩展包及 `apps/server`。主 Agent 不依赖 Python 或 Node.js 业务后端 |
| Web 界面 | HTML/CSS 与原生 JavaScript ES Modules；Node.js 用于开发启动、静态资源服务、构建和测试，不执行 Agent 循环 |
| 桌面宿主 | Tauri 2 + Rust，复用 Web 页面和同进程中的 Rust 服务；当前正式桌面壳通过带鉴权的环回 HTTP 访问服务 |
| 独立自动测评 | `eval-harness/` 使用 Python 3.11+ 提供测评引擎和 Web 服务，页面使用 JavaScript；可以独立运行，不是第二套 Agent 后端 |

## 快速开始

### 使用 Web

准备 rustup、Node.js 20+ 及本机编译工具链；Rust 版本由仓库的 `rust-toolchain.toml` 固定。浏览器自动化测试另需 Node.js 22+。在仓库根目录运行：

```sh
npm run dev:web
```

该命令启动 Rust Server 和 Web 界面。按终端输出的地址访问，在“设置 → 模型设置”中填写协议、端点、模型和 API Key，即可开始任务。不需要手工创建 `.env`；本机连接 Token 和模型设置存储密钥由启动器管理。

会话保存在宿主本地，重启后可继续。标准 Web 默认装配长期记忆和历史检索；Agent 更新长期记忆的工具默认关闭，可以在工具设置中授权。详细配置与生效方式见[应用层与 Web/桌面装配](docs/architecture/WEB.zh-CN.md)。

### 使用桌面版

安装 Tauri 2 CLI 和对应平台的桌面构建依赖后，在仓库根目录运行：

```sh
npm run dev:desktop
```

桌面版复用同一页面和产品服务，项目可通过系统文件夹选择器登记；首次仍在模型设置页配置供应商。详细入口见[桌面宿主](apps/desktop/README.md)。

### 嵌入自己的应用

| 使用方式 | 所需组件 |
|---|---|
| Rust 库、CLI 或业务服务 | `api`、`runtime`，以及所需模型和工具；不要求启动 Web 或数据库 |
| 浏览器或远程调用 | 执行底座 + `application` + `http-bridge` |
| 自行嵌入的 Tauri 应用 | 执行底座 + `application` + `tauri-bridge`；不要求 HTTP 服务，原生绑定需启用 `tauri` feature |

可先运行最小接入示例：

```sh
cargo run -p demo --bin minimal
```

该示例使用明确的离线脚本模型和加法工具，展示执行与流式输出，不会调用真实供应商。真实模型接入和自定义扩展见[装配方式](docs/architecture/OVERVIEW.zh-CN.md#assembly)与[扩展接口](docs/architecture/EXTENSIONS.zh-CN.md)。

## 项目结构

| 部分 | 主要目录 | 职责 |
|---|---|---|
| 核心层 | `packages/api`、`packages/runtime` | 通用契约与唯一默认执行循环，管理运行状态和可靠执行边界 |
| 扩展层 | `packages/providers`、`tools`、`sessions`、`memory` 等 | 可独立复用的模型、工具、上下文、存储和编排能力 |
| 应用层 | `apps/server`、`apps/web`、`apps/desktop`，以及 Application、Bridge、Client 包 | 装配能力、管理配置与授权、提供 Web 和桌面界面 |
| 接入示例 | `examples/` | 最小运行、扩展组合及 Tauri 宿主示例 |
| 独立测评工程 | `eval-harness/` | Python 测评引擎与 Web 服务，通过公开适配器调用被测 Agent，不属于主 Agent 的运行依赖 |
| 开发者文档 | `docs/architecture/` | 当前架构、数据流、接口契约和二次开发边界 |

完整包职责见[组件目录](packages/README.md)，生产依赖与运行调用关系见[总体架构](docs/architecture/OVERVIEW.zh-CN.md)。

## 使用边界

取消不等于撤销已经发生的外部操作；会话恢复不会自动重跑中断工具。长期 Memory 不包含后台自进化；Planner 不派发子 Run。Web 默认执行模式可按需规划，显式计划模式先调研和提交方案，再由用户发起执行；这不是通用审批或操作系统沙箱。

工作目录绑定本身不构成沙箱；标准 Web 另行装配本地 Sandbox，默认限制 Agent Shell 与 write/edit 的文件修改范围。不限制一般读取和联网，不自动隔离原生插件或用户手动终端；Windows/旧 Linux 内核可报告部分限制。标准本机宿主不提供完整多用户隔离，远程部署仍需 TLS、身份认证和权限域控制。

协议支持不代表每个模型都具备相同能力。真实供应商行为、摘要质量、生产负载及桌面平台兼容性仍需在目标环境验证。`apps/desktop` 是桌面应用入口；`examples/tauri-composition` 只演示进程内 Bridge 接入，两者不是同一部署方式。

## 开发者文档

统一入口：[文档首页与术语](docs/README.zh-CN.md)。

| 文档 | 内容 |
|---|---|
| [总体架构](docs/architecture/OVERVIEW.zh-CN.md) | 三层分工、依赖关系、装配选择与二次开发定位 |
| [核心执行机制](docs/architecture/RUNTIME.zh-CN.md) | 执行时序、模型网关、工具调度、预算、取消与检查点 |
| [上下文与三类记忆](docs/architecture/CONTEXT-MEMORY.zh-CN.md) | 工作历史、模型投影、压缩、长期记忆与历史检索 |
| [扩展接口与装配](docs/architecture/EXTENSIONS.zh-CN.md) | 工具、模型、上下文、策略和 Plugin 接入契约 |
| [应用层与 Web/桌面装配](docs/architecture/WEB.zh-CN.md) | Server、Application、Bridge、Client 与页面的连接方式 |

## 许可证

项目自有代码采用 [MIT License](LICENSE)。第三方许可见[静态资源说明](apps/web/vendor/README.md)、[DSH 沙箱实现参考许可](packages/sandbox/LICENSE-DSH)及 [Codex 并发许可代码来源](packages/subagent/NOTICE)（[Apache-2.0](packages/subagent/LICENSE-CODEX)）。
