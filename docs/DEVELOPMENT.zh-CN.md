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

无需创建 `.env`，也无需运行 `npm install`。直接执行标准命令，浏览器打开后可分别在“设置 → Agent 组件”和“设置 → Agent 工具”管理可选项，并在“模型设置”中添加 OpenAI 兼容供应商、API Key 和模型。固定基础项和部署锁定项不会显示为用户设置。

通过正式 Web 发起的会话会自动保存到用户状态目录。重启后从左侧历史列表打开并继续；未完成的旧任务会标记中断，不自动重跑。参阅 [本地会话](LOCAL-SESSIONS.zh-CN.md)。

若接入本机 HTTP 模型端点，可在启动前设置：

```dotenv
AGENT_ALLOW_HTTP_LOOPBACK=1
```

`.env` 仍可用于部署覆盖、旧模型配置导入或固定模型模式，但不是 Web UI 的默认启动条件。

## 3. 启动器实际做什么

1. 可选读取根目录 `.env`，且不覆盖调用进程已经设置的环境变量。
2. 临时生成 Bridge Bearer Token；Token 不写入 URL、localStorage 或终端日志。
3. 首次生成本机开发用模型设置加密密钥，并把能力选择与 Spill 归档放在用户状态目录中。
4. 启动 `cargo run -p server`，应用根目录的 `agent.toml`，默认监听 `127.0.0.1:8787`。
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
| `MONA_DEV_STATE_DIR` | 用户状态目录 | 覆盖开发模型设置、密钥、能力状态、Spill 和会话目录；各自的显式覆盖仍优先 |
| `AGENT_SERVER_TOKEN` | 临时生成 | 显式覆盖开发 Bridge Token，通常不需要 |
| `AGENT_CONFIG_PATH` | 根目录 `agent.toml` | 显式指定部署能力配置 |
| `AGENT_CAPABILITY_STATE_PATH` | 用户状态目录 | 覆盖最终用户的能力选择文件 |
| `AGENT_SHELL_PATH` | 自动发现 | 部署时显式指定 `shell` 工具的可执行文件；Windows 不依赖 Git Bash |
| `AGENT_BASH_PATH` | 未设置 | 旧部署兼容入口；与 `AGENT_SHELL_PATH` 同时设置时新配置优先 |
| `SHELL_PATH` | 未设置 | `packages/tools` 定向测试的 shell 可执行文件覆盖，不作为部署配置 |
| `AGENT_MODEL_*` | 未设置 | 可选旧配置导入；页面配置无需这些变量 |
| `AGENT_MODEL_CONTEXT_TOKENS` | 未设置 | 固定模型或首次环境模型导入的可选容量；模型管理页支持逐模型设置，已有保存值不被环境覆盖 |
| `AGENT_SKILLS` | 按能力配置，默认关闭 | 设为 `0` 时将 Skills 锁定为关闭 |
| `AGENT_INSTRUCTIONS` | 按能力配置，默认启用 | 设为 `0` 锁定关闭项目规则自动加载；不关闭普通 read 工具 |
| `AGENT_SKILL_DIRS` | 项目/用户 `.agents/skills` | Skills 启用后，可选自定义根目录列表，使用平台路径分隔符，替换默认根 |
| `AGENT_COMPACTION` | 按能力配置，默认启用 | 设为 `0` 时将上下文压缩锁定为关闭 |
| `AGENT_SPILL` | 按能力配置，默认启用 | 设为 `0` 时将长结果归档锁定为关闭 |
| `AGENT_SPILL_DIR` | 用户状态目录 | 覆盖长结果私有归档目录 |
| `AGENT_SESSIONS_DIR` | 用户状态目录下的 `sessions` | 覆盖宿主权限域的会话状态目录；格式 3 在每会话头保存 cwd，不再按默认 cwd 哈希分目录 |
| `AGENT_WORKSPACE_DIR` | 用户目录下 `Mona/workspaces` | 部署指定默认工作根并锁定设置；没有覆盖时可由 Web 修改，普通会话各分配子目录 |
| `AGENT_PROJECTS` | 默认启用（需编译 projects feature） | 设为 `0` 不装配项目管理，基础工作目录和文件栏保持可用 |

Windows 默认状态目录为 `%LOCALAPPDATA%/mona-agent-core`；Unix 默认使用 `XDG_STATE_HOME/mona-agent-core` 或 `$HOME/.local/state/mona-agent-core`。

正式四工具为 `read/shell/edit/write`。`shell` 的后端由宿主解析：Windows 优先 `pwsh.exe`，否则使用系统 Windows PowerShell；Linux/Unix 优先 `bash`，否则使用 `sh`。模型收到的工具描述会标明实际语法，命令必须按该语法编写。

工作文件不再默认写入本仓库。普通会话在默认根下分配独立目录；项目会话直接绑定项目目录；右侧文件栏跟随当前会话。详细用法见[工作区方案](WORKSPACES.zh-CN.md)。当前会话格式为 3，不自动迁移旧格式。保留旧开发记录时，请由开发者显式设置新的 `MONA_DEV_STATE_DIR`（独立整套开发状态）或 `AGENT_SESSIONS_DIR`（仅独立会话），不要删除原状态以绕过报错。

## 5. 重要边界

发行版包含 Skills，但新环境默认关闭。用户在“设置 → Agent 组件”显式启用并重启后，才从项目和用户的 `.agents/skills` 中发现技能；关闭时不会扫描目录或发布技能摘要。完整指南和资源通过固定 `read` 渐进加载，见 [Skills 说明](../packages/skills/README.md)。

上下文压缩和长结果归档默认启用，但只有达到阈值才工作。归档保存在用户状态目录，不写入项目工作区；关闭后恢复 Runtime 原有的明确超限行为。

- 这是开发入口，不是生产部署器；它拒绝绑定 `0.0.0.0` 或公网地址。
- UI 获取 Token 的开发配置接口只存在于环回 UI Server，并设置 `Cache-Control: no-store`。
- 首次启动允许模型目录为空；未配置默认模型时任务会明确拒绝，设置页仍可使用。
- `apps/web/index.html` 使用 ES Module，必须通过 HTTP/Tauri 宿主加载；`npm run dev:web` 会自动连接真实 Runtime。
- 远程部署应由正式宿主管理 TLS、用户认证、租户隔离和加密主密钥。
