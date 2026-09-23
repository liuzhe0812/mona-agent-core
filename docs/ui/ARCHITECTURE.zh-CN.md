# Mona Agent Web UI 架构

## 1. 定位

`apps/web/` 是正式通用界面，不是另一套 Agent Runtime。它消费 `packages/client` 的任务协议客户端和 `RunView`，通过宿主管理接口读取已装配能力、模型和本地会话；界面不执行模型或工具。

```text
index.html + app.mjs + feature modules
                 |
        packages/client / RunView
          /                    \
HttpAgentClient          TauriAgentClient
      |                         |
 HTTP/SSE Bridge           Tauri Bridge
          \                    /
             AgentApplication
                    |
                Agent Core
```

关闭浏览器订阅不等于取消任务；停止必须调用独立 cancel。UI 不决定权限、预算、工具调用或模型重试。

## 2. 设计系统管线

```text
                design-system.css
       Mona Foundation -> Semantic -> Component/Layout
                         |
 theme.mjs palette ---- semantic roles
                         |
 styles.css + sessions.css + appearance.css
                         |
             formal index.html surfaces
```

Mona 的权威规则是 [`DESIGN.zh-CN.md`](./DESIGN.zh-CN.md)，组件数值见 [`COMPONENTS.zh-CN.md`](./COMPONENTS.zh-CN.md)，生产范围见 [`DESIGN-INVENTORY.zh-CN.md`](./DESIGN-INVENTORY.zh-CN.md)。

## 3. 正式文件

| 文件 | 职责 |
| --- | --- |
| `apps/web/index.html` | 正式页面结构、设置和 Dialog；不含内联样式 |
| `apps/web/app.mjs` | UI 状态、连接、设置装配、流式订阅、取消和用户输入 |
| `apps/web/run-view.mjs` | 执行过程、工具组、详情和 Markdown 安全渲染 |
| `apps/web/sessions-ui.mjs` / `sessions.mjs` | 本地会话导航、搜索、恢复和持久请求适配 |
| `apps/web/theme.mjs` / `appearance.mjs` | 浏览器主题偏好、皮肤校验和外观设置 |
| `apps/web/design-system.css` | 正式 Token、共享控件、Dialog 与设置 shell 合同 |
| `apps/web/styles.css` | 主工作区、聊天、执行过程、搜索、能力和模型设置 |
| `apps/web/sessions.css` | 任务历史导航样式 |
| `apps/web/appearance.css` | 外观设置与预览样式 |
| `packages/client/src/*` | HTTP/Tauri 客户端、SSE 解码和共享 RunView |

`preview.html` 和 `apps/web/src/` 是历史离线预览/自定义元素实验，正式入口不加载；重新启用前需要先迁移并登记。

## 4. 主题边界

`theme.mjs` 在页面启动时把皮肤配色写入同一组语义变量，并只调整消息正文字号和四个受控圆角角色。它不替换页面 DOM、不重建任务客户端，也不访问会话、模型密钥或工具。

皮肤导入是配色数据导入，不是 CSS/脚本插件。主题文件无法改变布局、事件、网络或权限。

## 5. 管理接口与安全

- 模型设置和能力设置通过独立、受鉴权的宿主管理接口；不注册成模型工具。
- 页面可提交用户新输入的 API Key；已保存密钥不回传、不写 URL 或 `localStorage`。
- 正式任务正文通过 DOM 节点渲染，不把模型输出解析为 HTML。
- Spill 只通过同 Run 的鉴权接口读取纯文本，不打开宿主本机路径。
- 会话续聊由宿主恢复可信历史，浏览器不提交自造 transcript。

## 6. Web 与 Tauri

Web 使用 `HttpAgentClient`，Tauri 注入 `invoke`/`Channel` 后使用 `TauriAgentClient`。页面和设计系统不因传输改变；当前 Tauri 宿主未接入的会话/管理功能必须如实显示不可用，不能伪装成功。

## 7. 开发与验证

根目录运行：

```sh
npm run dev:web
```

它启动真实 Rust Server 和正式 Web UI，并通过环回配置端点连接；没有 `dev:web:demo`。

设计和 UI 变更按风险运行：

```sh
npm run check:web:design
npm run test:web
npm run test:web:design
npm run test:web:themes
npm run test:web:sidebar
```

真实本地会话链路另运行 `node apps/web/test/sessions-e2e.mjs`（需先构建 Server）。各检查的证据边界见 [`DESIGN-GOVERNANCE.zh-CN.md`](./DESIGN-GOVERNANCE.zh-CN.md) 和 [`apps/web/test/README.md`](../../apps/web/test/README.md)。
