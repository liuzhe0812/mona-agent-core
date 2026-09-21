# Mona Agent Web UI 接入

## Web

无需 `.env`，在仓库根目录运行：

```sh
npm run dev:web
```

首次启动后在模型设置页添加供应商。环境变量仅用于可选的部署覆盖、旧配置导入或固定模型模式。

该命令只面向本机开发。远程 Web 产品应使用正式静态构建、HTTPS、账户认证和独立部署的 HTTP Bridge。

## Tauri

Tauri 宿主复用同一组 `apps/web/index.html`、`apps/web/app.mjs`、`apps/web/styles.css`。Rust 端安装 `tauri-plugin-bridge`；前端宿主向页面提供 `invoke` 与 `Channel` 后，UI 会选择 `TauriAgentClient`，不启动本地 HTTP 服务。

如果 Tauri 仅作为远程客户端，也可以继续使用 `HttpAgentClient`。

## 自定义

- 品牌与布局：修改 `apps/web/index.html` 与 `apps/web/styles.css`。
- 流式工作项和工具卡片：修改 `apps/web/app.mjs`。
- HTTP/Tauri 协议：修改 `packages/client`，不要在 UI 中复制另一套 reducer。
- Memory、Planner、浏览器、代码编辑、审批：放在插件或应用层，UI 只显示其公开事件与详情。

## 边界

当前参考 UI 是单 Run 工作台，不是后端 Session 数据库。项目名称是本地展示标签，不代表工作目录授权；页面关闭订阅不等于取消外部动作。
