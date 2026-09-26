# 本地会话保存与继续对话

## 使用效果

正式 Web 宿主默认保存会话。仍使用 `npm run dev:web`，不需要数据库服务、额外 npm 安装或手工创建 `.env`。第一次发送时创建会话，左侧可以搜索名称、打开历史、重命名和删除；刷新页面或重启同一宿主、同一工作空间后，可查看已保存的用户消息、助手回复、工具调用和结算结果，并继续发送下一轮。URL 片段只保存会话 ID，不保存内容或凭据。

## 参考与取舍

参考 Pi 的[会话文件与工作目录组织](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)：会话有持久身份，历史选择与当前执行分开。参考 DSH 的[持久化后端](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/session/session-persistence-jsonl/README.md)和[检查点策略](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/session/session-checkpoint-policy/README.md)：保存实现与执行时序分工，关键确认失败不能继续模型或工具调用。

这是独立实现，不兼容或导入两者的会话文件。Pi 的树形分支、DSH 的完整事件溯源与压缩日志没有搬入本项目。当前已有可靠 `CheckpointSink`，因此首版复用完整检查点，不建立第二套运行日志或执行循环，也不把它改成异步旁路。

## 职责与调用路径

```text
Web 会话列表 / 继续对话
  -> apps/server 的 /api/sessions 管理接口
  -> application::sessions::SessionApplication
  -> sessions::Store 校验 revision / 请求身份，先保存输入并生成可信工作历史
  -> AgentApplication.start_task_with_history
  -> 原有模型管理包装器与唯一 Agent Runtime
  -> 原有 CheckpointSink 确认点
  -> sessions 扩展的本地文件存储

实时展示仍通过 /v1/runs/{run_id}/events 等原有 Bridge 接口。
历史展示从持久记录派生浏览器安全的 RunSnapshot，不把 UI 缓存当正式历史。
```

[sessions](../packages/sessions/README.md) 管理文件、单写者锁、轮次去重、工作集、检查点与运行收尾，生产依赖不包含 Web、Application 或 Runtime 实现。[instructions](../packages/instructions/README.md) 管理规则发现与检查，可注入可信历史来源。`apps/server/src/session_routes` 仅派生安全历史视图并提供鉴权管理接口。Application 的可选 `sessions` 适配复用现有运行注册表，不复制存储。HTTP/Tauri Stream v2 与 Checkpoint v1 不变。

会话包含多轮；每次新用户轮次对应一个新的 Run，使用当前宿主的模型选择、系统提示词、工具权限和新 Run 预算。历史不会恢复旧权限或旧密钥。旧 System 消息仍留在原检查点，但不覆盖当前宿主指令。历史接纳和压缩继续经过既有 Runtime 边界；超限明确失败，绝不静默丢弃早期对话。

## 本地位置与格式

默认位置：Windows `%LOCALAPPDATA%/mona-agent-core/sessions`；Unix `$XDG_STATE_HOME/mona-agent-core/sessions`，未设置时为 `$HOME/.local/state/mona-agent-core/sessions`。可通过可信宿主环境变量 `AGENT_SESSIONS_DIR` 覆盖。开发启动器的 `MONA_DEV_STATE_DIR` 同时隔离模型设置、能力状态、Spill 和会话；显式 `AGENT_SESSIONS_DIR` 优先。独立 `server --demo` 的默认目录为 `demo-sessions`。

```text
sessions/
  writer.lock
  s-<创建请求ID>.jsonl
```

每个文件严格包含两行 JSON。只支持当前格式（版本定义见[文档首页](README.zh-CN.md#协议与数据版本)），保存身份、不可变实际工作目录、通用有界组织 metadata、revision、导航标记、回合元数据、完整档案、工作前缀、最新检查点与可验证摘要。工作目录不是 Store 分组键，更换默认根不会隐藏当前格式历史。普通会话的独立工作目录与项目会话的共享目录规则见[应用层工作区接口](architecture/WEB.zh-CN.md)。Store 只扫描当前目录下的 `*.jsonl` 会话文件；其他目录不参与会话加载，不提供旧格式迁移或兼容逻辑。完整档案不被摘要覆盖，新 Run 使用验证后的工作集，见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)。

采用同目录临时文件、完整写入、文件同步、原子替换；Unix 还同步目录。读者只读取一代完整文件，不单独更新索引与正文。列表只读取有界首行，正文按会话、回合分页查询。每个宿主会话状态目录一个 OS 文件锁：同一存储目录的第二个写宿主启动失败，进程退出或崩溃后锁自动释放，不靠删除锁文件争抢写入。

默认边界：每宿主状态目录最多 1000 个会话，每会话最多 512 轮、文件 32 MiB、首行 16 KiB。列表每页最多 100 条，回合摘要每页最多 20 条，执行项每页最多 50 条。Runtime 默认 4 MiB 初始接纳、8 MiB 累计历史及 8 MiB 检查点限制独立生效；会话接纳使用宿主实际限额并预留当前系统消息；有已验证摘要的较大档案可用较小工作集继续，缺乏可用工作集或关闭压缩时仍可能先触及较小的限制。删除只针对选中的已结束会话及匹配 revision；运行中的会话拒绝删除或重命名。置顶与归档只改变用户导航，不改变执行状态，因此运行中也可设置，但仍要求匹配 revision，避免覆盖更新的首行。列表默认只返回未归档会话并按置顶优先、更新时间倒序排列；`GET /api/sessions?archived=true` 返回已归档会话，供页面恢复。

## 保存、失败与崩溃

新轮次先保存用户输入及持久请求 ID，再进入 Application。相同会话内相同请求 ID 与相同 prompt 返回原轮次，不重复启动；同 ID 不同内容冲突。修订号过期会提示刷新，不能覆盖其他页面已经保存的修改。创建会话也以请求 ID 去重。删除整个会话后不保留永久幂等墓碑，这不是跨设备或永久 exactly-once 保证。

模型/工具的保存时序复用原有 BeforeModel、AfterModel、ToolIntent、ToolSettled、AfterTools、InputApplied 和 RunFinished 确认点。存储操作在阻塞线程池中执行，调用者等待结果；取消时在原子替换前再次检查令牌。超时或确认失败不证明磁盘没有完成写入，也不证明外部工具没有产生影响；后续动作仍遵守 Runtime 的 fail-closed 规则。

重启获取独占写锁后，原 `running` 轮次标记为 `interrupted`，不自动调用模型、运行工具或恢复旧预算。打开历史时只构造恢复投影：尚未记录工具 intent 的调用记为 Skipped；已记录 intent 但没有确认结果的调用记为 Unknown；已确认结果保留原状态。恢复不修改原始检查点，也不猜测副作用已撤销。用户明确发送下一轮后才建立新的 Run，并带上这些状态供模型判断。

已确认的运行中追加指令随 InputApplied 检查点保存；尚未被接纳的队列输入不能宣称已保存。未完成的模型 token、瞬时工具进度日志、UI 展开状态及自定义 UI detail 不形成耐久日志。重启后显示的是确认过的消息、参数、工具结果和终态，不保证逐字重播中断前的动画。Spill 引用保留；宿主允许同会话的新 Run 在结构化归属校验后读取原 Run 文件，不允许跨会话伪造 URI 授权。文件依然遵守 Spill 自己的到期策略，过期或删除明确报不可用，不承诺永久附件归档。

损坏、截断、未知格式或不一致的回合范围不会被当作空会话覆盖。无效首行在列表中计数提示；正文错误在打开时明确报错。存储初始化失败会阻止持久宿主启动，而不是悄悄退回易丢失的内存模式。

## 安全与部署边界

浏览器仅收到用户内容、助手可见文本和有界工具结果；系统消息、原始模型审计、隐藏推理和 Provider 私有回传字段不通过历史接口回传。API 使用同一可信宿主 Bearer、明确 CORS、请求体大小上限及 `Cache-Control: no-store`，不允许页面提交任意 history、权限、模型配置或文件路径。客户端不自动重试写操作，也不在 localStorage 保存正文或 Token。

本地会话文件是**未加密的用户数据**，其中可能包含用户输入、敏感工具结果以及模型继续对话所需的私有协议数据；它们不是可公开上传的日志。Unix 会话目录设为 0700，Windows 继承用户状态目录 ACL。部署者负责磁盘权限、备份与必要的磁盘加密。会话工作目录绑定不是多租户鉴权，也不是任意本地进程的安全沙箱。

当前支持单机本地文件系统。进程崩溃恢复有测试，未做断电、网络盘或跨进程分布式文件系统耐久性认证。每个检查点的完整文件替换会产生与会话长度有关的 I/O 成本；本版不把它伪装成常数成本的增量日志。

通用 `/v1/runs` 仍是明确的临时 Run 接口，`/v1/info` 的非耐久回放描述不变。扩展和 Application 会话适配均可供 Tauri/CLI 复用，但当前 Tauri 示例未装配会话管理端点，不能宣称已完成原生产品接入。不包含树形分支、跨设备同步、自动崩溃续跑或旧会话文件导入。

## 验证

存储独立性、失败、取消和严格格式测试位于 `packages/sessions/tests`；无 Web 的规则/Skills/摘要组合见 `composition.rs`；产品接口与私有内容过滤测试位于 `apps/server/src/session_routes`。


```sh
cargo test --offline -p application -p http-bridge -p server
cargo test --offline -p sessions --no-default-features
cargo test --offline -p sessions --features compaction
cargo test --offline -p server --no-default-features --bin server session_routes::tests
npm run test:web
cargo build --offline -p server
node apps/web/test/sessions-e2e.mjs
```

浏览器测试使用隔离的临时工作空间、状态目录、随机环回端口、受控模型端点和真实 Rust `read` 工具；会终止并重新启动自己创建的测试宿主，不操作开发者正在运行的服务，不调用真实供应商。浏览器可通过 `BROWSER_BIN` 指定，默认 Windows Edge / Unix Chromium；报告默认在忽略目录 `target/browser-reports/sessions`，可用 `MONA_TEST_REPORT_DIR` 指定。构建或单元测试不能替代这条重启、续聊与异常中断检查。
