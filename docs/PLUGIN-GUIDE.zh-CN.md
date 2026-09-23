# 插件开发规范 v0.3

## 1. 开发原则

Plugin 是组件接入 Runtime 的一种方式，不是所有组件的必需形态。组件应先拥有可复用的普通 API；需要运行时注册或统一生命周期时，再提供薄 Plugin 入口。Plugin 生产依赖 `api`，不要导入 Runtime 的私有结构。示例可参考 `packages/memory` 与 `packages/planner`；两者均没有第二套执行循环。

当前 Plugin 是可信 Rust crate，随宿主编译，通过 `HostBuilder.plugin(Arc<dyn Plugin>)` 注册。没有 npm/Python 包装，没有动态库、热加载或下载市场。

## 2. 安装协议

`Plugin` 包含三个方法：`manifest`、异步 `install`、异步 `shutdown`。

Manifest 指定 id、api_version、requires、provides。requires/provides 是服务键，不是类名和文件路径。当前公共 API 协议号为 9；它与包版本 0.3.0 不是同一个值。包在 1.0 前仍可能调整 Rust 接口。

安装流程：

1. 所有声明做唯一性、版本和依赖检查。
2. 在开始安装前识别缺失依赖及依赖环。
3. 按依赖顺序安装；同样就绪的插件保持传入顺序。
4. 每个插件在独立暂存注册表中安装。
5. 实际发布的服务必须与 provides 声明一致；工具、变换器等不是 requires/provides 中的服务键。
6. 该插件成功后才提交其注册；失败时不发布半成品。
7. 失败插件自身也会被调用 shutdown，随后逆序关闭之前成功的插件。

宿主启动完成后注册表不再变化。shutdown 先禁止新 Run、取消并排空已有 Run，才撤销注册和关闭插件。

安装时创建的外部资源由插件负责在 shutdown 中关闭；不要创建无法追踪或无法停止的后台工作。

## 3. 可注册的能力

| Registrar 方法 | 对应接口 | 语义 |
|---|---|---|
| `publish` | `ServiceRegistration` | 发布类型检查的服务 |
| `tool` | `Tool` | 注册并冻结工具与 Schema |
| `context_transform` | `ContextTransform` | 先收集有来源的 `sources` 并计入预算，再串行生成真实历史的模型投影 |
| `result_transform` | `ResultTransform` | 工具结束后处理输出，不得篡改状态 |
| `policy` | `ToolPolicy` | 工具执行前的权限否决 |
| `observer` | `EventObserver` | 非关键实时观察 |
| `tool_selector` | `ToolSelector` | 在Run上限内选择本轮工具，同轮只能继续收紧 |
| `checkpoint_sink` | `CheckpointSink` | 安装一个可等待的执行记录提交实现，不是UI观察器 |

API 7 的目录、规则、记忆等注入组件应通过 `ContextTransform::sources` 返回唯一来源的 `ContextBlock`，普通 `transform` 处理真实对话。不要两处重复注入，或把来源伪装为最后一个用户请求。压缩策略仍属于独立组件，执行器只处理预算、时序与校验。计量、来源与恢复契约见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)。

会话、规则和 Skills 的独立接入分别见 [sessions](../packages/sessions/README.md)、[instructions](../packages/instructions/README.md)、[skills](../packages/skills/README.md)。普通接口先独立可用；会话 Sink 与运行收尾不依赖 Web，规则 Plugin 只注册同一实例的来源和派发策略。

### 模型服务

通过服务键 `agent.model` 发布 `ModelProvider(Arc<dyn Model>)`，即可不通过 HostBuilder.model 提供模型。两种配置方式不能同时声明同一个服务。

运行时服务视图会隐藏原始模型适配器；辅助模型调用使用 `RunContext.model`。不要私建 HTTP 客户端绕过任务预算。

`Model::validate_history` 提供纯协议预检，不进行网络或模型调用、不改写历史。`AgentRuntime` 包装器应转发该入口，实际 start 对绑定模型再检查，不能把先前预检当作配置预留。模型私有回传不兼容返回 `ModelHistoryIncompatible`；不自动重试、删除字段或新建会话。标准会话适配在持久接纳前调用，具体路由/签名规则只在 Providers/Models，不进入 Runtime。

### 自定义服务

`ServiceRegistration::new` 发布 `Arc<T>`。消费者使用 `Services.get::<T>(key)`，若键不存在或类型不匹配就返回错误。服务键相同不代表 Rust 类型一定相同。

如需暴露 trait object，可用一个具体 wrapper，例如示例中的 `MemoryService`。不要让消费者 downcast Core 内部类。

## 4. 工具规范

ToolSpec 描述名称、用途、参数 Schema、并发模式与 side_effects。工具名长度 1..64，限 ASCII 字母、数字、下划线和连字符；本版参数根类型必须为 object。

参数 Schema 使用 Draft 7，注册时编译，仅允许文档内 `$ref`。不要在 spec() 中做网络请求。

工具收到 ToolContext，其中有 run_id、call_id、取消令牌、共享任务控制、受预算保护的模型网关和进度通道。

工具必须：

- 在异步执行中让出调度权，不直接进行长时间 CPU／同步 IO 阻塞。
- 监听取消；拥有并清理自己创建的子进程和资源。
- 返回最终结果，不依赖 progress 消息作为唯一数据出口。
- 准确声明副作用及并行安全性。
- 对写操作自行设计幂等与“结果未知后的核查”方式。

side_effects 不是沙箱权限推导，Core 信任插件的声明。宿主默认拒绝副作用工具；注册工具不等于授权工具。

核心层的 `max_history_bytes` 独立约束累计历史。工具派发前预留其允许最大结果的序列化上界；预算压力可能降低实际并行度或使未启动项 Skipped，但不删减已确认的执行记录。已启动工具仍按现有结果上限、取消和期限结算；详情见 [Runtime](../packages/runtime/README.md#历史容量与结果结算)。

## 5. 上下文变换

输入是已拥有的一份消息投影；返回新的投影。可以检索、注入或裁剪，但不能返回未配对的工具消息。

`ToolSelector` 先从已结算正式历史选出本轮工具，随后才调用来源与投影。此时 `RunContext.request_tools` 与 `allowed_tools` 是本轮已选集合，请求开销包含该集合和已收集来源，不再预留所有注册工具的 Schema。`model_request` 构造与主请求一致的计量对象；网关仍对实际序列化请求执行硬限制。选择期间以 `available` 为候选集合。上下文工作仍受 `context_timeout`、任务期限及取消控制；错误恢复复用同轮声明与来源。

`recover_context` 默认返回 `None`。只有能在供应商确认上下文超限后安全缩小投影的组件才应覆盖；Runtime 最多调用一次，并要求结果严格变小且保持消息/工具配对。该入口不能执行工具、放宽权限或启动另一套 Agent 循环。

不要通过保存 ContextTransform 的私有全局变量把不同用户的运行混在一起。需要共享数据时明确放到 Host/Agent 级后端；当前任务临时信息放在 RunContext 或调用栈中。

消息会在每次变换后校验，最终规范化请求会进入 RequestAudit。该操作失败会停止当前运行，不会“悄悄忽略记忆检索异常后假装结果完整”。

## 6. 结果变换

`ContextTransform::finish(run_id)` 与 `ResultTransform::finish(run_id)` 用于释放单个 Run 的资源，默认空实现。执行器在工具结算之后、终态检查点及报告发布之前等待它们；每项使用独立于任务取消的 `hook_timeout`。一个收尾失败不阻止其余收尾，错误进入最终报告，不重做任何工具。缓存、活动归档等可靠收尾不能只依赖 `EventObserver`；直接调用变换器的宿主应遵守同样的调用顺序。

ResultTransform 可以对长工具输出归档、筛选、压缩或增加 ArtifactRef。它不能改变 call_id、ToolStatus；原始长度和已截断标记也不能被悄悄降低。

如果采用外部存储，还需要提供读取工具和访问权限。ArtifactRef 只是协议字段，Core 不会自动把 URI 当作可访问网页，也不会自动把省略原文保存在磁盘。

变换失败后保留已知的执行结果，运行以失败状态结束。不能因为结果后处理失败，就重复执行原工具。

## 7. 权限否决

ToolPolicy 返回 Allow 或 Deny，多个策略共同收紧。静态参数与不可变宿主授权先预检；动态策略在该工具 intent 确认之后、实际派发之前检查一次。前一个独占工具修改规则后，同批后续调用也必须使用当前状态。拒绝和策略异常不会进入对应工具函数；异常停止尚未派发的队列，不撤销之前已完成的动作。

ToolPolicy 不修改参数，也不能放宽宿主授权。检查、取消与结果结算沿用现有预算和确认屏障；它不是跨 Run 或外部文件系统的原子事务。

需要人工审批时，在该接口等待业务审批结果并响应取消。审批 UI、用户身份鉴别、审计落库都属于宿主应用；本版没有默认审批服务器。

## 8. 事件观察

EventObserver 是 best-effort：可能 lag，也可能在关闭时被取消。on_event/on_lagged 的错误不影响运行结果。

不要把账单扣款、必须成功的记忆保存、事务提交放进 EventObserver。执行快照可使用本版可选CheckpointSink；其他业务事务仍需专用的应用协议，而不是提高观察器“优先级”。

宿主关闭会等待或终止本版创建的观察任务，以免插件资源关闭后还有 Core 管理的回调继续执行。插件私建的未管理任务不在这一保证之内。

## 9. Memory 示例

MemoryBackend 是插件自己的扩展接口，可替换为数据库实现。示例 InMemoryStore 有容量上限；检索是简单键匹配／最近条目，不是 embedding 语义检索。

ContextTransform 把记忆作为低信任参考加入投影；memory_remember 是需要显式宿主授权的写工具。没有自动持久化，也不会自动保存每一段对话。

## 10. Planner 示例

PlannerPlugin 发布 planner.service。调用者取出 Planner 后把同一个 AgentExecutor 传给它：先运行一个禁用工具的规划任务，再顺序执行步骤。

这种组合不要求 Core 知道 Planner 类型，也没有复制工具注册、模型适配、取消或 ReAct 代码。每个子运行共享 TaskControl，因此预算不会重置。

新 Planner 可以替换上层编排器，不应通过 ContextTransform 的副作用私自启动无法追踪的子运行。

## 11. 协议演进

优先新增可选字段和明确能力声明；修改已有语义需同步 API_VERSION、测试和文档。服务键要带明确领域含义，不能全部命名为 `manager`。

新增一种扩展需求前先回答：它是执行能力、输入投影、输出治理、权限、观察，还是跨任务编排？如果都不属于，先记录 ADR，再修改 Core，而不是增加一个可以改一切的 Hook。


## 12. v0.2 流式迁移与工具UI详情

旧事件消费者要迁移到带item_id的生命周期协议。EventObserver仍是best-effort，不能承担可靠业务提交；统一Application会另外持有UI快照与有界回放。

ToolProgress.report发送纯文本日志；set_detail(key,value)可增加当前工具项的有界结构化显示信息。键应有命名空间（如coding.diff），最多8个，每个JSON<=16KiB。不能改变call_id、状态或给别的运行发事件；也不能把详情当成任意执行指令。

工具最终返回值仍是正式结果。UI详情、日志丢失或截断不应改变工具执行语义。工具产生的HTML/路径/URL都应按不可信数据处理。

Memory/Planner示例生产依赖仍只有API，不需要HTTP或Tauri依赖。Planner保持最终结果接口，复合规划流不是本次默认实现；不要在两个bridge里各自实现一遍Planner适配。


## 13. v0.3 工具输出、选择和检查点

Tool.execute返回Result<ToolOutput>；纯文本可用.into()包装。is_error表达已知业务错误，不能让Runtime把“错误字符串”当成功。多模态与structured可作为模型观察，UI采用独立预览；详情显式发布且必须脱敏。

ToolSelector 拿到只读 RunContext、step、已结算正式历史和 available，在来源收集与压缩之前执行，每轮一次。不能增加未注册或被前一选择器排除的工具；重复名、异常和超时停止执行。该接口不发布工具、不修改 Schema、不替代权限；不得依赖压缩结果反复选取。

CheckpointSink在Host级安装、按Run并发隔离，由Core等待每次commit；必须支持取消、幂等和自己的耐久性承诺。它不接收可修改Runtime，不调用工具或开启另一个Agent循环。详见[检查点协议](CHECKPOINTS.zh-CN.md)。

示例generic_extensions通过普通Plugin/Registrar安装选择器与内存sink，无需导入Core私有结构。Planner的PlanRequest新字段把工具上限与模型选项传给每个子运行，不能默认扩权。
