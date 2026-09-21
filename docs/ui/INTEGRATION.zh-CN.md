# Mona Agent Web UI 接入

## Web

准备根目录 `.env` 后：

```sh
npm run dev:web
```

该命令只面向本机开发。远程 Web 产品应使用正式静态构建、HTTPS、账户认证和独立部署的 HTTP Bridge。

## Tauri

Tauri 宿主复用同一组 `ui/index.html`、`ui/app.mjs`、`ui/styles.css`。Rust 端安装 `tauri-plugin-agent-bridge`；前端宿主向页面提供 `invoke` 与 `Channel` 后，UI 会选择 `TauriAgentClient`，不启动本地 HTTP 服务。

如果 Tauri 仅作为远程客户端，也可以继续使用 `HttpAgentClient`。

## 自定义

- 品牌与布局：修改 `ui/index.html` 与 `ui/styles.css`。
- 流式工作项和工具卡片：修改 `ui/app.mjs`。
- HTTP/Tauri 协议：修改 `clients/javascript`，不要在 UI 中复制另一套 reducer。
- Memory、Planner、浏览器、代码编辑、审批：放在插件或应用层，UI 只显示其公开事件与详情。

## 边界

当前参考 UI 是单 Run 工作台，不是后端 Session 数据库。项目名称是本地展示标签，不代表工作目录授权；页面关闭订阅不等于取消外部动作。
