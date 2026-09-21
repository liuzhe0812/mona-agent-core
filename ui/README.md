# Mona Agent Web UI

这是 Mona Agent Core 的标准参考 Web UI。它使用与 HTTP Bridge / Tauri Bridge 相同的流式工作项协议，但本身不实现 Agent Runtime。

## 一键连接真实 Runtime

在仓库根目录准备 `.env` 后运行：

```sh
npm run dev:web
```

启动器会同时启动：

- Rust `agent-server-example`：真实模型模式，默认 `127.0.0.1:8787`。
- Web UI Server：默认 `127.0.0.1:4173`。
- 浏览器中的 UI 会自动使用 HTTP/SSE Bridge 连接，不需要手工复制 Bridge Token。

没有 `npm run dev:web:demo`。配置缺失会报错，不会退回假模型。完整说明见 [本地开发文档](../docs/DEVELOPMENT.zh-CN.md)。

## 单独使用界面

标准入口是 [index.html](index.html)。它使用原生 ES Module，因此需要通过 HTTP Server 或 Tauri WebView 加载，不能依赖 `file://` 直接双击。它包含连接设置：

- Web / 远程场景选择 HTTP Bridge。
- Tauri 宿主选择 Tauri Bridge，不需要本地 HTTP Server。
- `npm run dev:web` 会在开发环境自动完成 HTTP 连接。

## 与 Core 的边界

```text
Web UI
  -> JavaScript AgentClient / RunView
  -> HTTP 或 Tauri Bridge
  -> AgentApplication
  -> Agent Core
```

UI 负责渲染文本增量、工具工作项、最终状态和取消操作；Core 负责 ReAct、模型、工具、权限、取消和资源限制。关闭订阅不等于取消任务，停止按钮必须调用独立的 cancel 接口。

## 当前交付边界

- `index.html`、`app.mjs`、`styles.css` 是可直接修改的参考界面，不是完整产品账号/Session/工作区系统。
- 模型设置通过独立的 HTTP 管理接口工作；页面可以提交用户新输入的凭据，但已保存的模型密钥不会返回浏览器，也不会写入 `localStorage`。浏览器拿到的是本机开发 Bridge Token。
- Tauri Bridge 当前只支持聊天能力；模型设置页会如实提示宿主尚未接入管理接口。
- 直接部署到公网前，需要把这套模块化 UI 纳入正式前端构建与认证体系。
- 当前仓库未把 UI 发布为 npm 包；调用协议客户端位于 `clients/javascript/`。
