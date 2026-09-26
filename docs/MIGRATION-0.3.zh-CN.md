# v0.2 → v0.3 迁移

## 1. 三个版本分开

本页记录 0.3 首次交付时的 Rust 插件 API_VERSION=3；当前版本以[架构入口](README.zh-CN.md)集中列出的源码常量为准。API 4 的上下文预算字段见 [Rust 插件 API 4 迁移](MIGRATION-API-4.zh-CN.md)，API 5 的工具可见性字段见 [Rust 插件 API 5 迁移](MIGRATION-API-5.zh-CN.md)，API 6 的可靠性接口见 [Rust 插件 API 6 迁移](MIGRATION-API-6.zh-CN.md)。UI 与检查点版本也独立维护。

这是Rust源码接口的破坏性升级，需重新构建插件/组合根；不是动态ABI兼容发行。已有HTTP/SSE与Tauri Channel客户端的帧结构不变，不因Rust API升级而要求UI改成protocol_version=3。

## 2. 必需的源码调整

- Tool.execute从Result<String>改为Result<ToolOutput>。普通字符串可通过.into()包装；已知业务失败用ToolOutput::error，富结果填写content/structured/artifact。
- Message::User.content与ToolResult.content由String改为Content。读取正文用.text()返回Cow<str>；需要拥有的String用.into_owned()；估算内容大小用.byte_len()。
- ToolCall新增可选provider_data，优先用ToolCall::new。Assistant和ModelReply增加可选provider_data。
- ModelRequest增加options: ModelOptions。用默认值保持旧参数行为；新参数不得绕过max_output_tokens。
- RunRequest增加allowed_tools、model_options；RunRequest::new仍是简洁默认入口。
- RunReport增加checkpoint。公共UI用RunOutcome和UiToolResult，不再直接拿模型层ToolResult作为UI结果。
- 自定义Registrar实现需增加tool_selector/checkpoint_sink。使用默认HostBuilder的项目无须实现Registrar。
- ModelEvent完整match需处理ProviderData；应当保留给模型回传，而不是拼进可见文本。
- PlanRequest新增可信工具上限与模型选项；示例Planner向所有子运行传递这些设置。

包内现有Memory/Planner、例子、两桥接组合根及测试代码已同步调整。

## 3. 可选能力的启用

不用多模态，继续用字符串；不用动态工具视图，allowed_tools=None且不注册selector；不用持久化，不注册CheckpointSink；不用私有选项和回传数据，相关字段为None。默认ReAct没有引入数据库、Python/Node或新的后台服务。

ToolSelector可由插件注册；只返回给定available中的名字。每轮从可信上限重新计算，同轮多selector单调收紧，重复或未知名字报错。实际工具执行仍经过全部旧安全检查。

CheckpointSink的语义请先阅读[检查点文档](CHECKPOINTS.zh-CN.md)，尤其不能把UI观察器当可靠提交，也不能把有快照当已实现恢复。

## 4. Provider迁移

每个适配器应明确自己支持哪些内容块与选项，不支持则返回Unsupported。现有默认Chat适配器增加用户图片和工具图片投影；资源引用需外部解析。

需要保存模型私有数据时，输出带namespace的完整ProviderData快照，区分Assistant与ToolCall索引。发送下一轮时按相同Provider协议恢复；不同Provider/模型不能盲目混用签名。

ChatConfig新增字段均有构造器默认值，建议使用ChatConfig::new而不是完整结构体字面量。temperature/top_p/stop不再通过extra_body覆盖，使用ModelOptions；endpoint、凭据、允许模型和私有字段白名单由宿主配置。

## 5. Application与桥接

ApplicationConfig增加可信allowed_tools/model_options默认值，默认不改变旧行为。对外StartRequest仍只有request_id/prompt，不能从任意前端注入模型密钥/工具权限。

默认桥接不包含附件上传、资源授权或Mona会话。需要多模态UI时由应用层把授权资源转换为Content，再构造RunRequest或另外提供受控的产品入口；不能往prompt塞Base64代替图片协议。

UI结果结构仍是文本预览，富数据不会自动渲染成图片。应用可通过受授权资源服务和已有工具详情展示；完成项继续覆盖旧增量，不重复拼接。

## 6. 验收

运行scripts/verify.sh或verify.ps1；新增generic_extensions离线示例。验证入口包含Rust单元/集成测试、可选原生Tauri检查、JS客户端。真实图片模型、生产持久化sink和Mona迁移均另行联调。

本次交付环境没有Rust编译器，源码兼容性修改和测试文件不代表已成功编译；详见[交付报告](DELIVERY.zh-CN.md)。
