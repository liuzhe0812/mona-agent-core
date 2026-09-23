# providers

`providers` 将 Chat Completions、OpenAI Responses、Anthropic Messages 转换为同一个公共 `Model` 接口。协议实现属于扩展层；不拥有用户设置、执行循环、会话数据库或 UI。

统一入口为 `ProviderConfig::new(protocol, full_endpoint, model)` 和 `create_model(config)`；也可直接构造 `ChatModel`、`ResponsesModel`、`MessagesModel`。宿主提供完整端点、API Key、限制、命名空间和实际模型窗口。Chat 的专用回传白名单配置继续由 `ChatConfig` 提供。

| 协议 | API Key 请求头 | 本地工具与流终止 | 私有历史 |
|---|---|---|---|
| Chat Completions | Bearer Authorization | tool_calls；Finish 与 `[DONE]` | `{route, fields}` |
| Responses | Bearer Authorization | function_call/function_call_output；response.completed/incomplete | 原顺序 output items 与 encrypted_content |
| Messages | x-api-key；anthropic-version 2023-06-01 | tool_use/tool_result；message_stop | 原顺序 content blocks 与 thinking signature |

Responses 使用 `store:false` 和 `reasoning.encrypted_content`；只发送本地保存的完整输入，不使用 previous_response_id、conversation 或后台执行。Messages 连续工具结果作为同一 user 消息的 tool_result 块发送；system 单独映射。工具声明和执行仍由原 Runtime 管理。

非成功响应只读取有界错误正文用于分类，不把正文、Prompt 或凭据写入错误消息。分类优先读取 `error.code`/`error.type` 的已知标识；401/403 按鉴权、402 按额度优先，409 默认是确定性请求错误，只有明确的临时故障标识才进入限流或服务端分类。严格匹配的标准 `message` 仅作为回退，不扫描任意正文。适配器区分鉴权、额度、限流、服务端、确定性请求错误、上下文超限和一般传输故障，并保留可用的 HTTP 状态与 `Retry-After`。适配器本身不重试；统一 Runtime 网关负责预算、取消、审计和恢复策略。 Messages 的 `invalid_request_error` 加完整 `prompt is too long` 格式明确映射到上下文超限；带数量时检查 `输入 tokens > 最大值 maximum`，不靠任意正文子串推断。此错误格式参照 [Claude 上下文溢出说明](https://platform.claude.com/docs/en/build-with-claude/context-windows)。

`context_window_tokens` 只对配置的 `ChatConfig.model`（或未指定覆盖模型的请求）生效；请求选择备用模型时返回 `None`。SSE 回传的 replay tool index 使用 API 的独立 `MAX_TOOL_CALLS_PER_STEP` 上限，不受 UI 快照保留量影响。

同一个网络数据块可能同时包含模型内容、用量和后续错误。适配器先按顺序交付已经解码的内容与用量，再交付一次终态错误；不会让后续错误抹去此前的输出事实。文本、推理、工具参数和私有回传数据都会建立不可撤销的 `model_output_started` 标记，由 Runtime 据此禁止部分输出后的自动重试。

## 历史回传检查

`Model::validate_history` 是不发送网络请求的协议检查。普通历史可换模型；含私有字段时，需要匹配供应商命名空间和路由指纹。指纹包含规范化端点、实际请求模型、部署扩展参数及私有请求选项，不包含 API Key、运行编号或设置 revision。重启和替换密钥不改变同一路由身份；改变模型、端点或私有配置不会借用旧身份。稳定模型别名背后的服务端变化无法由本地指纹发现。

Chat 的 `ProviderData.value` 仅接受 `{route, fields}` 当前结构。完整私有字段保存在 fields，发送时只恢复白名单内的字段，不把 route 元数据发送给供应商。推理文本仍只存一份 `reasoning_content`，在流结束时附上小型来源信封，不逐 token 重复复制推理。无来源推理、外来签名、重复承载推理或不支持字段均返回 `ModelHistoryIncompatible`，不丢字段、不迁移、不重试。

Responses/Messages 的私有信封为 `{route, items}`，保存完整有序协议项；恢复前校对可见正文、工具 ID、名称与参数，不能借旧签名改写动作。Responses 允许 encrypted_content 仅在最终快照补齐，但不会覆盖已收到的密文。Messages 保留 thinking/redacted_thinking；隐藏内容不生成 UI 思考事件。私有信封受公共 64 KiB 限额约束，无法保存时明确失败而不是丢签名。

普通历史按目标协议映射，工具标识也必须符合目标协议要求；私有历史只能由同协议、同实际路由接纳。不兼容返回 `ModelHistoryIncompatible`，不会静默删字段、改 ID、改写签名或自动新建会话。检查不预测凭据、服务端别名变化或服务端最终接纳。

## 能力与明确限制

`ModelCapabilities` 提供 tools/images/temperature/top_p/stop 三态事实及可选最大输出。未知不是支持承诺，也不按模型名猜测。已知不支持在发送前拒绝；已知输出上限不能被请求覆盖。窗口与这些事实由宿主按具体模型配置，运行期间不切换。

用户图片和工具图片支持 Base64，URL 需宿主显式允许；Resource 不是文件内容，默认拒绝，不能暗中读取或联网。Responses 工具图片置于 function_call_output 的 input_image；Messages 置于 tool_result 的 image。文字结果保留状态和结构化数据，不把 Base64 当普通文字。

Responses 不支持 stop 序列。Messages temperature 限于 0..1；显式 thinking 与采样参数组合、thinking 预算不小于本轮输出预算时明确拒绝。原生额外参数只接受 Responses 的 reasoning，或 Messages 的 thinking/output_config；不允许覆盖工具、凭据、端点、输出预算和远程会话字段。摘要辅助调用沿用同一配置，因此过小的摘要输出预算也可能明确拒绝 thinking，不会悄悄降级。

未知事件、服务器工具、引用/音频等未实现块明确失败。原生 SSE 必须有协议终态，TCP EOF 不是成功；同一网络块中的先前内容不会被后续错误抹去，部分输出不自动重试。Messages 输入用量包含缓存读取/创建，Responses 不重复累加已包含的缓存量。

## 参考与验证

实现依据 [Responses 流事件](https://developers.openai.com/api/docs/guides/streaming-responses)、[工具调用](https://developers.openai.com/api/docs/guides/function-calling)、[Messages 流](https://platform.claude.com/docs/en/build-with-claude/streaming)。借鉴 Pi 的协议适配与私有块顺序保留，不复制其有损历史转换。

本包测试是受控本地 HTTP/SSE 与结构校验；实际模型参数、图片解读和真实签名须按目标端点联调。完整 Web 流程见 [测试入口](../../apps/web/test/README.md)。

必要验证：

```sh
cargo test -p providers
```
