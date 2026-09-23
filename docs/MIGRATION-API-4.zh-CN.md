# Rust 插件 API 4 迁移

包版本仍为 0.3.0，Rust `API_VERSION` 从 3 升至 4；UI `STREAM_VERSION` 仍为 2，检查点协议仍为 1。

本次变更为 `RunContext` 增加 `limits`、`request_overhead_bytes` 和 `allowed_tools`，并为 `RunLimits` 增加 `context_timeout`。使用 `PluginManifest::new` 的插件会自动声明当前版本；手填版本的插件需要更新。直接构造 `RunContext` 或完整 `RunLimits` 字面量的代码需要补齐字段，优先使用 `RunLimits::default()` 加覆盖值。

`ContextTransform` 的方法签名不变。实现现在可以使用 `RunContext::max_message_bytes()` 计算消息投影的保守预算；最终完整请求仍由模型网关校验。上下文变换不再共享短策略 Hook 的 `hook_timeout`，改用受任务截止时间约束的 `context_timeout`。

HTTP/Tauri 任务请求与 UI 流协议没有字段变化。
