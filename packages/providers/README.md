# providers

`providers` 实现具体模型协议到公共 `Model` 接口的转换。当前正式实现是 OpenAI Chat Completions 风格的 HTTP/SSE 适配器；它不拥有用户设置、路由或 UI。

`ChatConfig` 配置端点、模型、凭据、请求限制、协议回传白名单及可选 `context_window_tokens`。上下文容量未知时保持 `None`，由 Runtime 使用字节硬限制。

非成功响应只读取有界错误正文用于分类，不把正文、Prompt 或凭据写入错误消息。分类优先读取 `error.code`/`error.type` 的已知标识；401/403 按鉴权、402 按额度优先，409 默认是确定性请求错误，只有明确的临时故障标识才进入限流或服务端分类。严格匹配的标准 `message` 仅作为回退，不扫描任意正文。适配器区分鉴权、额度、限流、服务端、确定性请求错误、上下文超限和一般传输故障，并保留可用的 HTTP 状态与 `Retry-After`。适配器本身不重试；统一 Runtime 网关负责预算、取消、审计和恢复策略。

`context_window_tokens` 只对配置的 `ChatConfig.model`（或未指定覆盖模型的请求）生效；请求选择备用模型时返回 `None`。SSE 回传的 replay tool index 使用 API 的独立 `MAX_TOOL_CALLS_PER_STEP` 上限，不受 UI 快照保留量影响。

同一个网络数据块可能同时包含模型内容、用量和后续错误。适配器先按顺序交付已经解码的内容与用量，再交付一次终态错误；不会让后续错误抹去此前的输出事实。文本、推理、工具参数和私有回传数据都会建立不可撤销的 `model_output_started` 标记，由 Runtime 据此禁止部分输出后的自动重试。

## 历史回传检查

`Model::validate_history` 是不发送网络请求的协议检查。普通历史可换模型；含私有字段时，需要匹配供应商命名空间和路由指纹。指纹包含规范化端点、实际请求模型、部署扩展参数及私有请求选项，不包含 API Key、运行编号或设置 revision。重启和替换密钥不改变同一路由身份；改变模型、端点或私有配置不会借用旧身份。稳定模型别名背后的服务端变化无法由本地指纹发现。

Chat 的 `ProviderData.value` 仅接受 `{route, fields}` 当前结构。完整私有字段保存在 fields，发送时只恢复白名单内的字段，不把 route 元数据发送给供应商。推理文本仍只存一份 `reasoning_content`，在流结束时附上小型来源信封，不逐 token 重复复制推理。无来源推理、外来签名、重复承载推理或不支持字段均返回 `ModelHistoryIncompatible`，不丢字段、不迁移、不重试。

该检查不预测凭据有效性、模型模态能力或供应商最终是否接受请求；具体错误仍按实际协议返回。跨供应商原生协议及安全转换不在当前实现范围。

必要验证：

```sh
cargo test -p providers
```
