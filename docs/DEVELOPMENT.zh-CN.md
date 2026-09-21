# 本地开发：一键启动 Web UI 与 Agent Runtime

## 1. 唯一标准命令

仓库根目录提供一个 Web 开发入口：

```sh
npm run dev:web
```

没有 `dev:web:demo`。该命令启动真实 Rust Runtime；首次没有模型配置时正常打开设置页，配置供应商后才能发送任务，不会切换到假模型。

## 2. 第一次使用

要求：

- Node.js 20 或更高版本。
- Rust stable 与 Cargo。

无需创建 `.env`，也无需运行 `npm install`。直接执行标准命令，浏览器打开后进入“设置 → 模型设置”，添加 OpenAI 兼容供应商、API Key 和模型。

若接入本机 HTTP 模型端点，可在启动前设置：

```dotenv
AGENT_ALLOW_HTTP_LOOPBACK=1
```

`.env` 仍可用于部署覆盖、旧模型配置导入或固定模型模式，但不是 Web UI 的默认启动条件。

## 3. 启动器实际做什么

1. 可选读取根目录 `.env`，且不覆盖调用进程已经设置的环境变量。
2. 临时生成 Bridge Bearer Token；Token 不写入 URL、localStorage 或终端日志。
3. 首次生成本机开发用模型设置加密密钥，保存在用户状态目录，不写入仓库。
4. 启动 `cargo run -p server`，默认监听 `127.0.0.1:8787`。
5. 启动仅绑定环回地址的 UI Server，默认监听 `127.0.0.1:4173`。
6. 等待 `/v1/info` 确认 Runtime 已就绪，再打开浏览器并自动连接。
7. Ctrl+C 时同时关闭 UI Server 与 Rust 进程树。

调用链仍然只有一套 Agent Runtime：

```text
Web UI -> HTTP/SSE Bridge -> AgentApplication -> Agent Core
```

Node 启动器只负责编排本地进程和开发配置，不执行 Agent 循环、工具或模型调用。

## 4. 可选设置

| 变量 | 默认值 | 含义 |
|---|---|---|
| `AGENT_SERVER_ADDR` | `127.0.0.1:8787` | Rust HTTP Bridge 监听地址；开发启动器只允许环回地址 |
| `MONA_WEB_HOST` | `127.0.0.1` | UI Server 地址；只允许环回地址 |
| `MONA_WEB_PORT` | `4173` | UI Server 端口 |
| `MONA_OPEN_BROWSER` | `1` | 设为 `0` 时不自动打开浏览器 |
| `MONA_STARTUP_TIMEOUT_SECONDS` | `300` | 等待首次 Cargo 编译与服务就绪的时间 |
| `MONA_DEV_STATE_DIR` | 用户状态目录 | 覆盖本地模型设置与开发加密密钥目录，主要用于隔离测试 |
| `AGENT_SERVER_TOKEN` | 临时生成 | 显式覆盖开发 Bridge Token，通常不需要 |
| `AGENT_MODEL_*` | 未设置 | 可选旧配置导入；页面配置无需这些变量 |

Windows 默认状态目录为 `%LOCALAPPDATA%/mona-agent-core`；Unix 默认使用 `XDG_STATE_HOME/mona-agent-core` 或 `$HOME/.local/state/mona-agent-core`。

## 5. 重要边界

- 这是开发入口，不是生产部署器；它拒绝绑定 `0.0.0.0` 或公网地址。
- UI 获取 Token 的开发配置接口只存在于环回 UI Server，并设置 `Cache-Control: no-store`。
- 首次启动允许模型目录为空；未配置默认模型时任务会明确拒绝，设置页仍可使用。
- `apps/web/index.html` 使用 ES Module，必须通过 HTTP/Tauri 宿主加载；`npm run dev:web` 会自动连接真实 Runtime。
- 远程部署应由正式宿主管理 TLS、用户认证、租户隔离和加密主密钥。
