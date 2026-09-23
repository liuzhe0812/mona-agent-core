# 最小 Rust Agent Harness：机制必要性与集成缺口审计

范围：API、Runtime、扩展能力、Application、桥接与 Web 宿主。本报告保留机制评估，并按用户确认的三部分架构修正建议；三个已复现缺陷的当前修复与证据见下文。其他建议不代表已经实施。

## 结论

不是所有现有机制都是最小 Agent 的必需能力。当前分层大体正确，可靠执行约束应保留；旧版本兼容、尚无真实消费者的插件依赖编排、重复工具开关和两套长输出存储值得精简。最值得补的是组合正确性，不是增加万能 context 包、强制 Planner 或更多 Agent 框架。

当前 Demo 没有用户和存量数据。废弃、错误和被替代的实现直接删除，不新增旧接口、旧配置或旧格式兼容；当前供应商适配、预算、取消、权限与数据完整性仍是必要机制。本次已删除被替代的工具输出存储，不宣称完成了全仓库其他兼容支路的清理。

## 1. 三部分职责

| 部分 | 职责 | 边界 |
|---|---|---|
| 核心层 | api + runtime，可靠执行及必要扩展时点 | 不绑定编程场景、存储、Web 或具体模型 |
| 扩展层 | 模型、工具、压缩、会话、归档、Skills、Memory、Planner 等 | 按生产场景装配；非 Core 必需不等于只能是样例 |
| 产品层（Web） | 组合根、配置、鉴权、交互与传输 | 不独占本应可复用的能力；Tauri/其他宿主可独立接入 |

编程、运维、办公是装配场景，不设置独立的“编程 Agent”架构层。多轮、长任务与持久化是验收要求，不能据此扩大 Core 或削弱扩展层。

runtime 的生产依赖只有 api 与基础 Rust 库，memory/planner 位于 dev-dependencies。根工作区 default-members 却包括 memory/planner 和 demo：这是构建入口/示例分发问题，不等于 Core 运行依赖已膨胀。依据：packages/runtime/Cargo.toml、根 Cargo.toml。

初次评估时的物理行统计（src/*.rs，含注释和内联测试，不含 tests/、apps、JS；不是修复后的规模）：api 1859、runtime 2295、providers 982、tools 2862、application 430、http-bridge 142、tauri-bridge 182、models 1208、compaction 939、spill 1349、skills 1073、memory 93、planner 106。它不是生产代码净行数，也不能直接与不同版本/语言的 Pi 或 DSH 比大小。

## 2. 当前调用关系

```text
Web UI
  ├─ /api/sessions ────────────── Server 会话管理、归档和存储
  └─ /v1/runs、事件、取消 ─────── HTTP Bridge
                                      │
                                AgentApplication
                                      │
                          SessionRuntime → ManagedRuntime
                                      │
                              唯一 Runtime/Engine
                 ┌────────────────────┼─────────────────────┐
           模型网关/预算          工具调度/校验/策略       上下文来源与投影
                 │                    │                     │
          Models Router          read/shell/edit/write   Skills、规则、Compaction
                 │                    │                     │
             Provider         结果变换/Spill          统一网关调用摘要
                 └────────────────────┼─────────────────────┘
                         await CheckpointSink → Server Store

直接嵌入：宿主 → Engine（不必经过 Application、HTTP 或 Web）
Tauri 接入：桌面宿主 → Tauri Bridge → 同一 Application/Runtime
```

这些包装器本身不是第二套 Agent 循环，但每一层都必须保持取消、完成、错误和绑定生命周期契约。

## 3. 机制必要性表

| 机制 | 判断 | 建议 |
|---|---|---|
| 唯一模型—工具循环、终止条件 | 执行必需 | 保留；不在 UI/插件里复制 |
| 工具调用与结果配对、完整流结束验证 | 可靠执行必需 | 保留，部分 JSON/截断结果绝不能进入工具函数 |
| 模型适配接口 | 执行必需 | 实现可替换，不要求依赖本仓库全部 Provider |
| 原始适配器与受预算网关分离 | 可靠执行必需 | 保留；摘要和工具辅助模型调用也必须计费/可取消 |
| 工具集中 JSON Schema 校验 | 可靠执行必需 | 保留；字段级、有界错误反馈，避免每工具重复实现 |
| 取消、总期限、超时、未知副作用状态 | 可靠执行必需 | 保留；所有 Runtime 包装器也遵循同一语义 |
| 任务调用预算、Run 步数、输入/结果硬上限 | 可靠执行必需 | 不同预算保护不同对象，不应全合成一个数值 |
| 并行工具与独占工具 | 并行是可选策略；安全结算必需 | 简单调度足够；独占目前仅限同一 Run 的批次 |
| 策略否决与宿主最终授权 | 使用有副作用工具时必需 | 保留，明确不等于 OS 沙箱 |
| 活动工具状态独立于展示缓存 | 可靠执行必需 | 保留，缓存淘汰不能让任务丢失终态 |
| 流式事件、序号、有限回放 | 交互宿主需要 | Runtime 产生语义事件；传输和展示保留策略可向 Application 收敛 |
| 同步确认 CheckpointSink | 持久任务需要；纯内存任务可不装配 | 保留确认屏障，存储算法不进 Core |
| 每次检查点完整序列化/复制 | 当前实现选择，不是正确性的必要形式 | 先测量，再优化共享快照或宿主日志；不改成不可靠异步旁路 |
| 完整请求审计 | 调试模式，不是所有任务必需 | 正式长任务建议默认 Metadata，Full 按需启用 |
| 原始档案与模型投影分离 | 长会话需要 | 保留；用户历史不能用摘要覆盖 |
| ContextBlock 来源标识与预留预算 | 启用上下文注入后需要 | 保留小型契约，不再扩张为万能 Hook 框架 |
| 压缩算法与可恢复摘要 | 长会话需要；不属于最小执行器必装件 | 保留在 compaction 与宿主状态，不内置摘要算法进 Runtime |
| 模型窗口和主请求用量估算 | 长会话需要 | 与实际模型绑定；未知时字节保护；不承诺精确分词 |
| Skills、项目规则 | 编程产品能力 | 可选来源，正文按需读取，不扩大工具权限 |
| Memory、Planner | 可复用扩展能力 | 按明确需求完善交付标准；不强制默认装配，也不因非 Core 必需而降级成样例 |
| Plugin 安装、关闭、失败清理 | 组件装配需要 | 保留薄装配，不强制每个对象插件化 |
| Services + provides/requires DAG + 手动 API 数字协商 | 需要核对实际扩展依赖 | 比较显式注入与依赖图的收益；不只因当前 Web 未使用就删除，也不为假想生态保留复杂机制 |
| 动态 ToolSelector 链 | 按需扩展接口 | 按已明确的动态工具场景判断必要性，不仅看当前 Web 消费者；不扩张成热加载框架 |
| enable_tools 与 allowed_tools=[] | 表达重叠 | 统一一种工具上限表达 |
| reasoning_content 与 opaque provider_data 双重回传 | 部分职责重叠 | 统一当前规范数据来源；仍须回传供应商当前协议要求的字段 |
| Application 幂等、回放、留存、订阅管理 | 多客户端产品需要 | 留在 Application，不下沉执行器；传输共享它 |
| HTTP/Tauri/auth/CORS/IPC ACK | 各自宿主的条件性需要 | 保持独立，没用的传输不强装配 |
| 旧会话格式恢复、旧 bash 配置名、旧接口别名 | Demo 阶段不需要 | 删除旧版支路，同步当前调用方、测试和文档 |

主要证据：packages/api/src/{plugin,run,context,event,model,protocol}.rs；packages/runtime/src/{host,engine,tools,model,events,checkpoint}.rs；packages/application/src/{service,subscription}.rs；packages/{memory,planner}/src/lib.rs。

## 4. 不应混为一谈的数据

| 数据 | 为什么存在 | 可以删除吗 |
|---|---|---|
| Run Transcript | 执行顺序和真实工具结果 | 不可以拿 UI 缓存代替 |
| 模型 Projection | 当次发送的摘要、近期内容和来源 | 不能反向覆盖事实历史 |
| 会话 Archive | 用户重启后查看的完整记录 | 已有持久会话需求下需要 |
| Checkpoint | 可确认的执行边界及未结算工具状态 | 可靠持久任务需要；不要求每次复制所有历史 |
| UI Snapshot / Event journal | 页面展示、掉线续订 | 产品需要，必须有界；不当数据库 |
| RequestAudit | 请求证据或调用元数据 | Metadata 通常足够，全文可选 |

“多种视图”有合理职责；“每种视图都频繁重新复制、序列化全部历史”不是必需。

## 三个缺陷的修复与验证

| 缺陷 | 当前修复 | 针对性验证 |
|---|---|---|
| ManagedRuntime.execute 丢弃 Future 后继续执行 | Engine、ManagedRuntime、SessionRuntime 共用 `RunHandle::wait_owned()`，删除重复守卫；start/passive wait 保持观察语义 | 真实 Engine 下仅取消该 Run，其他 Run 与共享 Task 不取消；未轮询拥有型 Future 也可取消，成功等待不误发取消 |
| 大输出在 Shell 与 Spill 间产生存储断层 | 删除 `packages/tools/src/output.rs` 及 `tool-output:` 执行路径；Tools 经注入接口把临时有界采集流式提交到共用 Spill 后端 | 约 40/60/120 KB 实际命令输出统一归档，重开存储完整读取头、中、尾；同会话续读，其他会话拒绝 |
| 同批改规则后仍按旧决定写文件 | 静态预检保留；动态策略在各工具 intent 确认后、实际派发前调用一次，不缓存整批决定 | 同批第二项旧写入被拒绝且未产生文件；新一轮看到新规则后重新发起才执行；拒绝可靠结算且只有一次终态事件 |

实现入口：`packages/api/src/streaming.rs`、`packages/runtime/src/tools.rs`、`packages/tools/src/{archive,capture,shell}.rs`、`packages/spill/src/{archive,local}.rs`、`apps/server/src/spill_setup.rs`。未新增包、Core 存储实现、执行循环或兼容支路。

归档后端对字符串与流只保留 `put_stream` 一套写入实现。固定缓冲验证完整 UTF-8、精确长度和配额，原子发布前不返回引用；取消、编码错误、长度不符与超限不发布不完整记录。临时采集没有索引或公开 URI，退出即清理；无归档或无读取能力时明确省略，不虚构可取回全文。

回归代码：`packages/api/tests/owned_wait.rs`、`packages/models/tests/cancellation.rs`、`packages/runtime/tests/policy_dispatch.rs`、`packages/tools/tests/shell.rs`、`packages/spill/tests/streaming_write.rs`、`apps/server/src/instructions_tests.rs`、`apps/server/src/spill_session_tests.rs`。慢 intent 保存导致的策略过期也有独立测试。

验证日志在 `.tmp-verify/three-defects-validation/`。`components.log` 覆盖 API、Runtime、Models、Tools、Spill；后续输出边界修正以 `tools-final.log` 为准。`server.log`、`minimal-server.log` 分别覆盖默认与无可选扩展宿主。`browser.log` 及 `.tmp-verify/context-browser-report/result.json` 来自正式 Web、真实 Rust Shell/Read、实际宿主重启；模型是受控本地端点，不是付费供应商或生产认证。

| 本次验证范围 | 结果 |
|---|---|
| API、Runtime、Models、Spill | 165 项通过 |
| Tools 最终版本 | 29 项通过，包含归档缺失、读取禁用、保存失败和小结果预算 |
| 默认 Server 单元测试 | 29 项通过 |
| Server 无默认扩展 | 21 项通过 |
| 正式 Web + 真实宿主进程重启 | 18 项断言通过，11 次本地模型请求、1 次摘要 |

已有长任务性能探针保持显式 ignored，未计入通过数。无默认扩展构建仍有未使用变量/方法的编译警告；测试无失败。测试只操作隔离工作区、状态目录及自己创建的进程。

这组修复不承诺跨 Run 的文件事务、任意 Shell 脚本规则分析、永久归档或无限输出；这些边界不通过增加 Core 框架解决。

## 核心层累计历史与动态工具预算修复

| 缺陷 | 当前实现 | 主要验证 |
|---|---|---|
| 运行中累计历史没有独立容量保护 | `RunLimits.max_history_bytes` 默认 8 MiB；计量规范 JSON，输入接纳、模型回复及工具结果均受约束。派发前预留结果空间，在途工具完成后释放多余预留；停止新派发而不是删除已确认事实 | 无检查点／缩小投影仍会命中历史上限；控制字符、私有模型字段均计量；注入失败不确认；并行限流与取消都保持调用配对和唯一终态 |
| 动态筛选前按所有工具计量 | 每轮先基于已结算正式历史选择，再收集来源和压缩。已选 Schema 在计量、发送、检查点和执行之间一致；重试／溢出恢复不重新选择 | 未选大 Schema 不再触发不必要压缩；已知窗口与纯字节模式均使用真实 Compaction 验证；来源只计一次；重试不重跑工具 |

容量是规范历史大小，不是进程 RSS。未知工具结果按当前结果上限的保守序列化边界预留，因此临界容量可能降低实际并行度；不改变工具允许结果上限，不让已经开始的副作用因腾空间被取消。超大的模型回复不进入正式历史也不派发工具，模型调用仍计费与审计。实际摘要算法、数据库、输出归档不进入核心层。当前契约见 [API](../packages/api/README.md)、[Runtime](../packages/runtime/README.md) 和 ADR-054。

| 本轮最终验证范围 | 结果 |
|---|---|
| API、Runtime | 142 项通过；既有成本探针 1 项 ignored，未计入 |
| Compaction | 17 项通过 |
| Application、Providers、Models、Tools、Skills、Spill | 125 项通过 |
| 默认 Server（单元与接口） | 35 项通过 |
| 无默认扩展 Server | 21 项通过 |

日志在 `.tmp-verify/core-boundaries/`。`final-tests.log` 保留了其他包通过与 Skills 原测试失败：原夹具没有注册 read，却断言模型应收到提示使用 read 的目录。该夹具已改为验证“未注册、已提供、动态排除 read”三条路径；`extensions-final.log` 为 Skills/Spill/Tools 最终通过记录，`no-default-tests.log` 为无默认扩展通过记录。已检查最终生产源码哈希未变；没有为了覆盖旧日志重复运行未受影响的通过项。无默认扩展宿主仍有既有未使用项编译警告。

新增 15 个专项测试，复用现有可靠提交、取消、>256 工具及受控 HTTP/真实 Rust 工具集成回归。本轮没有浏览器交互、付费供应商或生产环境验收，不用之前的浏览器结果代替本轮测试。

## 6. 源码可见但未在本轮专项复现的边界

### 同一工作空间的多个 Run

Runtime 默认允许32个 Run，ToolConcurrency::Exclusive 仅对单批次有效；edit/write 的单文件互斥不涵盖 shell 或其他进程。同一会话不能同时发两轮，不代表同一工作空间的两个会话不会冲突。建议正式编码宿主先实现简单明确的工作空间写协调，不立即引入多 Agent 调度系统或 worktree 管理。

### 切换供应商与私有协议历史

models 的命名空间按供应商隔离，Provider 遇到外来 provider_data 会正确拒绝。但同一会话下一轮采用新的默认供应商时，没有明确的历史转换/分支策略。含私有回传字段的历史可能被拒绝。建议启动前明确校验并提示新会话或提供明示、安全的协议转换；不可静默丢弃签名/私有状态，更不能发送给别的供应商。这是当前外部协议适配问题，不是旧版本兼容。

依据：packages/models/src/lib.rs:279-294、644-655；packages/providers/src/chat.rs:198-214。

### 检查点与宿主档案的真实成本

runtime/checkpoint.rs:36-57 每个提交序列化并复制完整检查点；宿主 store.rs:212-274 读取/校验并原子重写完整文件，history() 还对档案和工作前缀散列。Store 的 gate 是工作空间级互斥。当前模型请求缩小不意味着本地存储已变成常量成本。

建议分别测量请求构造、序列化、文件写入与锁等待。仍可保留同步确认，而优化不可变共享快照或宿主追加日志。不要把可靠保存改成 fire-and-forget，也不要只凭行数直接换数据库。

### 摘要质量与默认工作指导

compaction 的摘要提示是有限的自由文本，硬上限4096字节，安全配对/哈希验证不能证明摘要保留了所有关键约束。需要连续压缩后仍能遵守用户纠正、路径、已完成动作、未完成事项的真实场景评测，不是增加每轮必须调用的反思模型。

正式宿主使用 ApplicationConfig::default，system_prompt 为 None。编码等具体场景需要默认工作指导时，应随场景能力装配，不固定到通用 Web 或 Runtime，也不强制规划、反思等执行流程。

依据：packages/compaction/src/lib.rs:230、529-623；packages/application/src/service.rs:26-32；apps/server/src/main.rs:164。

### 多模态与可观察性

当前旧图片组默认不可摘要；大量图片和无法再缩小的完整调用组仍可能明确超限。这是能力边界，不是理由去拆散调用/结果。按实际视觉任务需求补近期图片保留/旧图引用策略即可。

RunEvent 目前主要是步骤、展示项和结果，没有精简的重试等待、压缩前后大小、限制命中原因事件。建议提供安全元数据用于宿主说明“正在压缩/正在等待”，而不是展示隐藏推理或引入全套遥测平台。

## 7. Pi / DSH 对照与取舍

本次使用其公开仓库源码/文档；不是相同硬件、模型和工具的性能基准。DSH 明确处于 developer preview，不能把所有上游设计都当成生产认证。

| 方面 | Pi | DSH | 我们的取舍 |
|---|---|---|---|
| 底座 | agent-core 执行与事件，上层 coding-agent 负责完整编码体验 | Cordis 插件树，连 loop、session 和 registry 都可替换 | 借鉴分工，不复制 everything-is-a-plugin 的全部装配框架 |
| 上下文 | transformContext / convertToLlm；上层压缩和会话组织 | token-meter、compaction、上下文来源分工 | 现有来源/投影/档案分离保留，无需万能 context 包 |
| 工具调度 | 当前文档支持并行/顺序；结果保留助手调用顺序 | tools 与 policy/execute seam | 保留配对、有界并行、明确副作用；补宿主级写协调 |
| 会话 | 编码应用保存树形JSONL，支持继续/分支；SQLite后端亦独立 | 事实日志、持久后端、投影与检查点策略分开 | 线性会话足够，不为追平增加分支树/事件框架 |
| 摘要 | Goal、Constraints、Progress、Decisions 等结构，并累计文件信息 | 压力计量与压缩策略独立，原事实仍在日志 | 加强摘要可用性与评测，不强制更多模型调用 |
| Planner/Multi-Agent | 不作为默认强制模式 | 可通过插件组合 | 可选，不能作为最小底座缺陷来补 |
| 权限 | 文档不内置每工具确认弹窗，建议容器或扩展流程 | 基础组合包含 sandbox/approval 能力 | 工具授权不等于沙箱；本地信任域清楚，远程/不可信任务需宿主隔离 |

上游来源：
- Pi Agent API：[README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md)
- Pi 编码应用及设计取舍：[README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md)
- Pi 压缩：[文档](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md)
- Pi 默认指导：[system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts)
- DSH：[Architecture](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/architecture.md)
- DSH：[Token meter](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/llm/token-meter/README.md)
- DSH：[Compaction basic](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/compaction/compaction-basic/README.md)
- DSH：[Checkpoint policy](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/session/session-checkpoint-policy/README.md)

## 8. 推荐收敛顺序

1. 删除开发阶段不需要的旧版读写/配置兼容支路，保留格式拒绝与数据保护。
2. 三个已复现问题已修复；工作空间写协调仍按实际场景需求评估，不作为 Core 的全局锁。
3. 合并重复开关和协议回传载体；插件机制按当前需求和明确扩展用途判断。Memory/Planner按扩展交付标准评估，不因非 Core 必需降级。
4. 默认长任务精简审计，测量本地检查点/档案成本；完善安全的错误与进度解释。
5. 在真实任务上评估摘要质量、模型切换和视觉上下文边界。MCP、向量库、强制规划、多Agent、热加载、插件市场、跨设备同步不作为本轮最小性必补项。

已实现范围包括“三个缺陷的修复与验证”和“核心层累计历史与动态工具预算修复”，其余条目仍为评估建议；原始探针与历史验证不能代替当前回归。
