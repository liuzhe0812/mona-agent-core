# Mona Agent Web UI 架构

## 1. 定位

`apps/web/` 是正式通用界面，不是另一套 Agent Runtime。它消费 `packages/client` 提供的 `AgentClient` 与 `RunView`，并通过宿主管理接口完成可选能力配置：

```text
apps/web/index.html + apps/web/app.mjs + apps/web/styles.css
                  |
          packages/client
             /           \
    HttpAgentClient   TauriAgentClient
          |                 |
     HTTP/SSE Bridge    Tauri Bridge
             \           /
             AgentApplication
                    |
                Agent Core
```

## 2. 当前文件

| 文件 | 职责 |
|---|---|
| `apps/web/index.html` | 页面结构与连接对话框 |
| `apps/web/app.mjs` | UI 状态、流式订阅、工具卡片、取消与补充输入 |
| `apps/web/styles.css` | Codex 风格的响应式明暗主题 |
| `apps/web/preview.html` | 早期离线视觉预览；不作为真实 Runtime 的标准入口 |
| `packages/client/src/*` | 两种传输客户端与共享 RunView |

## 3. 复用原则

Web 模式使用 `HttpAgentClient`，Tauri 模式使用 `TauriAgentClient`。页面和渲染逻辑不因传输方式改变；只有客户端注入不同。

UI 不接收模型密钥，不执行工具，不决定权限。工具参数增量只是显示内容；真正执行必须等待 Core 完整校验、授权和调度。

## 4. 开发入口

根目录 `npm run dev:web` 启动真实 Rust Runtime、HTTP Bridge 和 Web UI，并通过环回配置端点自动连接。没有假模型的 `dev:web:demo` 命令。
