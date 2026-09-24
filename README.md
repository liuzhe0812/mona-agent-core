# Mona Agent Harness

Mona 是一个基于 Rust 的可复用 Agent 项目，由**核心层、扩展层、产品层（Web）**组成。既可以作为执行底座嵌入自己的应用，也可以使用配套 Web 界面配置模型、执行任务和管理会话。

编程、运维、办公等场景通过组合模型、工具和扩展实现，不需要另一套执行器。当前处于 Demo 开发阶段，接口和存储格式仍会调整，暂不提供旧版迁移保证。

[快速开始](#快速开始) · [开发者文档](docs/README.zh-CN.md) · [组件目录](packages/README.md)

## 主要能力

| 能力 | 当前实现 |
|---|---|
| 任务执行 | 模型与工具循环、流式输出、运行预算、取消、工具参数校验和有界并发；可选可靠检查点与有限模型重试 |
| 模型接入 | Chat Completions、OpenAI Responses、Anthropic Messages；自定义端点、API Key、模型能力配置与切换前历史检查 |
| 工具扩展 | 可复用的文件、命令和检索工具；通过公共接口接入业务工具，按宿主权限执行 |
| 会话与记忆 | 当前运行历史、本地会话保存和重启续聊、SQLite 历史原文检索、精选 Markdown 长期记忆 |
| 上下文处理 | 长会话摘要、项目规则与 Skills 按需加载、长工具输出归档及受控取回 |
| 上层编排 | 可选的有限计划与顺序执行，共享已有执行器和任务预算 |
| Web 与客户端 | 会话交互、模型和能力设置、记忆管理、工作区与项目组织；提供独立 HTTP/SSE、Tauri 桥接和无框架 JavaScript 客户端 |

扩展可按场景独立装配。没有安装长期 Memory 时，Agent 仍能使用当前运行的上下文；多轮历史由宿主传递或通过 Sessions 保存恢复。

## 快速开始

### 使用 Web

准备 Rust stable（含 Cargo）、Node.js 20+ 及本机编译工具链，在仓库根目录运行：

```sh
npm run dev:web
```

该命令启动 Rust Server 和 Web 界面。按终端输出的地址访问，在“设置 → 模型设置”中填写协议、端点、模型和 API Key，即可开始任务。不需要手工创建 `.env`；本机连接 Token 和模型设置存储密钥由启动器管理。

会话保存在宿主本地，重启后可继续。标准 Web 默认装配长期记忆和历史检索；Agent 更新长期记忆的工具默认关闭，可以在工具设置中授权。详细配置与生效方式见[产品层与 Web 装配](docs/architecture/WEB.zh-CN.md)。

### 嵌入自己的应用

| 使用方式 | 所需组件 |
|---|---|
| Rust 库、CLI 或业务服务 | `api`、`runtime`，以及所需模型和工具；不要求启动 Web 或数据库 |
| 浏览器或远程调用 | 执行底座 + `application` + `http-bridge` |
| Tauri 本地应用 | 执行底座 + `application` + `tauri-bridge`；不要求 HTTP 服务，原生绑定需启用 `tauri` feature |

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
| 产品层（Web） | `apps/server`、`apps/web`，以及 Application、Bridge、Client 包 | 装配能力、管理配置与授权、提供传输和界面 |
| 接入示例 | `examples/` | 最小运行、扩展组合及 Tauri 宿主示例 |
| 开发者文档 | `docs/architecture/` | 当前架构、数据流、接口契约和二次开发边界 |

完整包职责见[组件目录](packages/README.md)，生产依赖与运行调用关系见[总体架构](docs/architecture/OVERVIEW.zh-CN.md)。

## 使用边界

取消不等于撤销已经发生的外部操作；会话恢复不会自动重跑中断工具。长期 Memory 不包含后台自进化，Planner 当前不提供自动重规划。

原生插件、Shell 和工作目录绑定不是操作系统沙箱。标准本机宿主不提供完整多用户隔离；远程部署需要额外的 TLS、身份认证和权限域控制。

协议支持不代表每个模型都具备相同能力。真实供应商行为、摘要质量、生产负载与原生 Tauri 界面仍需在目标环境验证；Tauri 组合示例不是完整桌面产品。

## 开发者文档

统一入口：[文档首页与术语](docs/README.zh-CN.md)。

| 文档 | 内容 |
|---|---|
| [总体架构](docs/architecture/OVERVIEW.zh-CN.md) | 三层分工、依赖关系、装配选择与二次开发定位 |
| [核心执行机制](docs/architecture/RUNTIME.zh-CN.md) | 执行时序、模型网关、工具调度、预算、取消与检查点 |
| [上下文与三类记忆](docs/architecture/CONTEXT-MEMORY.zh-CN.md) | 工作历史、模型投影、压缩、长期记忆与历史检索 |
| [扩展接口与装配](docs/architecture/EXTENSIONS.zh-CN.md) | 工具、模型、上下文、策略和 Plugin 接入契约 |
| [产品层与 Web 装配](docs/architecture/WEB.zh-CN.md) | Server、Application、Bridge、Client 与页面的连接方式 |

## 许可证

项目采用 [MIT License](LICENSE)。随附第三方组件的许可信息见[静态资源说明](apps/web/vendor/README.md)。
