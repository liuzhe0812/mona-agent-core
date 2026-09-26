# Mona Agent 开发者文档

这组文档说明 Mona 当前实现的架构、执行契约和二次开发边界，与所在提交的代码一起阅读。Mona 分为**核心层、扩展层、应用层（宿主与界面）**；核心可以独立嵌入，使用 Web 不是接入前提。

## 阅读路线

| 你要做什么 | 先读 | 再读 |
|---|---|---|
| 理解整个项目 | [总体架构](architecture/OVERVIEW.zh-CN.md) | [核心执行机制](architecture/RUNTIME.zh-CN.md) |
| 在 CLI、桌面或业务服务中嵌入 Agent | [总体架构：装配方式](architecture/OVERVIEW.zh-CN.md#assembly) | [核心执行机制：调用与生命周期](architecture/RUNTIME.zh-CN.md#ownership) |
| 增加工具、模型适配或上下文能力 | [扩展接口与装配](architecture/EXTENSIONS.zh-CN.md) | 对应公共接口和包内说明 |
| 接入会话、长期记忆、压缩或历史检索 | [上下文与三类记忆](architecture/CONTEXT-MEMORY.zh-CN.md) | [扩展接口与装配：组合约束](architecture/EXTENSIONS.zh-CN.md#composition) |
| 修改 Web、替换前端或接入 Tauri | [应用层与 Web/桌面装配](architecture/WEB.zh-CN.md) | [核心执行机制：事件和状态](architecture/RUNTIME.zh-CN.md#events) |

文档覆盖接入所需的接口、数据所有权、权限、配置生效时机、失败处理和资源生命周期；具体参数与用法见[组件目录](../packages/README.md)中的包说明。

## 架构文档地图

| 文档 | 负责回答的问题 |
|---|---|
| [总体架构](architecture/OVERVIEW.zh-CN.md) | 三层如何分工？包怎样依赖？新增业务应该改哪里？ |
| [核心执行机制](architecture/RUNTIME.zh-CN.md) | 一次 Run 怎样执行、停止、结算？预算、重试、工具并发和检查点怎样配合？ |
| [上下文与三类记忆](architecture/CONTEXT-MEMORY.zh-CN.md) | 当前历史、长期事实和历史原文怎样共存？谁压缩、谁保存、谁检索？ |
| [扩展接口与装配](architecture/EXTENSIONS.zh-CN.md) | 应使用哪个扩展接口？怎样独立复用、安装、关闭，并避免绕过核心约束？ |
| [应用层与 Web/桌面装配](architecture/WEB.zh-CN.md) | Server、Application、Bridge、Client 和页面如何连接？怎样更换宿主而不复制执行器？ |

每篇文档末尾都有源码定位。架构图表达模块关系和时序，不代表部署了图中同名的独立进程。

## 统一术语

| 术语 | 本项目含义 | 不要混淆为 |
|---|---|---|
| 应用层 | Server、Web/桌面宿主、调用管理与传输适配所在层 | 仅指 Web 前端，或新增一套业务执行器 |
| `application` 包 | 应用层中可复用的任务管理组件 | 整个应用层，或业务场景本身 |
| Host | 已装配模型、工具、扩展与运行配置的宿主对象；由 `HostBuilder` 构建 | Web Server 或一台物理服务器 |
| Engine / Runtime | 默认执行器及其通用运行接口；驱动模型与工具循环 | 另一套必须单独部署的服务 |
| Task / `TaskControl` | 一组执行共享的调用预算、用量熔断、取消和总期限；通常只有一个 Run，也可由编排器共享给多个 Run | 会话数据库中的聊天条目 |
| Session | 扩展层保存的持久会话，可包含多个用户轮次 | 一个模型 HTTP 请求 |
| Turn | Sessions 接纳的一次用户输入和对应执行记录；启动失败也可以形成失败轮次 | 一次模型推理尝试 |
| Run | 一次 `AgentRuntime.start` / `execute` 的执行实例，拥有独立运行编号和正式历史 | 整个长期会话 |
| Step | 一个 Run 中的一轮模型决策及其后续工具批次；摘要、重试可能增加实际模型调用 | 固定只发生一次模型请求的计费单位 |
| `RunSession` | 公共 API 中控制和观察单个 Run 的句柄接口 | `sessions` 包的持久 Session |
| Transcript | 当前 Run 接纳的正式消息与工具结果；初始内容由宿主提供 | UI 快照、整库历史或供应商原始 HTTP 报文 |
| Projection | 从正式历史构造的本轮模型输入，可含摘要和上下文来源 | 可覆盖会话原文的持久正本 |
| Checkpoint | 执行器等待确认的运行快照 | 自动续跑引擎、UI 回放或业务事务回滚 |

## 协议与数据版本

版本用于识别当前契约，不表示支持读取全部历史格式。以下版本对应所在提交的源码定义。

| 对象 | 当前值 | 权威定义 |
|---|---|---|
| Rust 扩展契约 | `API_VERSION = 10` | [`api/src/lib.rs`](../packages/api/src/lib.rs) |
| UI 事件协议 | `STREAM_VERSION = 2` | [`api/src/event.rs`](../packages/api/src/event.rs) |
| 运行检查点 | `CHECKPOINT_VERSION = 2` | [`api/src/checkpoint.rs`](../packages/api/src/checkpoint.rs) |
| Sessions 会话文件 | 格式 `5` | [`sessions/src/store.rs`](../packages/sessions/src/store.rs) |
| 产品 UI 模块与装配清单 | `UI_VERSION = 1` | [`ui/registry.mjs`](../apps/web/ui/registry.mjs)、[`capabilities.rs`](../apps/server/src/capabilities.rs) |

包版本、Rust 契约版本和持久文件格式是不同维度。检查点 revision、会话 revision、流事件 seq、Tauri delivery_id 也各自有独立作用，不能互相替代。

## 文档与源码分工

架构文档解释跨模块关系与接口语义；各包 README 介绍具体入口、参数及限制；源码接口与测试用例提供精确类型和可复现行为。

本文档只描述可从实现核对的能力及其限制，不以架构图或测试夹具替代真实供应商、生产负载、安全隔离和原生桌面集成验证。
