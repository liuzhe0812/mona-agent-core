# sessions

扩展层的本地持久会话：保存完整已确认历史、轮次身份、工作上下文与可选摘要状态，支持重启后查看和显式继续。生产依赖不包含 Runtime、Application、HTTP、Tauri 或 Web；宿主提供存储目录、工作空间和授权。它不执行模型或工具，不自动重放中断任务。

## 接入

| 接口 | 用途 |
|---|---|
| `Store::open(base, workspace)` | 打开宿主权限域的状态目录并持有单写者锁；workspace 仅为 `create` 的显式默认目录，不决定历史存储路径 |
| `create_in(key, cwd, metadata)` | 为会话永久绑定可信目录及有界组织元数据；无需 projects 包，不额外创建子目录 |
| `initialize_history(id, revision, history, state)` | 只为尚无轮次的空会话保存可信初始历史及扩展状态；拒绝 System 与未配对工具，不伪造已完成轮次 |
| `header` / `list_matching` | 有界轻量头查询和宿主过滤；不因默认目录改变或原 cwd 缺失隐藏历史 |
| `create/list/get/rename/delete` | 会话管理；修改使用 revision 防止覆盖，运行中不能删除 |
| `prepare(id, revision, request_id, prompt, admission_bytes)` | 在启动 Run 前可靠保存输入并去重；返回 `Prepared::New { history }` 或已保存的轮次；上限由宿主传入 |
| `prepare_checked(..., check)` | 在同一 revision/接纳锁内对准确工作历史执行宿主纯检查；失败不保存新轮次，已保存的重复请求不重新检查或执行 |
| `SessionSink(store)` | 接入现有 `CheckpointSink`，等待原子保存确认；不使用尽力而为的事件观察器 |
| `runtime(inner, store)` | 装饰同一个 `AgentRuntime`，保持取消语义并等待失败收尾；不是新执行循环 |
| `Document::history()` / `source_history(id)` | 读取完整事实；不把摘要当作用户原文，不恢复旧权限或系统指令 |
| `artifact_owner(session, current_run, uri)` | 校验当前 Run 会话身份和已确认结构化引用，返回最初归档 Run；URI 文本不是授权 |
| `references::SessionReferences` | 将已确认归档引用作为有界上下文来源提供给模型 |

直接嵌入时，向同一个 Host 注册 `SessionSink`，再使用 `sessions::runtime` 包装其公共运行接口。创建会话后调用 `prepare`；只在返回 New 时把返回历史和新 prompt 构造成 `RunRequest`，设置 `SESSION_KEY`、`TURN_KEY` 两项可信 metadata。当前系统指令、模型、工具权限与预算由宿主提供，不从档案恢复。检查点会确认 Run 与轮次绑定；启动失败调用 `fail_start`。已保存的重复请求不能再执行一次。

`Store` 是同步文件 API，持续运行中的宿主应放到阻塞线程池；`SessionSink` 已完成这层适配。接口用法与重开、取消、去重断言见 `tests/standalone.rs`，完整的规则、Skills、摘要组合见 `tests/composition.rs`。这些测试不依赖应用层。

使用既有 Application 的产品可启用它的 `sessions` feature，再构造 `application::sessions::SessionApplication`。该适配器负责启动去重、保存失败和短期 Run 注册表清理；存储实现仍只有本包一份，不要求 HTTP。见 [Application](../application/README.md)。

## 摘要与恢复

`compaction` Cargo feature 默认关闭。需要跨轮次摘要时启用它，将同一个 `CompactionPlugin::compactor()` 传给 `Store::attach_compactor`，同时在 Host 装配该 Plugin。摘要、覆盖范围和检查点一起确认；下一轮验证工作前缀与原文指纹后重建较小工作集，原档案不改写。

当前摘要是 CompactionState 格式 2 的结构化任务交接记录；只接受当前状态，不转换自由文本旧摘要。`tests/long_handoff.rs` 验证连续压缩、五轮重开存储、用户纠正、已完成工具不重跑和待办保留。它使用受控模型，不证明真实模型必然正确概括所有事实。

`admission_bytes` 包含返回历史与新 prompt，不包含宿主尚未插入的系统消息；宿主应先预留系统消息，并取初始历史及累计历史限额的较小值。容量不足在保存新轮次前返回错误，旧摘要和档案保持不变。

仅支持当前格式 5，没有旧格式读取、迁移、探测或旧字段兼容。会话文件直接位于宿主指定 base 下，实际 cwd 保存在每个会话头；Store 只扫描当前目录下的 `*.jsonl` 文件，其他目录不属于当前会话格式。默认根和普通会话目录分配属于宿主，见[应用层与 Web/桌面装配](../../docs/architecture/WEB.zh-CN.md)。当前格式文件损坏或不一致时明确拒绝，不重置为空会话。保存采用同目录临时文件、sync、原子替换；重启将运行中轮次标为中断。未派发工具为 Skipped，已有 intent 但结果未知为 Unknown；不猜测回滚，不自动重跑。

每轮随检查点保存实际进入的步数与 `RunStatistics`。统计仅含数字和上下文估算，不保存模型请求正文；已确认历史翻页或宿主重启后可按会话汇总，未采集过的分项保持未知。

## 宿主扩展状态

`Body.state` 是有界、具名的 `HostState`，和轮次、原文、检查点保存在同一个会话文件中。它不进入模型输入，不直接作为公共 HTTP 响应，也不自动授予权限。本包不认识 Planner 或任何 UI 类型；业务解释和安全投影由可信宿主负责。

| 接口 | 确认与并发语义 |
|---|---|
| `update_state(id, revision, key, update)` | 仅空闲且未归档的会话可以更新；纯回调读取精确版本，完整校验后原子保存；失败不发布新状态 |
| `update_auxiliary_state(id, key, update)` | 仅接受 `aux.*` 命名空间，供可信宿主在活动会话中原子维护有界辅助信息；不改写执行历史、授权或检查点身份 |
| `prepare_with_context(..., check)` | 在接纳锁中读取当前 Document 和工作历史、生成有界可信 metadata，再保存新输入；旧检查点尚未被新工作集替换，重复轮次不重新运行回调 |
| `commit_with_state(checkpoint, cancel, patch)` | 运行检查点与其派生状态一并提交；相同检查点的重发还需 patch 内容一致，不形成第二个存储事务 |

状态最多 16 项，键为至多 64 字节的字母、数字、点、下划线或连字符，序列化总量不超过 64 KiB。接纳回调生成的 metadata 最多 14 KiB，不能覆盖 Session/Turn 身份，余量留给宿主身份字段。回调必须是有界纯计算，不能重新进入 Store 或执行网络、文件操作。跨 Run 设置必须通过可信接口绑定，不接收浏览器任意 JSON 作为执行 metadata。

标准 Web 的计划模式使用此存储能力：用户空闲时切换模式不需要伪造用户消息或调用模型；工具执行中的计划投影与检查点共用一次确认。实例见 Server 的 `planning.rs`。`SessionSink` 普通装配不要求任何状态插件，关闭 Planner 不删除状态。格式 5 的 `state` 为必需字段，不推断或迁移旧格式。

`initialize_history` 将起始前缀保存为 `sessions.origin`，后续轮次范围从此前缀之后计算；恢复时校验内容与指纹，不把继承资料伪装成该会话已执行的工作。Subagent 复用此接口保存独立子记录，并以辅助状态保存消息与模型身份；父子授权关系仍由 Subagent/宿主解释，不进入通用 Store。

## 可选历史检索

启用 `sessions/search` 后，`HistorySearch::open(store)` 在同一私有目录建立 `history-index.sqlite3` 派生索引；不替换可靠会话正本，也不在关键检查点里追加另一份持久化事务。查询时按当前档案 revision/文件状态增量刷新，完成的索引可跨查询和重启复用，丢弃索引后可从原文重建。索引损坏或刷新失败明确报错，不返回假空结果。捕获原文快照时短暂持有会话锁，分词和 SQLite 操作不持锁，不让索引工作长期占用可靠提交关口；返回前核对来源版本，期间变化则明确重试。

`search(scope, SearchRequest, cancel)` 使用安全构造的 FTS5 词项表达式；中文采用字/双字词项，英文/标识符使用词项，无语义模型或第二次摘要调用。默认 8 条、最多 20 条结果；读取原文每页最多 8 KiB。SQLite 和刷新操作受取消与约 10 秒总期限限制，持锁等待也计时。索引上限约 256 MiB；大规模首次构建有成本，不承诺毫秒检索。

结果提供 session/turn/message 位置、revision、内容指纹和有界摘录；`read(scope, ReadRequest, cancel)` 再读会话正本，返回原文页和相邻消息定位。仅追加历史、检查点或重命名不使未改变的原文失效；指纹/身份不匹配则拒绝。只检索用户正文、助手可见文本及工具文本/状态，排除 System、隐藏推理、ProviderData、Base64、structured 和 UI 缓存；压缩前原文来自 `Document::history()`，不是模型摘要。

`search::tools(search, ScopeResolver)` 返回只读 `session_search` / `session_read`，复用既有 Tool 接口。Scope 由可信宿主解析，不是模型参数；查询中的 session_id 只能缩小范围。Web 默认普通会话工具只检索普通会话，项目会话只检索同项目；临时子会话继承父会话的历史读取范围，但不获得写入父会话的身份。工作目录相同不代表获准读取另一个项目；管理 API 属于本机单用户权限域，可由用户主动查看全部历史。删除后不再返回该会话，归档不等于删除。索引是私有用户数据；删除不等于磁盘安全擦除或已发送内容撤回。

该功能不依赖 Memory。关闭 search feature 不链接 SQLite，不影响会话保存、恢复及压缩。见[上下文与三类记忆](../../docs/architecture/CONTEXT-MEMORY.zh-CN.md)。

## 边界与验证

本地文件未加密；权限域与磁盘保护由宿主负责。每宿主状态目录最多 1000 个会话，每会话最多 512 轮、文件 32 MiB；完整快照有随历史增长的 I/O 成本。本包不是永久附件库、分布式存储、事务回滚或跨设备同步系统。Web 接口和分页展示见 [会话接纳与产品接口](../../docs/architecture/WEB.zh-CN.md)。

```sh
cargo test -p sessions --no-default-features
cargo test -p sessions --features compaction
cargo test -p sessions --features search --test search
```
