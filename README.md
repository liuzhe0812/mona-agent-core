# Mona Agent Harness v0.3.0

**可嵌入的组件化 Rust Agent Harness + 统一应用层 + 可独立选择的 Tauri / HTTP 桥接。**

组件负责能力，公共接口定义边界，宿主负责组合。普通 API 与薄 Plugin 入口可以并存；Plugin 是组件接入运行时的一种方式，不要求所有组件都插件化。

本版保留唯一默认ReAct、插件框架、统一应用层和两个可选bridge，只补四类通用契约：**多模态内容/工具输出、每轮工具视图、可等待检查点、模型参数/私有协议保真**。不添加Mona业务功能，不加入JEV。

Rust插件API升级到3；UI流式协议保持2。默认仍可只用字符串和内存执行；持久化sink、业务工具和两桥接均按需装配。

## 交付状态

当前工作树已完成 Rust workspace 测试、文档测试、服务无默认 feature 编译、Tauri 原生 feature 编译，以及 JavaScript/TypeScript 和 Web 启动器检查。隔离环境中的 `npm run dev:web` 可启动并打开空模型设置页。具体范围见[交付报告](docs/DELIVERY.zh-CN.md)与 `verification/`。

真实付费供应商流、原生 Tauri 窗口交互、高并发和生产认证/租户策略仍需在目标环境联调；本地模拟端点和 mock 测试不替代这些验收。

## 1. 结构与依赖

```text
                 业务界面 / 其他调用者
                 /                 \
      Tauri IPC + Channel       HTTP + SSE
                 |                 |
        Tauri Bridge            HTTP Bridge        ← 分别选择，不互相依赖
                 \                 /
                  application                 ← 统一任务接口、回放、保留策略
                           |
                      api                     ← AgentRuntime / RunSession / 事件协议
                           ↑ 实现
                      runtime                    ← 唯一默认 ReAct 执行器
                    /            \
           模型适配器             可选组件 / 薄 Plugin 入口
```

箭头中“实现”和“调用”含义不同：`application` 调用 `Arc<dyn AgentRuntime>`，**生产依赖不包含 `runtime`**。业务组合根决定把哪个 Runtime 实现注入进来。两个桥接的生产依赖都不包含 Core、provider 或对方桥接。

| 包/目录 | 职责 | 使用方式 |
|---|---|---|
| `packages/api` | 消息、工具、插件协议、公开流式运行契约 | 所有内层实现共用 |
| `packages/runtime` | 默认 ReAct、工具执行、状态、插件生命周期 | 可直接用于 CLI/嵌入式宿主 |
| `packages/providers` | HTTP/SSE 模型适配器 | 使用真实模型时装配 |
| `packages/models` | 模型配置、目录、默认选择与路由 | 可选；需要管理页面时装配 |
| `packages/application` | start/cancel/input/subscribe/snapshot/result/forget | 需要界面或远程入口时选用 |
| `packages/tauri-bridge` | Tauri 2 插件、Command、Channel + ACK | 本机桌面，不启动 HTTP 服务 |
| `packages/http-bridge` | Axum 路由、Bearer 校验、HTTP + SSE | 浏览器、远程客户端 |
| `packages/client` | 共用协议类型、RunView、两个客户端适配器 | 不绑定 React/Vue，不包含 UI |
| `packages/memory`、`planner` | 可选能力组件 | 可直接调用，也可通过薄 Plugin 接入；Planner 仍是上层编排 |
| `apps/server` | HTTP 服务组合根 | 正式应用入口；认证、租户与部署策略由产品宿主负责 |
| `apps/web` | 正式通用 Web UI | 通过宿主接口使用已装配能力 |
| `examples/tauri-composition` | Tauri Rust 宿主装配函数与 capability 样例 | 不是完整桌面产品/安装包 |

`apps/` 放正式应用入口，`examples/` 放可运行的接入与组合样例。Rust 包使用简短目录名；目录与 crate 名一致，`packages/tauri-bridge` 的 Cargo 名例外为 `tauri-plugin-bridge`，工作区别名仍为 `tauri-bridge`。JavaScript 客户端包名为 `client`。

## 2. 如何选择，不把依赖全部带进应用

- **只要 Agent Core：**依赖 API/Core 和需要的模型/工具。应用层、两个桥接都不需要。
- **Tauri 本地应用：**API/Core + Application + Tauri Bridge。开启桥接的 `tauri` feature；不依赖 HTTP Bridge/Axum。
- **浏览器/远程服务：**API/Core + Application + HTTP Bridge。不依赖 Tauri、WebView 或其平台库。
- **两种入口同时存在：**把同一个 `AgentApplication` 克隆给两个桥接，任务注册表、幂等键与事件投影都是同一份。

`Cargo.toml` 的 default-members 排除桥接和服务组合示例。显式 `cargo check --workspace` 会检查两种桥接，但 Tauri **原生绑定仍需要单独的 feature 检查**。工作区解析锁文件可能读取可选依赖的元数据；“可选”指不编译、不链接进所选应用，不承诺完全零元数据访问。

## 3. 本版流式能力

- 公共 `AgentRuntime.start` 返回 API 层的 `RunHandle`；不再要求 UI/bridge 导入 `Engine` 类型。
- 独立 `RunSession` 支持取消、追加输入、快照、订阅、等待最终报告。
- 工作项有稳定 ID；文本/工具参数/工具日志分别增量；完成事件携带权威的完整**有界 UI 状态**，而不是要求前端拼凑最终真相。
- 工具参数增量只用于展示，**绝不边生成边执行**。
- 事件带协议版本、run_id、递增序号。应用层提供有界内存回放；游标失效时明确发快照，客户端以快照替换旧投影。
- ToolProgress 可附带命名空间 JSON 详情，例如 `coding.diff`；内核不理解具体业务含义，不内置代码 diff/计划业务。
- HTTP：命令走 HTTP，流走 SSE。Tauri：Command + Channel，逐包 ACK 限制在途 IPC 消息。
- 中断连接/关闭订阅**不取消任务**；停止按钮调用独立 cancel。

设计借鉴 Codex 的 item 生命周期，但这**不是 Codex app-server 的兼容实现**，没有复刻 JSON-RPC、thread/session、审批回调、隐藏思维展示或完整 Coding Agent。

## 4. 本版通用能力与边界

| 通用能力 | 本次源码实现 | 明确保留在外部 |
|---|---|---|
| 内容/工具输出 | Text/Image/Resource描述、structured结果、明确is_error、媒体不静默截断 | 文件解析/采集/存储；默认Provider尚不解析Resource |
| 工具视图 | Run可信上限 + 每轮ToolSelector，执行时二次核对本轮集合 | 能力组/MCP发现/订阅规则；无热加载 |
| 检查点 | optional CheckpointSink、按Run串行提交、派发前intent、部分结果、失败停止 | 数据库、聊天Session、自动恢复和exactly-once |
| 模型协议 | 有界ModelOptions、命名空间ProviderData、参数白名单与回传验证 | 厂商专用适配、凭据管理、真实模型兼容性 |

私有协议、内嵌图片和structured结果不直接流向UI。默认桥接仍是文本prompt入口；多模态界面由可信应用完成附件授权与RunRequest构造。这个版本不是Mona的直接替换包。

[本轮通用性判定](docs/GENERIC-CORE-BOUNDARY.zh-CN.md)解释为什么只补这些接口；[迁移文档](docs/MIGRATION-0.3.zh-CN.md)列出Rust破坏性变更。

## 5. 验证入口

可选的[模型管理与 Web 设置](docs/MODEL-MANAGEMENT.zh-CN.md)由 `packages/models` 组件和宿主接口提供；未装配时仍可直接注入固定模型，不改变通用任务协议。

需要 Rust stable + cargo/rustfmt/clippy。第一次解析依赖需要网络；首次成功生成的 Cargo.lock 应保留并在产品发布时固定工具链。

```sh
bash scripts/verify.sh                       # 非原生 Rust 全工作区 + JS 客户端测试
bash scripts/verify.sh --native-tauri        # 额外检查原生 Tauri；需平台开发库
```

Windows：`scripts/verify.ps1`；原生检查加 `-NativeTauri`。

也可以独立选择包：

```sh
cargo check -p runtime
cargo run -p demo --bin generic_extensions         # 离线通用接口示例，不是视觉模型测试
cargo test -p application
cargo test -p http-bridge
cargo test -p tauri-plugin-bridge                  # 无 WebView 的 Channel 逻辑
cargo check -p tauri-plugin-bridge --features tauri # 真正的原生插件绑定
cargo check -p tauri-composition --features tauri
node --test packages/client/test/*.test.mjs
```

脚本不会调用付费模型，也不会自动启动桌面窗口。CI 另外配置 Windows 原生 Tauri 检查；**配置工作流不代表它已运行成功**。

## 6. HTTP 演示

`server` 在 `127.0.0.1:8787` 监听。必须通过环境变量传入随机生成、至少 32 字符的 `AGENT_SERVER_TOKEN`，没有默认密码。

正式通用 Web UI 的本地开发入口是 `npm run dev:web`；它自动生成本机 Bridge Token 和设置存储密钥，无需 `.env`。首次模型供应商在设置页配置。

`cargo run -p server -- --demo` 使用明确标记的离线脚本模型，演示工具调用、工具进度和逐字输出；不是实际 LLM。去掉 `--demo` 时配置 `AGENT_MODEL_ENDPOINT`、`AGENT_MODEL_NAME`、`AGENT_MODEL_KEY`。`.env.example` 仅作为说明，不会自动加载。

远程部署由宿主提供 TLS、身份认证和权限域隔离，不应直接把演示端口暴露到公网。Tauri 本地版本不需要运行这个服务。

## 7. 进一步阅读

- [Packages 目录与组件边界](packages/README.md)
- [Apps 与 Examples 目录](apps/README.md)
- [设计架构](docs/ARCHITECTURE.zh-CN.md)
- [通用Core边界与本次取舍](docs/GENERIC-CORE-BOUNDARY.zh-CN.md)
- [内容与模型协议](docs/CONTENT-AND-PROVIDERS.zh-CN.md)
- [可等待检查点](docs/CHECKPOINTS.zh-CN.md)
- [v0.2 → v0.3 迁移](docs/MIGRATION-0.3.zh-CN.md)
- [公共流式协议与 Codex 借鉴边界](docs/STREAMING-PROTOCOL.zh-CN.md)
- [统一应用接口](docs/APPLICATION-API.zh-CN.md)
- [Tauri / HTTP 接入指南](docs/BRIDGES.zh-CN.md)
- [JavaScript 客户端](packages/client/README.md)
- [v0.1 → v0.2 迁移](docs/MIGRATION-0.2.zh-CN.md)
- [长期设计方案](docs/DESIGN-NOTES.zh-CN.md)
- [插件规范](docs/PLUGIN-GUIDE.zh-CN.md)
- [安全与部署边界](docs/SECURITY.zh-CN.md)
- [验收矩阵](docs/ACCEPTANCE.zh-CN.md)、[测试清单](docs/TEST-INVENTORY.md)

**原生插件可信但不是沙箱；取消不等于撤销；有界内存回放不是崩溃恢复；流式结束不等于业务目标已经验收成功。**
