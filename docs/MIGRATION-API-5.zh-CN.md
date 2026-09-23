# Rust 插件 API 5 迁移

包版本仍为 0.3.0，Rust `API_VERSION` 从 4 升至 5；UI `STREAM_VERSION` 和检查点协议不变。

`RunContext` 新增只读字段 `tools_enabled`。上下文组件在发布需要模型调用工具的指引前，应先检查该字段；需要特定工具时还要检查 `allowed_tools`。使用 `PluginManifest::new` 的插件自动声明当前版本，手填版本和直接构造 `RunContext` 的代码需要更新。

此次变更支持 Skills 在没有专用 `skill` 工具的情况下发布目录：只有 Run 启用工具且允许 `read` 时，目录才进入模型投影。HTTP/Tauri 任务协议和 UI 流协议没有字段变化。
