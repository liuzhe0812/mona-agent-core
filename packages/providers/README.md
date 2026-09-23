# providers

`providers` 实现具体模型协议到公共 `Model` 接口的转换。当前正式实现是 OpenAI Chat Completions 风格的 HTTP/SSE 适配器；它不拥有用户设置、路由或 UI。

`ChatConfig` 配置端点、模型、凭据、请求限制、协议回传白名单及可选 `context_window_tokens`。上下文容量未知时保持 `None`，由 Runtime 使用字节硬限制。

非成功响应只读取有界错误正文用于分类，不把正文、Prompt 或凭据写入错误消息。分类优先读取 `error.code`/`error.type` 的已知标识；401/403 按鉴权、402 按额度优先，409 默认是确定性请求错误，只有明确的临时故障标识才进入限流或服务端分类。严格匹配的标准 `message` 仅作为回退，不扫描任意正文。适配器区分鉴权、额度、限流、服务端、确定性请求错误、上下文超限和一般传输故障，并保留可用的 HTTP 状态与 `Retry-After`。适配器本身不重试；统一 Runtime 网关负责预算、取消、审计和恢复策略。

`context_window_tokens` 只对配置的 `ChatConfig.model`（或未指定覆盖模型的请求）生效；请求选择备用模型时返回 `None`。SSE 回传的 replay tool index 使用 API 的独立 `MAX_TOOL_CALLS_PER_STEP` 上限，不受 UI 快照保留量影响。

同一个网络数据块可能同时包含模型内容、用量和后续错误。适配器先按顺序交付已经解码的内容与用量，再交付一次终态错误；不会让后续错误抹去此前的输出事实。文本、推理、工具参数和私有回传数据都会建立不可撤销的 `model_output_started` 标记，由 Runtime 据此禁止部分输出后的自动重试。

必要验证：

```sh
cargo test -p providers
```
