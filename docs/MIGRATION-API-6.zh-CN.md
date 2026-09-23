# Rust 插件 API 6 迁移

包版本仍为 0.3.0，Rust `API_VERSION` 从 5 升至 6；UI `STREAM_VERSION` 保持 2，检查点协议保持 1。

- `AgentError` 新增可选 `http_status`、`retry_after_ms` 和 `model_output_started`，并增加模型鉴权、额度、限流、服务端、确定性请求错误和上下文超限错误码。使用 `AgentError::new` 的代码无需变化。
- `RunLimits` 新增 `max_initial_history_bytes`、`model_retry`、`audit_mode`。模型重试默认关闭；默认审计仍为完整模式。
- `RequestAudit.request` 改为 `Option<ModelRequest>`，并新增 `request_bytes`。读取完整请求前必须处理精简模式的 `None`。精简模式序列化明确输出 `request: null`，`partial_text` 也为 `null`；旧记录缺失 `request` 字段仍兼容。完整模式保留请求与有界部分输出证据。
- `Model` 新增有默认实现的 `context_window_tokens`；不知道模型容量时无需实现。
- `RunContext` 新增 `model_context_window_tokens`。直接构造该结构的测试或嵌入代码需补字段。
- `ContextTransform` 新增有默认实现的 `recover_context`。它只能在上下文超限后返回更小投影，不能重放工具。
- `max_tools_per_step` 不再依赖 UI 的 256 项缓存，独立硬上限为 4096。

2026-09-23 补充：`TaskControl::check_model_call_available()` 提供不消耗预算的可用性检查，供重试等待前使用；它不能代替 `reserve_model_call()` 的原子预留。普通 `check()` 的语义不变，最后一次获准调用仍可正常完成。本次边界修正不再次变更 API、UI 或检查点协议号。

重编译所有 Plugin。完整审计调用者使用 `audit.request.as_ref()`；需要省内存的可信宿主可显式选择 `AuditMode::Metadata`。
