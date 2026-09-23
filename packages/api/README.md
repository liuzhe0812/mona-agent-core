# api

`api` 定义模型、工具、运行、事件、检查点和 Plugin 的公共 Rust 契约，不依赖具体 Runtime、Provider、存储或 UI。

当前 `API_VERSION` 为 9。主要运行入口是 `RunRequest`、`AgentRuntime`、`RunSession` 和 `RunReport`；扩展通过 `Tool`、`ContextTransform`、`ResultTransform`、`ToolPolicy`、`ToolSelector`、`CheckpointSink` 与 `Plugin` 接入。

`RunLimits` 分开限制启动历史接纳量（`max_initial_history_bytes`，默认 4 MiB）、运行中累计历史（`max_history_bytes`，默认 8 MiB）与实际模型请求量（`max_context_bytes`，默认 256 KiB）。启动时同时满足前两个限制；累计历史按规范 JSON 字节计量，不代表进程 RSS 或模型 token。模型重试默认关闭。`AuditMode::Full` 保存完整模型请求，`AuditMode::Metadata` 只保存大小、用量、错误及结算状态。`AgentError` 的模型错误码、HTTP 状态和 `retry_after_ms` 是机器可读事实，是否恢复由 Runtime/宿主策略决定。

精简审计序列化时显式输出 `request: null`，并且不保存失败时的部分模型正文（`partial_text: null`）；完整模式继续保留有界的部分输出证据。旧记录省略 `request` 字段时仍能反序列化。

`TaskControl::check_model_call_available()` 是重试等待前使用的非预留检查；并发调用仍必须通过 `reserve_model_call()` 原子预留。普通 `check()` 不检查调用次数是否恰好用完，避免阻止最后一次获准调用正常完成。

模型可通过 `Model::context_window_tokens` 声明容量；不知道时返回 `None`。`ContextTransform::recover_context` 只在供应商确认上下文超限后获得一次缩小投影的机会，默认不处理。

`ToolSelector` 每轮先基于已结算的正式历史选择工具，只调用一次；随后 `ContextTransform::sources` 提供有来源身份的 `ContextBlock`，再压缩真实对话。本轮已选工具及来源用于一致计量、请求发送、执行校验和溢出恢复。选择期间 `available` 是候选集合；选择后 `RunContext.allowed_tools` 是本轮已选集合，不能据初始 Run 上限发布当前不可读取的引用。`RunContext` 的 `request_tools/context_sources` 与 `model_request` 帮助组件统一计量，来源不会伪装成正式用户轮次。`ModelCaller::estimate_input_tokens` 是可选的主请求占用估算，不是账单。

`RunHandle::wait_owned()` 把 Future 的生命期绑定到执行：未完成时丢弃会取消该 Run，成功后解除取消守卫。所有 `AgentExecutor` 包装器共用它；`wait()` 只是观察，丢弃句柄或观察者不取消任务。

`ToolPolicy::check` 在各工具 intent 确认后、实际派发前调用一次，不缓存整个批次的动态决定。拒绝/检查失败不进入工具函数；权限检查不是外部资源事务。

`Model::validate_history` 与 `AgentRuntime::validate_history` 提供无网络的历史接纳检查，协议规则属于适配器；包装器应转发，实际启动必须针对绑定模型再次检查。默认空实现用于无私有回传约束的实现；`ModelHistoryIncompatible` 表示不能安全回传，不应重试、删字段或自动改用其他模型。检查成功不是模型配置预留。

`api::validate_messages` 是执行历史与存储恢复共用的消息配对校验，检查未完成批次、重复调用 ID、孤立工具结果与内容格式。它只验证传入数据，不执行模型、工具或存储；Runtime 不再保留另一份公共实现或兼容别名。

UI 投影与模型/工具数据严格分离：`UiToolResult` 只提供有界文本、安全 `UiContentBlock` 和受限 `ArtifactRef`。Base64 图片可进入 UI；远程图片 URL 与带路径/查询的 Resource locator 会被脱敏，只有 `spill:sp_xxx` 这类无路径 opaque 引用形态可作为可读取引用进入页面。任意 `ToolResult.structured` 不会复制到浏览器。工具若要提供结构化展示，应通过有命名空间且受 `UI_DETAIL_BYTES/UI_DETAIL_KEYS` 限制的 `ToolProgress::set_detail`；这类 detail 只影响展示，不改变执行状态。

开发阶段只维护当前契约，不承诺旧版接口兼容。必要验证：

```sh
cargo test -p api
```
