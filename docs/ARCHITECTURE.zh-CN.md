# Agent Core RS v0.3.0 · 架构

## 1. 定位

这是组件化库/SDK，不是必须运行的独立服务。目标是在权限和资源限制内执行一个任务，同时支持本机桌面与远程界面。默认 ReAct 只有一份；桥接不实现模型调用、工具执行或第二套循环。组件负责能力，公共 API 定义边界，宿主负责组合；Plugin 是可选的接入机制，不是所有组件的必需形态。

### 组件的生产依赖方向

```text
api ← runtime
api ← providers
api ← tools
api + providers ← models
api ← skills
api ← instructions
api + 可选 compaction ← sessions
api ← compaction
api ← spill
api ← Memory / Planner 组件
api + 可选 sessions ← application
api + application ← Tauri Bridge
api + application ← HTTP Bridge
```

组合根（`apps/server`、`apps/web` 宿主和 `examples/` 中的样例）可以依赖具体实现；内层包不能反向依赖组合根。`application` 的测试会依赖 Runtime，**dev-dependencies 不是生产依赖反转**。正式应用放在 `apps/`，可运行的组合样例放在 `examples/`。

正式 Server 通过 `agent.toml` 应用部署者的能力策略，再叠加允许的用户选择并构造 Host。Web UI 只管理宿主已经包含且部署者允许调整的能力；启停在下次启动装配，不修改运行中的 Host。详见 [Agent 能力装配与管理](CAPABILITY-ASSEMBLY.zh-CN.md)。

## 2. 公共契约与执行基座

`packages/tools` 实现 Pi 风格的四个固定基础工具 `read/shell/edit/write` 和三个可选检索工具 `grep/find/ls`。正式 Server 负责固定装配四工具并授权副作用工具；Runtime 仍只依赖公共 `Tool` 契约，其他宿主可以追加工具，重名注册会失败。`shell` 通过公开的 `ShellTool`、`ShellConfig`、`ShellKind` 选择可信宿主后端，并在模型工具描述中声明 PowerShell、Bash 或 POSIX `sh` 语法。

Skills 通过独立的 `packages/skills` 提供 Registry、本地 Provider 和摘要目录投影。Agent 使用固定 `read` 加载完整 `SKILL.md` 与相对资源，使用已经授权的 `shell` 执行脚本；不注册专用 `skill` 工具。正式 Web 发行版包含该组件但默认关闭。详见 [Skills](../packages/skills/README.md)。

Compaction 和 Spill 也保持独立：前者只改模型可见投影，通过同一受预算模型网关摘要较早历史；后者在结果硬限制前归档长纯文本。独立 Spill Plugin 可提供 `spill_read`，正式 Server 将 `spill:` 引用接入固定 `read`，因此默认模型工具仍只有四个。正式 Web 默认装配二者，短任务不产生摘要调用或落盘。

`api` 定义 `AgentExecutor`（只取最终报告）、`AgentRuntime`（启动流式运行）、`RunSession`（控制/订阅/快照/最终报告）以及 `RunHandle`。默认 Engine 实现这些接口。桥接只知道应用 API；应用 API 只知道 Runtime trait。

`RunHandle.events` 在执行启动前创建，避免“任务先输出，前端后订阅”丢首包。原始 RunSession.subscribe 只订阅未来事件，发生 Lagged 后获取快照并丢弃不新于快照的事件；需要回放的调用者使用应用层。

Runtime 的职责仍是插件宿主、ReAct、模型网关、工具执行、上下文投影、运行控制和事件。工具默认独占，声明 ParallelSafe 后允许有界并行；结果按模型调用源顺序写回。完整参数、合法结束原因、完整传输、Schema 与权限检查全部通过后才能执行。

模型适配器发布结构化失败事实和可选上下文容量；统一模型网关负责默认关闭的有限重试、预算、取消与审计。上下文超限只给投影组件一次缩小机会。原始历史接纳上限、模型请求上限、工具执行上限和 UI 快照保留量分别配置，避免产品展示容量反向约束嵌入式 Runtime。

## 3. 统一应用层不是另一个 Runtime

`AgentApplication` 只实现与界面/传输无关的调用管理：

- 启动、取消、追加输入、取快照、取结果、忘记已完成任务。
- 有界任务句柄注册表、短期幂等键、输入操作幂等。
- UI 事件投影、有界内存 journal、游标订阅与失效快照。
- 订阅数量、已完成任务 TTL、关闭接入与任务排空。
- 不向不可信界面暴露 raw RunReport、系统提示词、原始请求审计和 provider 推理协议字段。

一个 Application 实例对应一个可信权限域，不实现用户账户、多租户认证、workspace 锁或 Session 数据库。`sessions` 扩展提供工作空间隔离的持久会话及本地写者锁，Application 的可选会话适配连接现有 Run 注册表，Server 装配并提供管理接口，见 [本地会话](LOCAL-SESSIONS.zh-CN.md)。不同权限域仍应使用独立的应用实例及 Host/Memory；不能在 prompt 里写 tenant_id 就认为已经隔离。

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

活动执行状态独立于 UI 缓存，单轮工具上限不再受 256 项展示容量约束：默认 32，API 硬上限 4096。展示淘汰不影响终态结算。UI 快照不是完整会话历史；持久会话从宿主确认过的正式记录派生，而不是把裁剪过的展示缓存当原文。

这些值是可解释的边界而不是性能测量结果。每个工作项可能同时有多种预览，序列化转义也会扩大传输字节；部署时需按实际并发和数据测量内存。不要仅按 journal 的 2 MiB 推导整个 Run 的内存。

## 4. 执行、展示与保存状态

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

已写源码：公开流式 Runtime 契约、工作项事件/原子快照、通用工具详情、统一 Application、内存回放、HTTP/Tauri 桥接、模型管理、Skills、Compaction、Spill、JS 客户端与 reducer。

正式 Web 已实现线性本地会话、历史查看和显式下一轮续聊。未实现：树形会话分支、完整持久化事件/WAL、自动崩溃继续执行、跨设备同步、自动重新规划、标准审批服务、WebSocket、完整桌面 UI、WASM/热加载、多租户安全沙箱、JEV 决策层。

参阅 [交付报告](DELIVERY.zh-CN.md)：源码实现不等于编译验收或生产发布。


## 8. v0.3 通用执行链路

注册时：模型/工具/权限/上下文插件 → 可选ToolSelector → 可选单一CheckpointSink → 冻结注册表。

每轮：接纳输入（如有则等待检查点） → 从Run工具上限筛选本轮工具 → 收集来源并计入预算 → 构造上下文投影 → BeforeModel提交 → 受预算模型调用 → 完整回复/私有数据校验 → AfterModel提交 → 逐工具预检/最终授权 → intent提交 → 执行与结果变换 → 局部结果提交 → 按源顺序加入历史 → AfterTools提交 → 下一轮。

终止时：结算未完成调用 → 可选终态提交 → RunReport及有界UI终态。关键存储失败不由观察器吞掉，也不盲目重做业务动作。

### API承担数据和契约，Core承担时序

- Content/ContentBlock、ToolOutput和ToolResult是模型/工具数据，不是UIDTO。UiToolResult保持v2文本投影。
- ToolSelector只决定本轮可用名字；RunRequest.allowed_tools和最终宿主授权是不同关口。
- CheckpointSink由外部实现；内部Checkpoints只管理单Run序号、快照和确认，不管理数据库。
- ModelOptions/ProviderData经统一网关继承、检查、预算和审计。不同Provider的编码在providers或外部适配器处理。

无新常驻服务，无第二套Agent循环，无Mona类型。没有新增第三方依赖；更丰富的类型和测试带来源码增长，不以行数替代交付验证。

见[通用性判定](GENERIC-CORE-BOUNDARY.zh-CN.md)、[内容协议](CONTENT-AND-PROVIDERS.zh-CN.md)、[检查点语义](CHECKPOINTS.zh-CN.md)。

## 9. 持久会话的宿主装配

`packages/sessions` 持有本地 Store 和 SessionSink，保存线性会话、工作集与确认记录；生产依赖没有 Runtime 实现、Application 或 HTTP。`application/sessions` 是可选的产品调用适配，先保存用户轮次身份再调用现有 `start_task_with_history`。Server 的 `session_routes` 只处理部署目录、授权、路由和安全展示投影。一个会话有多个 Run，不增加执行循环；存储收尾包装器只等待结算。

文件采用版本化的原子完整快照，包含轻量头、回合范围与最新确认检查点。浏览器以分页读取安全历史视图，实时输出仍复用原有 Bridge。不同页面的修改用 revision 防冲突，同一工作空间用 OS 文件锁约束单写者。重启只恢复历史并标识中断，绝不自动重放未知工具。具体边界与测试见[本地会话](LOCAL-SESSIONS.zh-CN.md)。

## 10. API 7 上下文闭环

共享的消息配对校验在 `api::validate_messages`，Runtime 与会话恢复使用同一实现。规则通过可注入的 HistorySource 恢复完整历史中的目录作用域，不依赖具体会话后端。Skills 的根选择函数接收真实工作空间，环境解析留在产品层。

来源通过 `ContextTransform::sources` 返回有身份的 `ContextBlock`，Runtime 先收集和计入完整请求预算，再压缩真正历史并插入来源；Skills、Memory、项目规则不再混淆真实用户锚点。主对话用量计量仅保留匹配配置与消息前缀的哈希锚点，不使用摘要调用用量替代占用估算。

会话格式 2 分开完整档案和有界模型工作集。Compaction 的摘要/覆盖范围由原有同步检查点确认后持久保存，跨 Run 与重启复用；完整档案不会被摘要覆盖。`instructions` 扩展与 `sessions::references` 提供可装配来源，Spill 底层隔离不变，同会话跨 Run 读取由宿主验证结构化引用归属。接口变化与取舍见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)及[会话扩展](../packages/sessions/README.md)。
