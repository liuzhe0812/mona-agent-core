# 可选模型管理与 Web 设置

模型管理以 `plugins/agent-model-manager` 单独交付，提供供应商增删改、模型目录、显示开关、默认模型、OpenAI 兼容模型发现与加密配置存储。当前仅支持 Chat Completions；没有加入计费、图片/视频默认模型或供应商原生协议集合。

## 装配与边界

宿主创建 `ModelManager`，通过 `manager.plugin()` 安装唯一的 `agent.model` 服务，再将 `manager.runtime(engine)` 注入原有 `AgentApplication`。管理对象只保留在宿主中，不发布成 Agent Tool 或运行上下文服务。Core、通用 Application、HTTP/Tauri Bridge 和 JavaScript AgentClient 的请求协议保持原样；`StartRequest` 仍只有 `request_id` 与 `prompt`。

宿主装饰器在任务启动时固定供应商、模型、端点和凭据，并调用原有执行器。任务所有轮次继续使用这份配置，直到任务结束后释放。设置变化只影响新任务；预算、工具调度、取消和运行结果仍由 Core 管理。未安装插件时，宿主继续使用 `HostBuilder.model(...)` 注入固定模型。

## Web 宿主

`examples/server` 的默认 Cargo feature 启用模型管理。使用 `--no-default-features` 可在构建时完全排除插件；`AGENT_MODEL_MANAGEMENT=0` 可在启动时使用固定模型装配。离线 `--demo` 保持原有演示行为。

宿主管理路由位于 `examples/server/src/model_settings.rs`，路径前缀为 `/api/model-settings`。它与通用任务 Bridge 分开装配，要求 Bearer 鉴权、明确 CORS 来源、请求大小限制，并禁止响应缓存。参考宿主是单一权限域，同一 Bearer Token 拥有任务与配置权限；多用户产品应由宿主实现管理员鉴权和权限域隔离。

Web UI 连接后单独读取管理接口；没有安装该能力时显示不可用状态，不伪造可用模型。设置页提供供应商/模型搜索、编辑、密钥替换/清除、获取模型、手动录入、显示开关及默认选择。输入框的模型选择器只显示启用模型，选择后更新新任务的默认值。默认模型保持可见，删除默认供应商前需先切换默认模型。

Tauri 宿主可以直接复用管理对象实现自己的管理命令；本次没有修改通用 Tauri Bridge 或添加原生命令。

## 配置保存

首次启动从既有 `AGENT_MODEL_ENDPOINT`、`AGENT_MODEL_NAME`、`AGENT_MODEL_KEY` 和 `AGENT_MODEL_EXTRA_JSON` 导入初始供应商。已有保存文件时使用文件内容，避免每次重启覆盖页面中的设置。

| 环境变量 | 含义 |
|---|---|
| `AGENT_MODEL_STORE_KEY` | 外部提供的稳定随机加密密钥，至少 16 字节；应在首次启动前配置 |
| `AGENT_MODEL_SETTINGS_PATH` | 加密配置文件的显式路径 |
| `AGENT_MODEL_MANAGEMENT=0` | 不安装模型管理插件 |
| `AGENT_ALLOW_HTTP_LOOPBACK=1` | 允许接入本机 HTTP 模型端点 |

Windows 默认路径为 `%LOCALAPPDATA%/mona-agent-core/model-settings.enc`；Unix 使用 `XDG_STATE_HOME` 或 `$HOME/.local/state` 下的同名目录。本地开发未设置专用加密密钥时，回退使用启动环境中的 `AGENT_MODEL_KEY`。此时不要随意更换该环境变量，否则旧文件无法解密；需要轮换模型凭据时应提前采用独立稳定的加密密钥。密钥管理与轮换策略由宿主负责，本模块不实现 OS 密钥链。

配置文件采用 AES-256-GCM 加密，包括供应商凭据。页面只收到 `has_key` 状态，不读取现有明文密钥，不将密钥或 Bearer Token 放入浏览器持久存储。密钥输入留空表示保持原值，显式勾选清除才会删除。

写入通过同目录临时文件替换，保存失败时不发布新配置；错误密钥或损坏文件导致明确失败，不会自动清空。每次修改携带配置版本，过期版本返回冲突。默认文件存储面向单宿主进程，多进程共享配置需替换为具备事务能力的存储实现。

启动脚本仅管理进程和 Bridge 连接；模型管理接口、目录和凭据均不经过开发配置端点。现有开发启动要求保留；直接运行 `node scripts/dev-web.mjs` 可启动当前仓库中的开发入口。

## 验证入口

```sh
cargo test -p agent-model-manager -p agent-server-example
cargo check -p agent-server-example --no-default-features
node --test scripts/dev-web.test.mjs
node --test ui/model-settings.test.mjs
```

测试使用临时文件和本地模拟模型端点，不消耗真实模型额度。真实供应商兼容性仍需按部署目标联调。完整 Rust 集成接口见 [插件说明](../plugins/agent-model-manager/README.md)。
