# Agent Core RS v0.3.0 · 架构

## 1. 定位

这是组件化库/SDK，不是必须运行的独立服务。目标是在权限和资源限制内执行一个任务，同时支持本机桌面与远程界面。默认 ReAct 只有一份；桥接不实现模型调用、工具执行或第二套循环。组件负责能力，公共 API 定义边界，宿主负责组合；Plugin 是可选的接入机制，不是所有组件的必需形态。

### 组件的生产依赖方向

```text
api ← runtime
api ← providers
api + providers ← models
api ← Memory / Planner 组件
api ← application
api + application ← Tauri Bridge
api + application ← HTTP Bridge
```

组合根（`apps/server`、`apps/web` 宿主和 `examples/` 中的样例）可以依赖具体实现；内层包不能反向依赖组合根。`application` 的测试会依赖 Runtime，**dev-dependencies 不是生产依赖反转**。正式应用放在 `apps/`，可运行的组合样例放在 `examples/`。

## 2. 公共契约与执行基座

`api` 定义 `AgentExecutor`（只取最终报告）、`AgentRuntime`（启动流式运行）、`RunSession`（控制/订阅/快照/最终报告）以及 `RunHandle`。默认 Engine 实现这些接口。桥接只知道应用 API；应用 API 只知道 Runtime trait。

`RunHandle.events` 在执行启动前创建，避免“任务先输出，前端后订阅”丢首包。原始 RunSession.subscribe 只订阅未来事件，发生 Lagged 后获取快照并丢弃不新于快照的事件；需要回放的调用者使用应用层。

Runtime 的职责仍是插件宿主、ReAct、模型网关、工具执行、上下文投影、运行控制和事件。工具默认独占，声明 ParallelSafe 后允许有界并行；结果按模型调用源顺序写回。完整参数、合法结束原因、完整传输、Schema 与权限检查全部通过后才能执行。

## 3. 统一应用层不是另一个 Runtime

`AgentApplication` 只实现与界面/传输无关的调用管理：

- 启动、取消、追加输入、取快照、取结果、忘记已完成任务。
- 有界任务句柄注册表、短期幂等键、输入操作幂等。
- UI 事件投影、有界内存 journal、游标订阅与失效快照。
- 订阅数量、已完成任务 TTL、关闭接入与任务排空。
- 不向不可信界面暴露 raw RunReport、系统提示词、原始请求审计和 provider 推理协议字段。

一个实例对应一个可信权限域。本版不实现用户账户、多租户认证、workspace 锁或 Session 数据库。不同权限域应使用独立的应用实例及 Host/Memory；不能在 prompt 里写 tenant_id 就认为已经隔离。

### 默认上限

| 范围 | 默认值/语义 |
|---|---|
| 应用保留任务 | 32；运行中与已完成都计数，满时拒绝，不悄悄驱逐活动任务 |
| 已完成保留时间 | 600 秒；在查询/启动时惰性清理，不是精确定时销毁 |
| 每 Run 回放 | 512 个事件，同时不超过 2 MiB 序列化字节 |
| 应用订阅 | 64；订阅结束/drop 释放额度 |
| 每 Run 追加输入幂等记录 | 128 |
| 初始 prompt / 追加输入 | 64 KiB / 16 KiB |
| UI 工作项保留 | 最近有界的 256 项，优先淘汰已结束项，累计 pruned_items |
| UI 文本/参数/日志/结果预览 | 各 64 KiB，UTF-8 安全裁剪与截断标记 |
| 工具 UI 详情 | 最多 8 个命名空间键，每个 JSON 不超过 16 KiB |

为保证在途工作项始终可结算，单轮工具上限必须小于 UI 工作项容量，即最多 255，默认仍为 32。UI 快照不是无限会话历史；原文归档与持久化依然属于扩展层。

这些值是可解释的边界而不是性能测量结果。每个工作项可能同时有多种预览，序列化转义也会扩大传输字节；部署时需按实际并发和数据测量内存。不要仅按 journal 的 2 MiB 推导整个 Run 的内存。

## 4. 三种状态

1. **Transcript / RequestAudit：**Core 的正式执行记录和规范化模型请求；内存存储，可信 Rust 调用方可获得，不直接经过公共桥接。
2. **RunSnapshot：**有界 UI 投影；包含工作项、步骤、最后序号与 RunOutcome。记忆注入和隐藏 provider 字段不成为 UI 消息。
3. **Journal：**应用层最近事件窗口，用于重连；不是数据库 WAL，不具有掉电恢复或 exactly-once 保证。

4. **RunCheckpoint：**可选sink的不可变执行快照；确认顺序独立于UI事件。存储实现外置，Core不提供自动恢复。

Core 中序号分配、快照修改和 broadcast 发送在同一个短锁中完成。应用层收到连续事件更新自己的投影；上游丢事件时，从 Core 取得原子快照并清空旧窗口、设置 reset_floor。客户端过旧游标收到明确快照，而不是从一段缺失的 token 中继续拼字符串。

## 5. 两个桥接

### HTTP

`http-bridge::router` 返回 Axum Router，不绑定端口、不创建 Engine。命令是 HTTP JSON，流是 SSE；无需同时实现 WebSocket。Bearer 校验、CORS、body 限制属于传输边界，任务行为属于 Application。

### Tauri

`tauri-plugin-bridge` 的 `tauri` feature 提供真实 Tauri 2 插件命令。默认无 feature 时仍有可独立测试的 ChannelBridge，但这不等于原生 WebView 接入已验收。

IPC 每包包含 subscription_id、delivery_id、StreamFrame，前端处理后 ACK；最多一个 IPC 包在途。慢消费者不会无限塞满 Tauri 队列，而可能在下一次读取时收到恢复快照。ACK 超时仅结束订阅，不取消任务。原生命令验证宿主注入的 WebView label，并配合 Tauri capability 限定可信本地界面。

## 6. 生命周期与组件接入

组件可以通过普通 API 直接被宿主调用；需要运行时注册或统一生命周期时，再通过 `PluginHost` 接入。PluginHost 启动时检查接口版本、服务依赖和冲突，安装失败回滚，注册表在运行期间冻结。Host shutdown 顺序是禁止接入、取消/排空 Run、撤销注册、逆序关闭资源。Plugin 入口保持薄，不复制组件能力实现。

Application shutdown 停止自己的接入并取消/等待任务；**组合根随后负责 Host.shutdown**。桥接移除或界面订阅断开不应顺手关闭共享 Application，更不能取消另一个入口仍在观察的任务。

Memory 和 Planner 作为可选组件留在 `packages/`，不搬入 Runtime。它们可以提供普通 API，必要时再提供 Plugin 入口。Planner 仍通过 AgentExecutor 多次调用同一执行器；本版没有为 Planner 提供复合任务流/子 Run 聚合协议。

## 7. 实现范围与明确缺项

已写源码：公开流式 Runtime 契约、工作项事件/原子快照、通用工具详情、统一 Application、内存回放、HTTP/Tauri 桥接、模型管理组件、JS 客户端与 reducer。

未实现：完整聊天会话/thread、持久化事件/WAL、崩溃继续执行、自动重新规划、真实代码 diff 工具、标准审批服务、WebSocket、完整桌面 UI、WASM/热加载、多租户安全沙箱、JEV 决策层。

参阅 [交付报告](DELIVERY.zh-CN.md)：源码实现不等于编译验收或生产发布。


## 8. v0.3 通用执行链路

注册时：模型/工具/权限/上下文插件 → 可选ToolSelector → 可选单一CheckpointSink → 冻结注册表。

每轮：接纳输入（如有则等待检查点） → 构造上下文投影 → 从Run工具上限串行筛选本轮工具 → BeforeModel提交 → 受预算模型调用 → 完整回复/私有数据校验 → AfterModel提交 → 逐工具预检/最终授权 → intent提交 → 执行与结果变换 → 局部结果提交 → 按源顺序加入历史 → AfterTools提交 → 下一轮。

终止时：结算未完成调用 → 可选终态提交 → RunReport及有界UI终态。关键存储失败不由观察器吞掉，也不盲目重做业务动作。

### API承担数据和契约，Core承担时序

- Content/ContentBlock、ToolOutput和ToolResult是模型/工具数据，不是UIDTO。UiToolResult保持v2文本投影。
- ToolSelector只决定本轮可用名字；RunRequest.allowed_tools和最终宿主授权是不同关口。
- CheckpointSink由外部实现；内部Checkpoints只管理单Run序号、快照和确认，不管理数据库。
- ModelOptions/ProviderData经统一网关继承、检查、预算和审计。不同Provider的编码在providers或外部适配器处理。

无新常驻服务，无第二套Agent循环，无Mona类型。没有新增第三方依赖；更丰富的类型和测试带来源码增长，不以行数替代交付验证。

见[通用性判定](GENERIC-CORE-BOUNDARY.zh-CN.md)、[内容协议](CONTENT-AND-PROVIDERS.zh-CN.md)、[检查点语义](CHECKPOINTS.zh-CN.md)。
