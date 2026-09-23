# 内容、工具输出与模型协议 v0.3

## 1. 内容模型

Content 支持两种序列化形态：原有字符串，以及有序的 ContentBlock 数组。字符串调用可以继续使用 Message::user("...") / RunRequest::new("...")。

内容块仅包含三类：Text、Image、Resource。图片可为 Base64 或 HTTPS URL；Resource 是应用拥有的定位符、媒体类型和名称，不是文件内容。数组顺序保留，纯文本提取不会把 Base64/路径伪装成视觉信息。

本版覆盖用户多模态输入和工具多模态结果；Assistant 的普通生成正文仍是文本，工具调用独立保留。没有原生语音/视频实时流或模型生成图片协议。通用资源引用提供接入位置，不声称默认适配器已经能读取 PDF/音视频。

验证：最多128块；支持声明 PNG/JPEG/WebP/GIF；Base64检查编码字符、填充和尾部位，不做图片解码、分辨率或恶意文件检测。URI/元数据长度有限。宿主仍要做文件真实性、大小、敏感内容、权限和网络访问检查。

## 2. 工具结果

Tool.execute 返回 Result<ToolOutput>，不再只能返回字符串。ToolOutput 包含：content、可选 structured JSON、可选 artifact，以及 is_error。

- Ok(ToolOutput::new(...)) 表示工具报告成功。
- Ok(ToolOutput::error(...)) 表示已知业务错误，保留结构化内容，模型可以根据错误继续决策。
- Err 表示执行异常；Core 仍区分普通 Error 与取消/超时/panic 后可能的 Unknown。
- Denied、Skipped、Unknown 由执行器决定，不允许工具直接伪造调度结果。

Core 生成 call_id/状态关联，ResultTransform 不得篡改身份和状态。structured 是模型可见的结果数据，不是自动公开的 UI 元数据；需要校验业务数据结构时由工具/结果插件负责，本版没有新增强制输出 Schema 框架。

## 3. 大小与失败语义

默认原始历史接纳上限 `max_initial_history_bytes=4MiB`，最终模型请求 `max_context_bytes=256KiB`，工具结果 `max_tool_result_bytes=64KiB`。较大历史可以先经过上下文变换，但任何实际模型请求仍必须满足较小的硬限制。这些是保守的文本型默认值，并不适合任意截图。图片应用应显式设置合理预算，或在外部缩放/裁剪/归档。

普通长文本仍采用有标记、UTF-8安全截断。包含图片、structured或附件描述的富结果超过上限时，不截断Base64/JSON后假装可用：保存明确的有限预览，运行以 Limited 停止，不发送破损媒体到下一轮模型。ResultTransform 在此之前有机会归档、替换为受支持引用或减小图片。

原始长度计入内容、structured和附件描述。Core 没有文件归档服务，truncated/ArtifactRef不能被解读为“完整原文已备份”。原生插件本身不是内存沙箱；它创建超大对象的成本不可能在返回后自动撤销。

## 4. 当前 Chat Completions 适配器

- 用户文本保持字符串；用户图片转换为 image_url 内容块，Base64变成 data URI。
- HTTPS图片URL默认关闭，必须由 ChatConfig.allow_image_urls 显式启用；拒绝嵌入用户名/密码或片段的URL。Provider可能自行访问该地址，宿主必须审查URL访问与隐私，Core不会自动下载它。
- 工具文本/structured以包含状态的JSON文本信封返回。
- 工具图片转换成工具结果批次之后的一条用户角色“工具观察”内容消息，保留call_id标签；不打断连续tool消息，不放入system角色。这是本适配器的明确转换策略，不是所有厂商的原生工具图片协议；真实端点须验证。
- Resource 未被解析时明确报 Unsupported；不会偷偷联网、读取任意路径或忽略内容。需要文件输入的模型应增加专用适配器或可信上下文转换器。
- HTTP/SSE 失败按鉴权、额度、限流、服务端、确定性请求错误、上下文超限和一般传输故障分类；错误正文只用于有界分类，不回传 Prompt、凭据或供应商原文。HTTP 状态和有效 `Retry-After` 作为结构化事实保留。
- `ChatConfig.context_window_tokens` 可声明当前路由的上下文容量；未知时保持空值，Runtime 继续使用请求字节硬限制。

API能表示图片不等于任何指定模型都具备视觉能力；宿主选用支持对应能力的模型并完成联调。

### Responses / Messages

两种原生协议通过相同 Model 接口接入。Responses 使用本地 input、function_call/function_call_output、store:false 和加密推理回传；Messages 使用 system、tool_use/tool_result 及签名 thinking 内容块。模型发现、协议参数与三态能力由模型管理配置，不加入核心执行机制。

私有信封 `{route, items}` 保留原始 output/content 顺序；回传前校对可见正文和工具身份，不转换其他协议的签名。图片使用各协议的原生块，Resource 必须由显式解析组件提供内容。具体参数、大小限制和未实现内容统一见 [Providers](../packages/providers/README.md)。

## 5. 参数与模型选择

RunRequest.model_options 是本次运行默认值；辅助调用通过 RunContext.model 发出的 ModelRequest.options 可明确覆盖字段，否则继承默认值。

公共字段：model、temperature、top_p、stop；max_output_tokens仍使用原来的预算受控字段，不能通过通用选项绕过。

provider_options 是带命名空间的完整私有选项对象。默认 Chat 适配器只接受宿主预先白名单的字段；model/messages/tools/token限制/凭据/端点等保留字段不能通过该对象覆盖。单次模型覆盖也只能选 ChatConfig.model 或 allowed_models 中的值。凭据和端点始终是宿主配置，不是前端请求参数。

参数的类型/范围检查不是模型能力发现；不支持这些参数的端点仍可能明确拒绝请求，必须做真实服务测试。

## 6. 私有协议回传

ProviderData = namespace + JSON value。它可以附着到完整Assistant消息或某个ToolCall；ModelEvent::ProviderData携带完整替换快照，不是字符串/JSON增量补丁。数组顺序、JSON值和签名字符串保留；Core不解释、总结或改写其含义。

当前 Chat 适配器需要宿主配置消息/工具级回传字段白名单。它收集声明为完整值的字段；碎片签名、特殊内容块等需专门适配，不能把片段当完整值。当前 ProviderData.value 是 `{route, fields}`：route 绑定实际模型/端点/命名空间/私有配置，fields 保留白名单字段。reasoning_content 只使用文本聚合载体，结束时附小型来源信封，不在 fields 里重复保存推理文本。

历史预检要求命名空间和路由指纹一致、字段受支持，并拒绝覆盖 role/content/tool_calls/id/function/arguments 等控制字段。无来源推理或外来私有数据返回 ModelHistoryIncompatible，不转换、不静默丢弃。发送时只还原 fields 和独立推理文本，route 不进入供应商请求。指纹不含 API Key 或运行/设置 revision，同一路由重启和换密钥仍可用。

这是JSON语义与字段值保真，不是原始HTTP字节保真：对象键排序/转义可能规范化；若厂商要求原始签名字节，应由专用适配器将其保存为不透明字符串并按该协议恢复。不同模型之间的回传兼容性也由适配器/宿主负责，不是一个同名字段就保证兼容。

ContextTransform 应按完整消息/工具配对处理，不得修改签名正文后伪称原签名仍有效。默认 Compaction 保护带私有数据的完整组，不用摘要隐藏来源。实际启动在任何变换之前校验原工作历史，详细行为见[模型切换](MODEL-MANAGEMENT.zh-CN.md#长会话中切换模型)。

## 7. UI和安全边界

RunReport、RequestAudit、RunCheckpoint供可信Rust调用者或存储端使用，可能包含隐私数据，不能直接放进公共bridge。

UI 采用独立 UiToolResult，提供有界文本及安全富内容投影；私有推理、ProviderData、任意 structured 和路径凭据不直接回传。当前内嵌图片和受限引用规则以 [API 文档](../packages/api/README.md) 为准。显式 set_detail 只提供经过宿主审查的展示信息，不承担执行或回传协议。

这不是文本脱敏器。普通文本、工具参数、日志及显式artifact仍可能敏感，应用必须继续做权限、脱敏、URL校验与转义。

HTTP/Tauri默认StartRequest仍是request_id+prompt，不新增任意路径、模型凭据或权限的前端入口。附件上传、资源授权和多模态请求组装由产品应用层完成，或可信调用者直接使用RunRequest。

## 8. 重试、审计与上下文恢复

模型重试由 Runtime 的统一网关执行，Provider 不私自重试。默认关闭；启用后只处理尚未产生模型内容的临时传输、限流和服务端故障。每次尝试都占用模型调用预算并生成独立审计项，退避等待服从取消和任务期限。鉴权、额度、协议错误、截断及已产生部分输出的失败不自动重试。

`AuditMode::Full` 保留完整请求；`AuditMode::Metadata` 明确省略请求正文，只保留请求大小、结算、用量和错误。两者都属于可信 Rust 报告，不直接进入公共 UI。

供应商确认上下文超限后，Runtime 允许 ContextTransform 管道尝试一次恢复；只有投影严格缩小且仍满足字节上限才重新请求。具体摘要策略仍在 Compaction 组件，不进入 Core。
