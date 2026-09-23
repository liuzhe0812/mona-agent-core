# Rust 插件 API 7 与上下文存储迁移

包版本仍为 0.3.0，`API_VERSION` 从 6 升至 7。重新编译原生插件；UI `STREAM_VERSION` 仍为 2，公共 `CHECKPOINT_VERSION` 仍为 1。

## Rust 接口

`RunContext` 新增 `request_tools: Arc<Vec<ToolSpec>>` 与 `context_sources: Arc<Vec<ContextBlock>>`。手工构造该结构的测试/嵌入代码需要补字段；无来源/工具时用空 Vec。当前 Runtime 在选择完成后填充本轮已选工具与来源；选择器在来源和投影之前执行，每轮一次。早期按全部 Run 工具上限计量的实现已删除，见[当前扩展契约](PLUGIN-GUIDE.zh-CN.md#5-上下文变换)。

`ContextTransform::sources(ctx, messages)` 是默认返回空数组的方法。已有纯压缩 `transform` 无需改变；目录、检索、规则等注入组件应改在 `sources` 中返回有唯一来源标识的 `ContextBlock`，让 Runtime 先预留预算。不要在 `sources` 和 `transform` 重复注入同一内容。`RunContext::with_sources` 和 `model_request` 用于构造包含来源及工具开销的计量请求；最终请求仍由 Runtime/Gateway 校验。

`ModelCaller::estimate_input_tokens` 默认返回 None。返回值 `InputTokenEstimate` 表示占用估算，不是计费。自定义调用网关没有可信主请求用量锚点时保持默认；不能把摘要调用用量作为主对话窗口占用。

`models::ModelEntry` 新增 `context_window_tokens: Option<u64>`。Rust 结构体字面量需要补字段；旧模型配置 JSON 缺字段仍按 None 读取，不要求迁移密钥或重设供应商。模型管理页可设置/清空容量。

新接口会显式序列化 `context_window_tokens: null` 表示未知，依赖完整 JSON 对象相等的宿主测试需更新预期。由于旧 `ModelEntry` 拒绝未知字段，模型设置重新保存后也不承诺能被旧版模型管理组件读取；回退二进制前应同时保留升级前的模型设置备份，不能只回退代码。

## 宿主会话

本地会话格式升至 2，但文件路径和两行 JSONL 原子替换方式不变。读取格式 1 时仍按旧检查点重建；成功修改后写为格式 2。新格式把完整档案与工作上下文分开，并保存可校验的压缩覆盖范围。**旧版宿主不应再打开已迁移为格式 2 的会话文件**；回退应用前应保留升级前备份。

直接使用 `Compactor` 的宿主，可在可等待的提交点取得 `state(run_id)`，将它与对应工作历史原子保存，并在下一轮通过 `CompactionState::project` 校验恢复。必须在 `finish(run_id)` 清理前捕获；正式 Server 已在现有 CheckpointSink 中完成装配。不要把序列化摘要文本当作模型可授予权限的状态，也不要用它覆盖原始档案。

完整机制、数据边界和测试入口见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)。
