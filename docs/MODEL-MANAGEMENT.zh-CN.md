# 可选模型管理与 Web 设置

模型管理以 `packages/models` 单独交付，提供供应商增删改、模型目录、显示开关、默认模型、OpenAI 兼容模型发现与加密配置存储。当前仅支持 Chat Completions；没有加入计费、图片/视频默认模型或供应商原生协议集合。

## 装配与边界

宿主创建 `ModelManager`，需要运行时注册时通过薄入口 `manager.plugin()` 安装唯一的 `agent.model` 服务，再将 `manager.runtime(engine)` 注入原有 `AgentApplication`。管理对象只保留在宿主中，不发布成 Agent Tool 或运行上下文服务。Core、通用 Application、HTTP/Tauri Bridge 和 JavaScript AgentClient 的请求协议保持原样；`StartRequest` 仍只有 `request_id` 与 `prompt`。

宿主装饰器在任务启动时固定供应商、模型、端点和凭据，并调用原有执行器。任务所有轮次继续使用这份配置，直到任务结束后释放。设置变化只影响新任务；预算、工具调度、取消和运行结果仍由 Core 管理。未安装插件时，宿主继续使用 `HostBuilder.model(...)` 注入固定模型。

## Web 宿主

`apps/server` 的默认 Cargo feature 启用模型管理。使用 `--no-default-features` 可在构建时完全排除插件；`AGENT_MODEL_MANAGEMENT=0` 可在启动时使用固定模型装配。离线 `--demo` 保持原有演示行为。

宿主管理路由位于 `apps/server/src/model_settings.rs`，路径前缀为 `/api/model-settings`。它与通用任务 Bridge 分开装配，要求 Bearer 鉴权、明确 CORS 来源、请求大小限制，并禁止响应缓存。参考宿主是单一权限域，同一 Bearer Token 拥有任务与配置权限；多用户产品应由宿主实现管理员鉴权和权限域隔离。

Web UI 连接后单独读取管理接口；没有安装该能力时显示不可用状态，不伪造可用模型。设置页提供供应商/模型搜索、编辑、密钥替换/清除、获取模型、手动录入、显示开关及默认选择。输入框的模型选择器只显示启用模型，选择后更新新任务的默认值。默认模型保持可见；删除默认供应商时自动选择剩余供应商的首个启用模型，没有可用模型时清空默认值。

Tauri 宿主可以直接复用管理对象实现自己的管理命令；本次没有修改通用 Tauri Bridge 或添加原生命令。

## 长会话中切换模型

普通历史可继续交给新模型。含模型专属回传字段的历史，必须匹配实际模型、端点、供应商命名空间及私有配置。新轮次保存前检查一次，启动绑定后再检查一次；不兼容时显示“当前会话包含原模型专属历史，不能直接使用所选模型。请新建会话”，不自动新建、不静默删除字段、不回退原模型。普通预检拒绝不增加轮次或修改原会话；设置恰在接纳和启动之间变化时，已接纳输入可能保存为失败，但没有模型/摘要/工具执行。

同路由重启和替换 API Key 不影响回传身份。指纹不是服务器模型版本证明，也不能推断视觉能力等其他协议支持；未纳入已知规则的原生协议转换仍不支持。执行中的 Run 继续使用启动时的配置。

## 上下文窗口

每个 `ModelEntry` 可保存 `context_window_tokens`（可选，1–1,000,000,000 整数）。模型列表的窗口按钮及新增模型弹窗提供设置入口；留空表示未知，不依据同名模型或供应商猜测。通用模型发现只返回 ID，刷新列表和切换可见性保留既有窗口。

模型绑定会固定对应窗口，修改只影响新 Run。旧 JSON 无该字段时按 None 读取；首次环境导入可读取 `AGENT_MODEL_CONTEXT_TOKENS`，不覆盖已保存设置。运行时按实际绑定模型预留输出并近似判断压力，字节保护仍独立生效，详见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)。

## 配置保存

首次 `npm run dev:web` 允许配置为空，用户从设置页添加供应商。若启动环境同时提供 `AGENT_MODEL_ENDPOINT` 和 `AGENT_MODEL_NAME`，则在尚无保存配置时导入为初始供应商；已有保存文件时不会被环境变量覆盖。

| 环境变量 | 含义 |
|---|---|
| `AGENT_MODEL_STORE_KEY` | 生产宿主可注入的稳定随机加密密钥；开发启动器未提供时自动生成 |
| `AGENT_MODEL_SETTINGS_PATH` | 加密配置文件的显式路径 |
| `AGENT_MODEL_MANAGEMENT=0` | 不安装模型管理插件 |
| `AGENT_ALLOW_HTTP_LOOPBACK=1` | 允许接入本机 HTTP 模型端点 |

Windows 默认路径为 `%LOCALAPPDATA%/mona-agent-core/model-settings.enc`；Unix 使用 `XDG_STATE_HOME` 或 `$HOME/.local/state` 下的同名目录。`npm run dev:web` 在同一状态目录生成 `model-store.key` 并复用，用户无需手工管理。生产宿主仍应从自己的密钥系统注入稳定主密钥；本模块不实现 OS 密钥链或密钥轮换。

配置文件采用 AES-256-GCM 加密，包括供应商凭据。页面只收到 `has_key` 状态，不读取现有明文密钥，不将密钥或 Bearer Token 放入浏览器持久存储。密钥输入留空表示保持原值，显式勾选清除才会删除。

写入通过同目录临时文件替换，保存失败时不发布新配置；错误密钥或损坏文件导致明确失败，不会自动清空。每次修改携带配置版本，过期版本返回冲突。默认文件存储面向单宿主进程，多进程共享配置需替换为具备事务能力的存储实现。

启动脚本管理进程、Bridge 连接和本地开发密钥引导；模型目录和供应商凭据不经过开发配置端点。标准入口是 `npm run dev:web`，内部执行 `node scripts/dev-web.mjs`。

## 验证入口

```sh
cargo test -p models -p server
cargo check -p server --no-default-features
node --test scripts/dev-web.test.mjs
node --test apps/web/model-settings.test.mjs
```

测试使用临时文件和本地模拟模型端点，不消耗真实模型额度。真实供应商兼容性仍需按部署目标联调。完整 Rust 集成接口见 [插件说明](../packages/models/README.md)。
