# 本地开发：一键启动 Web UI 与 Agent Runtime

## 1. 唯一标准命令

仓库根目录只提供一个 Web 开发入口：

```sh
npm run dev:web
```

没有 `dev:web:demo`。该命令始终启动真实的 Rust Runtime 和模型适配器；模型配置缺失时会立即报错，不会悄悄切换到假模型。

## 2. 第一次使用

要求：

- Node.js 20 或更高版本。
- Rust stable 与 Cargo。
- 可访问的 OpenAI-compatible `/chat/completions` 模型端点。

复制配置文件：

```sh
cp .env.example .env
```

Windows PowerShell：

```powershell
Copy-Item .env.example .env
```

至少填写：

```dotenv
AGENT_MODEL_ENDPOINT=https://YOUR_MODEL_HOST/v1/chat/completions
AGENT_MODEL_NAME=YOUR_MODEL_ID
AGENT_MODEL_KEY=YOUR_MODEL_SECRET
```

本地 HTTP 模型端点还需要：

```dotenv
AGENT_ALLOW_HTTP_LOOPBACK=1
```

然后运行：

```sh
npm run dev:web
```

不需要 `npm install`：根启动器只使用 Node 内置模块。

## 3. 启动器实际做什么

1. 从根目录 `.env` 读取尚未由系统环境设置的变量。
2. 临时生成 Bridge Bearer Token；Token 不写入 URL、localStorage 或终端日志。
3. 启动 `cargo run -p agent-server-example`，默认监听 `127.0.0.1:8787`。
4. 启动仅绑定环回地址的 UI 服务器，默认监听 `127.0.0.1:4173`。
5. 等待 `/v1/info` 确认 Runtime 已就绪。
6. 打开浏览器，并让标准 UI 自动选择 HTTP/SSE Bridge。
7. Ctrl+C 时同时关闭 UI Server 和 Rust 进程树。

调用链仍然只有一套 Agent Runtime：

```text
Web UI -> HTTP/SSE Bridge -> AgentApplication -> Agent Core
```

Node 启动器只是本地进程管理器，不执行 ReAct、工具或模型调用。

## 4. 可选设置

| 变量 | 默认值 | 含义 |
|---|---|---|
| `AGENT_SERVER_ADDR` | `127.0.0.1:8787` | Rust HTTP Bridge 监听地址；开发启动器只允许环回地址 |
| `MONA_WEB_HOST` | `127.0.0.1` | UI Server 地址；只允许环回地址 |
| `MONA_WEB_PORT` | `4173` | UI Server 端口 |
| `MONA_OPEN_BROWSER` | `1` | 设为 `0` 时不自动打开浏览器 |
| `MONA_STARTUP_TIMEOUT_SECONDS` | `300` | 等待首次 Cargo 编译与服务就绪的时间 |
| `AGENT_SERVER_TOKEN` | 临时生成 | 显式覆盖开发 Bridge Token，通常不需要 |

## 5. 重要边界

- 这是开发入口，不是生产部署器；它拒绝绑定 `0.0.0.0` 或公网地址。
- UI 获取 Token 的开发配置接口只存在于环回 UI Server，并设置 `Cache-Control: no-store`。
- `ui/index.html` 使用 ES Module，必须通过 HTTP/Tauri 宿主加载；`npm run dev:web` 会自动连接真实 Runtime。
- 远程部署应分别构建前端和 Agent Server，并由 TLS、用户认证、租户隔离及密钥系统负责安全。
