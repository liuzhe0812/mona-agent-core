# mcp

可复用的 MCP 客户端扩展。协议、协商与传输直接依赖官方 Rust SDK `rmcp`；将外部能力适配成已有 Tool、ContextTransform 和 ToolSelector，不依赖 Runtime 实现、Sessions、Web 或具体模型，没有第二个 Agent 循环。

职责参考 [DSH MCP 客户端](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/mcp/mcp-client)；协议实现使用 [官方 Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)，版本固定在 Cargo.toml/Cargo.lock。不复制 DSH 的框架，也不自行实现 JSON-RPC。

## 普通接口

`Service::connect(Config, cancel)` 返回共享 `Arc<Service>`。`tools()` 提供普通工具，`plugin()` 是只负责注册工具、上下文来源和工具选择器的薄插件。宿主直接注册时仍须明确授权副作用工具；Plugin 不能自动获得宿主权限。`views()` 返回实际连接、工具和错误，`reconnect(id)` 显式重连，`shutdown()` 在所有使用者停止后关闭连接和进程。

单个服务器初始化失败只报告该服务器不可用，不发布半份目录；配置非法或整体容量超限返回错误。空配置不启动外部进程，也不新增工具。MCP Service 应由宿主共享，不随单个 Run 或 UI 模块卸载而关闭。

## 配置与权限

Config 的 `servers` 以稳定 ID 索引，最多 16 项。服务 ID 为 1–32 位字母、数字、下划线或连字符。

| 字段 | 含义 |
|---|---|
| `enabled` | 默认 false，仅宿主明确启用的服务器会连接 |
| `transport.type = stdio` | 已安装的 `command`、字符串数组 `args`、显式 `env`、可选绝对 `cwd` |
| `transport.type = streamable-http` | 完整 `url`、可选 `headers`；HTTPS 或回环 HTTP，其他 HTTP 须显式 `allow_http` |
| `timeout_ms` | 100–300000 毫秒，默认 60000；还受调用取消与 Run 总期限约束 |
| `allow_tools` / `deny_tools` | 按远端原始名称过滤；null 允许全部，[] 不提供远程工具，排除优先 |
| `read_only_tools` | 宿主确认的只读名称；不将服务器的 readOnlyHint 直接当成授权 |

stdio 清除继承环境中的 KEY/PASSWORD/SECRET/TOKEN、AGENT_/MONA_/DSH_ 项后再应用显式 env；不自动安装程序。cwd 未指定时使用宿主启动目录，不根据模型参数改成当前会话目录。stdout 专用于协议，stderr 不进入模型或宿主日志。进程回收复用 process-wrap 的 Windows Job Object / Unix 进程组。

HTTP 不跟随重定向，拒绝端点中的用户信息、query 和 fragment；凭据通过 headers 配置，不能覆盖 MCP 协议标头。仅连接可信服务器：外部进程和远端操作不受本地 Agent Sandbox 隔离，也不继承用户会话的文件根限制。

所有远端工具默认有副作用。宿主可以收紧工具白名单；`read_only_names()` 仅返回宿主明确声明的只读工具与资源工具，用于装配 Planner。标准应用的只读 Sandbox 模式另外拒绝 MCP 副作用工具，不将“工作区可写”宣传为远端目录隔离。Subagent 沿用父任务的工具上限。

## 工具、资源与输出

工具名为 `mcp__服务ID__原始名称`；不合法或过长时使用有界正规化与身份散列，调用仍使用原名。分页发现最多 32 页、1024 项，过滤后每个服务器及整体最多 64 个远端工具；重名、异常分页、外部 schema 引用和过大目录明确拒绝，不静默截断。

MCP 未声明 schema 方言时显式采用协议默认的 JSON Schema 2020-12，另支持明确声明的 2019-09 和 Draft 7。输入仍在 Runtime 的统一校验处验证，不通过校验就不发往服务器；声明 outputSchema 的结果须有符合 schema 的 structuredContent。

保留有序文本、支持的内联图像和内嵌文本资源；isError 映射为工具错误，structuredContent 保存在独立结构化结果中。资源链接只作描述，不自动访问，也不转为本机路径权限。音频、二进制资源及无效图像明确提示未支持，不伪装成文本或成功解析。

服务器具备资源能力时发布三个普通只读工具：`mcp_list_resources`、`mcp_list_resource_templates`、`mcp_read_resource`。按显式服务器、URI 和游标发现/读取，不预载全部资源。服务器 instructions 作为已选工具的有来源参考资料注入，参与现有上下文计量；不是系统权限，不覆盖用户要求，不污染正式 Transcript。

## 生命周期与限额

工具定义在 Host 装配期间固定。收到 tools/list_changed 后，该服务器旧工具停止提供并报告需重启；配置、过滤规则、指令或目录变化均需重新装配。显式重连只接纳与已装配目录相同的定义；初始化失败后的新目录也需要重启。首版不照搬 DSH 的动态注册和自动重连循环。

调用不自动重试，HTTP 过期 Session 自动重建并重发已关闭。取消、超时和调用 Future 丢弃会尽力发送 cancellation；这不证明服务器已经撤销操作。主任务的取消与总期限依然有效，外部服务自行产生的费用不计入本地模型调用预算。宿主关闭时先排空使用者，再等待共享 MCP Service 关闭。

参数上限 128 KiB、单 schema 64 KiB、指令 16 KiB、单服务目录 512 KiB、解码后响应/结果 4 MiB、结果块最多 128；HTTP SSE 单事件上限也是 4 MiB。这些不是 SDK 所有读取缓冲或进程 RSS 的完整限制，外部服务仍须可信并适当分页。

不含 OAuth、旧独立 SSE 传输、Sampling、Elicitation、Prompts、订阅、任务型工具执行或远程 UI。未装配 MCP 时基础 Agent 不受影响。

## 应用层与验证

标准 Server 默认装配 MCP 管理能力，但初始没有服务器；设置页支持两种连接、工具过滤、只读声明、真实状态和重连。保存以 revision 控制，配置重启生效。环境变量及标头值由宿主加密保存，页面只返回已保存键名；端点或程序身份改变时不能自动沿用旧凭据。

`cargo test -p mcp` 使用真实 SDK 与独立 stdio 协议 peer；`npm run test:web:mcp` 使用正式宿主、Web、stdio/HTTP peer 和受控模型验证闭环。测试脚本不属于正式入口，结果不代表所有第三方服务、真实模型或其他操作系统已通过。应用接口和存储路径见 [Server](../../apps/server/README.md)。
