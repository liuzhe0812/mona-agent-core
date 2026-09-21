# 两种桥接的选择与接入

## 1. 是否要同时使用

- 仅本地 Tauri：只选 Tauri bridge，不启动 HTTP 服务。
- 浏览器访问远程 Agent：只选 HTTP bridge。
- Tauri 也只是远程客户端：前端使用 HTTP client，本地无需装载 Core。
- 两种入口访问同一本机 Runtime：组合根创建一份 Host、一份 Application，再 clone Application 给两个桥接。不要各自 new 一个 Application 后误以为任务状态共享。

二者都是库组件，不是必须运行的两个进程。

## 2. HTTP Bridge

包名 `agent-bridge-http`。`router(application, HttpConfig)` 返回 Axum Router。宿主负责选择监听地址、TLS、用户认证和 graceful shutdown。示例服务只绑定环回地址。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | /v1/info | 协议版本和流模式；也需要 Bearer |
| POST | /v1/runs | StartRequest |
| POST | /v1/runs/{id}/cancel | 发取消信号 |
| POST | /v1/runs/{id}/input | InputRequest，响应可能等待安全边界 |
| GET | /v1/runs/{id}/snapshot | 当前有界视图 |
| GET | /v1/runs/{id}/result | 结果或未结束 |
| GET | /v1/runs/{id}/events | SSE；after 查询或 Last-Event-ID |
| DELETE | /v1/runs/{id} | forget，不能删除仍在运行的任务 |

所有业务路由要求 Authorization: Bearer。token 至少 32 个可见 ASCII 字符、最多 512；请随机生成并用安全配置注入。不能写入仓库、URL或日志。一个 token 对应一个受信任应用权限域；不是用户身份系统。

默认不放行跨域读取；需要浏览器另域接入时设置显式 allowed_origins，不支持 `*`。CORS不是身份认证。HTTP body 默认限制 1 MiB，Application 仍分别限制字段长度。响应 no-store；SSE 使用 event: agent、递增 id、15秒 keepalive，并设置禁用反向代理缓冲提示；代理还要按实际部署关闭缓冲和合理配置空闲超时。

客户端用 fetch 而不是带 URL token 的 EventSource。HTTPS 为远程默认；本项目 JS client 只允许 HTTPS 或 loopback HTTP。实际 Router 不自动提供 TLS，不能把客户端检查当成服务端安全措施。

## 3. Tauri Bridge

目录 `bridges/tauri`，实际 Cargo 包名 **tauri-plugin-agent-bridge**；工作区内别名 agent-bridge-tauri。

- 默认 feature 为空：可测试 ChannelBridge，不引入原生 WebView 依赖。
- 开启 `tauri`：编译真实 `init<R: Runtime>`、九个 Command、原生 ChannelSink、权限生成 build.rs。
- 用宿主的 Tauri Builder 安装插件；示例装配函数在 `examples/tauri-composition`。
- plugin 名称是 agent-bridge；前端调用路径 `plugin:agent-bridge|start_task` 等。
- capability 为受信任的本地窗口授权 `agent-bridge:default`，此外 Core bridge 还检查允许的 WebView label。示例只允许 main。

九个命令：start_task、cancel_task、send_input、get_snapshot、get_result、forget_run、subscribe_events、ack_event、unsubscribe_events。字段经 Tauri 默认 camelCase 参数映射，例如 runId、onEvent、subscriptionId、deliveryId；内部 DTO字段仍是 request_id/run_id。

前端传入 Channel 接收 ChannelPacket，处理完成后调用 ack_event。共享 JS TauriAgentClient 已封装 ACK、订阅建立的首包竞态和清理。只在确认应用完当前帧后 ACK，不能收到就立刻 ACK 然后把大量帧堆进无限本地队列。

用户关闭窗口触发 detach；宿主仍需决定是否取消任务/关闭整个应用。多 WebView 窗口应在销毁对应 WebView时调用 detach_owner；默认 Destroyed 事件按窗口 label 清理，非同名子 WebView未被及时清理时还有 ACK 超时兜底，不把此作为权限隔离保证。

Tauri 原生构建需要对应操作系统开发工具、WebView/GUI依赖。Windows可用配置的 native-tauri CI 检查；Linux headless测试通过不能代替 Windows/Tauri端到端联调。当前包未附带安装包，也没有前端页面。

## 4. 前端与业务扩展

`clients/javascript` 提供 HTTP/Tauri 两个实现，具有同一 AgentClient 方法集合和 RunView。换传输不必重写事件 reducer。使用 Tauri 客户端时由宿主显式传入 @tauri-apps/api 的 invoke 和 Channel；纯 Web 构建不因此自动依赖 Tauri SDK。

Coding diff、计划卡片、审批页面等真实业务 UI 仍需业务实现。Core 已提供工具详情 JSON 入口，不宣称拥有这些业务功能。工具侧的秘密、文件路径和原始输出应在适当层治理，再允许前端查看。


## v0.3说明

两个bridge仍可独立选择，未增加模型或业务Runtime。Rust API协议为3，但它们传输的UI帧仍为2；新ProviderData与检查点不经公共API公开。可信ApplicationConfig可以配置模型选项与Run工具上限。默认StartRequest仍是文本请求，不包含附件上传或任意模型/权限覆盖。
