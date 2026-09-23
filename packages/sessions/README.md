# sessions

扩展层的本地持久会话：保存完整已确认历史、轮次身份、工作上下文与可选摘要状态，支持重启后查看和显式继续。生产依赖不包含 Runtime、Application、HTTP、Tauri 或 Web；宿主提供存储目录、工作空间和授权。它不执行模型或工具，不自动重放中断任务。

## 接入

| 接口 | 用途 |
|---|---|
| `Store::open(base, workspace)` | 打开宿主权限域的状态目录并持有单写者锁；workspace 仅为 `create` 的显式默认目录，不决定历史存储路径 |
| `create_in(key, cwd, metadata)` | 为会话永久绑定可信目录及有界组织元数据；无需 projects 包，不额外创建子目录 |
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

`Store` 是同步文件 API，持续运行中的宿主应放到阻塞线程池；`SessionSink` 已完成这层适配。接口用法与重开、取消、去重断言见 `tests/standalone.rs`，完整的规则、Skills、摘要组合见 `tests/composition.rs`。这些测试不依赖产品层。

使用既有 Application 的产品可启用它的 `sessions` feature，再构造 `application::sessions::SessionApplication`。该适配器负责启动去重、保存失败和短期 Run 注册表清理；存储实现仍只有本包一份，不要求 HTTP。见 [Application](../application/README.md)。

## 摘要与恢复

`compaction` Cargo feature 默认关闭。需要跨轮次摘要时启用它，将同一个 `CompactionPlugin::compactor()` 传给 `Store::attach_compactor`，同时在 Host 装配该 Plugin。摘要、覆盖范围和检查点一起确认；下一轮验证工作前缀与原文指纹后重建较小工作集，原档案不改写。

当前摘要是 CompactionState 格式 2 的结构化任务交接记录；只接受当前状态，不转换自由文本旧摘要。`tests/long_handoff.rs` 验证连续压缩、五轮重开存储、用户纠正、已完成工具不重跑和待办保留。它使用受控模型，不证明真实模型必然正确概括所有事实。

`admission_bytes` 包含返回历史与新 prompt，不包含宿主尚未插入的系统消息；宿主应先预留系统消息，并取初始历史及累计历史限额的较小值。容量不足在保存新轮次前返回错误，旧摘要和档案保持不变。

仅支持当前格式 3，没有旧格式读取、自动迁移或旧字段兼容。会话文件直接位于宿主指定 base 下，实际 cwd 保存在每个会话头；旧哈希分组目录明确拒绝而不改动。默认根和普通会话目录分配属于宿主，见[工作区方案](../../docs/WORKSPACES.zh-CN.md)。未知格式、损坏和不一致数据明确拒绝，不重置为空会话。保存采用同目录临时文件、sync、原子替换；重启将运行中轮次标为中断。未派发工具为 Skipped，已有 intent 但结果未知为 Unknown；不猜测回滚，不自动重跑。

## 边界与验证

本地文件未加密；权限域与磁盘保护由宿主负责。每宿主状态目录最多 1000 个会话，每会话最多 512 轮、文件 32 MiB；完整快照有随历史增长的 I/O 成本。本包不是永久附件库、分布式存储、事务回滚或跨设备同步系统。Web 接口和分页展示见 [本地会话](../../docs/LOCAL-SESSIONS.zh-CN.md)。

```sh
cargo test -p sessions --no-default-features
cargo test -p sessions --features compaction
```
