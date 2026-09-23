# 上下文管理：有界工作集、来源与持久会话

## 已实现的职责划分

没有新增万能 `context` 包或第二套 Agent 循环。`api` 定义小型上下文契约，Runtime 控制收集、压缩、请求、校验与有限恢复的时序；`models/providers` 提供实际模型窗口；`compaction` 拥有摘要算法与可序列化状态；`sessions` 扩展保存完整历史、工作集并校验归档归属；`instructions` 扩展发现和刷新规则；Server 只提供装配、配置及产品授权。Skills、Memory 仍是各自独立的上下文来源。

当前 Rust API 为 9、HTTP/Tauri Stream 为 2、RunCheckpoint 为 1。会话仅支持当前格式 2，旧格式读取与迁移已删除。详见[会话扩展](../packages/sessions/README.md)和[Web 本地会话](LOCAL-SESSIONS.zh-CN.md)。

## 1. 模型窗口与计量

模型设置页可以为每个供应商下的每个模型填写 `context_window_tokens`。未知保持空值；接受 1–1,000,000,000 的整数，不把通用 `/models` 列表臆测为容量目录。编辑、发现模型和显示开关会保留已配置值。未指定容量仍按未知处理，不猜测窗口。

新 Run 绑定模型时连同窗口一并固定，修改设置只影响随后启动的 Run。已有 Provider 和 Router 窗口契约继续使用，不把供应商名称当作统一窗口，也不让切换模型借用另一模型容量。摘要继承同一实际路由，但使用自己的输出预留量。

计量保留字节硬限制。已知窗口时，默认按扣除输出空间后的 80% 触发压缩、目标 60%；保护内容无法放入时明确失败。Gateway 只记录主对话成功响应的输入用量锚点，使用请求配置、工具声明和逐消息 SHA-256 检查相同请求前缀；后续尾部仍按序列化字节近似估算。路由、设置、工具或既有消息发生变化时不复用锚点，回退字节估算。

摘要等辅助调用计入共享用量和审计，但不能替换主对话占用锚点。锚点不保存请求正文，精简审计仍不保留正文。它是估算，不是精确 tokenizer、供应商账单或模型一定接纳请求的保证；供应商确认溢出仍只允许一次严格缩小后的恢复。

## 2. 来源收集先于压缩

每轮先通过 `ToolSelector` 从已结算正式历史确定工具集合，再调用 `ContextTransform::sources` 收集有来源名称的 `ContextBlock`，不向正式 Transcript 追加伪用户输入。Schema 成本按本轮实际工具计算；来源校验名称、数量和体积并计入同一预算。随后压缩真正的历史，最后在开头 System 消息之后插入来源。选择器每轮仅一次，来源、压缩、发送和执行共享该集合。

因此 Skills 目录、项目规则和 Memory 检索不会被当成“最后一个真实用户请求”，也不会在压缩完成后才意外消耗未预留的空间。一次溢出恢复复用这一轮已选工具及已收集来源，不重新选择工具或扫描出不同版本后拼接同一回答，也不重复计算来源开销。来源作为低于宿主系统约束和当前用户要求的参考/指导，不授予工具权限。

上限为每轮 128 个唯一来源；单来源内容最多 128 KiB，source 标识最多 512 字节。各组件仍有更严格的自身限制。来源总量放不下时，在模型调用前明确失败。没有来源组件的最小 Runtime 保持原行为。

## 3. 完整历史与模型工作集分开

会话档案保存用户输入、助手回复及已确认工具结果。模型工作集可以是摘要加近期完整消息。下一轮只把经过验证的工作集交给 Runtime，旧档案不再反复作为每次 Run 的输入。

核心层以独立的 `max_history_bytes`（默认 8 MiB）限制当前 Run 累计历史；摘要不释放这份原始记录的预算。超限停止接纳或新工具派发，保留已确认事实和完整调用配对；不安装压缩／检查点也生效。会话档案、模型请求和当前 Run 历史是不同对象，限额不能互相代替。详见 [Runtime](../packages/runtime/README.md#历史容量与结果结算)。

`CompactionState` 保存格式号、摘要和被覆盖的整组消息范围及 SHA-256。恢复会验证覆盖范围、原文指纹和工具调用/结果边界；任意用户文本即使伪装成摘要标记，也不会成为可信替换状态。恢复状态只投影对应的工作历史，不重写完整档案。

宿主在原有可等待检查点里捕获这份状态；每 Run 的临时摘要缓存仍在 `finish` 清理。下一轮先应用已确认状态，再保存该轮工作前缀的长度与哈希、此前档案的哈希。随后将本轮检查点中新产生的消息接回完整档案。摘要范围、档案与检查点同文件原子提交，不另开异步保存支路。

这允许完整历史超过默认 4 MiB 接纳上限时，仍从较小的已确认工作集续聊；没有放大或取消 Runtime 的保护上限。会话文件仍有 32 MiB 总上限、512 轮限制，检查点仍有独立上限。关闭压缩时使用完整历史，过大的历史会明确拒绝。没有可验证压缩状态且工作输入本身超过保护上限时，不会静默裁切原文或在网关外调用模型。

进程重启只恢复已确认记录和工作集，不自动重跑模型或工具。中断工具沿用 Pending→Skipped、已确认 intent 但结果未知→Unknown、已确认结果→原结果的规则。未确认的流式 token 不被伪造成完整回答。

### 结构化任务摘要

摘要包含 goal、constraints、corrections、decisions、completed、pending、references。模型合并旧交接记录和新增事实，保留用户禁令、纠正、确定动作、未知副作用及待办；计划或工具 intent 不能改写成完成。格式、大小和覆盖范围可校验，但语义准确性仍需部署模型评测。

默认摘要上限 16 KiB，实际可用空间还受本轮窗口、来源、近期消息和输出预留约束；不能把 16 KiB 当每轮固定分配。缺字段、格式错误、截断、超预算和无缩小进展均明确失败，不裁切 JSON、不另开修复模型。取消/失败不发布错误状态，也不淘汰其他活动 Run 的摘要。原子保存和原始档案保持原语义。

当前 CompactionState 仅接受格式 2 的有效 TaskSummary，不提供旧摘要迁移。带私有 reasoning/signature 的完整消息或工具组不参加摘要，不能用压缩绕过模型切换检查；大量受保护私有历史仍可能明确超限。整体不可分组放不进摘要请求时也会失败，不通过拆散工具调用/结果解决。

## 4. 同会话历史归档

Spill 底层仍按 Run 隔离。宿主的 `read` 适配调用 `sessions::Store::artifact_owner`，验证当前 Run 属于该会话后，从确认过的结构化 ArtifactRef 查找最初拥有文件的 Run；用户正文里的 URI、模型摘要里的字符串和另一会话的引用均不授予访问权限。重复读取产生的相同引用不会覆盖原文件归属。

`session.artifacts` 来源提供该会话已确认引用的小型目录，帮助摘要后的模型找到准确引用；目录不是授权凭据，读取时仍独立验证。目录最多 64 KiB；模型只得到不透明标识，不得到任意本机路径能力。

Spill 仍按自己的保留策略清理，默认 24 小时；不是永久附件库。文件已过期、删除或不在授权范围时给出明确不可用结果，不恢复一个看似成功的空文本。浏览器查看旧轮次依然使用旧 Run 对应的有鉴权分页接口。Shell 通过宿主注入的流式输出接口共用 Spill 后端；Tools 内不再有 `tool-output:` 存储。只有完整归档发布后才返回 URI，跨轮与重启读取不再取决于输出是否超过旧的 50 KiB 预览阈值。

## 5. 项目规则

当前 Web 宿主装配 `instructions` 组件，默认开启，可在“设置 → Agent 组件 → 项目规则”关闭并在下次启动生效；部署者可用 `AGENT_INSTRUCTIONS=0` 锁定关闭。部署锁定后该项不会显示为用户设置。Runtime 本身不读取文件，未装配此组件的嵌入方式不受影响。

规则从宿主指定工作空间根开始。每个目录优先 `AGENTS.md`，不存在时使用 `CLAUDE.md`；不会递归扫描整个项目，也不读取工作空间之外的用户全局规则。结构化 `read/write/edit/grep/find/ls` 路径使对应目录及祖先规则变得相关。规则扩展通过宿主注入的 `HistorySource` 恢复完整档案中已触达的目录，不依赖摘要保留每个文件名；它不依赖具体会话后端。

每次构建模型请求重新读取适用规则，显示来源和目录作用域，更新与删除不会永久滞留。若写文件等副作用工具首次进入带有新规则的目录，或模型回答之后规则改变，该次工具会返回“未派发、先依据新规则重新判断”；下一轮请求带上新规则，只有模型重新发起调用后才执行。既有权限关口仍可否决。动态策略在每个工具 intent 确认后、实际派发前检查一次，因此同批先修改规则再写文件也会拒绝旧决定；不能把 intent 记录当成工具已实际执行。

规则为 UTF-8，单文件及总原文不超过 64 KiB，单来源编码后不超过 128 KiB；每 Run 最多记录 64 个相关目录及 128 个祖先作用域。规则文件不是普通文件、是符号链接/Windows reparse point、不可读、过大或编码损坏时明确失败，不悄悄忽略后继续写入。取消后不会重新创建已清理的来源缓存。

这是结构化文件操作的项目指导，不是 shell 命令语义分析器或文件系统沙箱。任意 shell 中隐藏的跨目录访问不会被自动推断，模型应先读取适用规则；同用户进程修改文件的所有竞态也不构成安全隔离承诺。

## 参考范围

参考 Pi 的[压缩与上下文重建](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/compaction.md)：保存摘要及覆盖边界，重建摘要加保留消息；不移植分支树或其文件格式。

参考 DSH 的[上下文计量](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/llm/token-meter/README.md)、[压缩契约](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/compaction/compaction/README.md)与[项目规则](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/context/agent-instructions/README.md)：区分计量、策略、持久替换和来源刷新。本项目独立实现，保持既有同步完整检查点；不会因为上游允许裁剪规则文件，就在本项目静默忽略指导。上游文档用于概念与边界参考，不证明本项目的生产验收。

## 验证入口

```sh
cargo test --offline -p sessions --features compaction
cargo test --offline -p instructions -p skills
cargo test --offline -p compaction -p runtime
cargo test --offline -p server --bin server
cargo test --offline -p server --no-default-features --bin server
npm run test:web
cargo build --offline -p server --target-dir target/context-validation
node apps/web/test/context-e2e.mjs
```

浏览器测试使用隔离的空状态目录、随机端口、受控 HTTP/SSE 模型、真实 Rust shell/read/Spill，以及自己创建并重启的宿主。模型设置经过正式页面保存；不使用开发者 .env、不调用付费供应商、不停止开发者正在使用的服务。`MONA_TEST_SERVER` 可指定其他隔离二进制，`BROWSER_BIN` 可指定浏览器。结果默认保存在忽略目录 `target/browser-reports/context`，可通过 `MONA_TEST_REPORT_DIR` 指定。

测试包含：固定模型窗口下主动压缩、同一摘要跨 Run/重启复用、旧格式拒绝、无 Web 的扩展组合、配置工作空间与服务启动目录不同、超过 Core 原始接纳上限的历史档案、篡改状态拒绝、同会话与跨会话归档读取、来源注册顺序、辅助用量隔离、规则变更与新目录写前拦截。脚本端点验证实际调用链，不等同于真实供应商联调、摘要语义质量保证、断电耐久性认证或无限长会话性能。
