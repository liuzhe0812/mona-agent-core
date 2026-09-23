# Web UI 验收

本目录只提供本地受控测试，不接真实供应商，不进入正式启动流程。所有脚本使用独立端口/profile或明确的固定 fixture 端口；结束时只清理自己创建的进程和数据。

## 1. 静态设计门禁和单元测试

```sh
npm run check:web:design
npm run test:web
```

`check:web:design` 检查正式 feature CSS 没有绕过设计系统写入裸颜色、任意字号/字重/圆角、硬编码动效时长和普通投影；同时检查关键组件 Token、正式样式入口、历史过大设置几何、HTML 内联样式、Dialog 重复关闭入口，以及运行时 JS 只使用已登记的侧栏宽度、主题变量和菜单坐标钩子。

`test:web` 覆盖开发启动器、能力/模型客户端、RunView、Spill、会话、主题以及设计门禁自身的正反例。它是逻辑和静态检查，不代替浏览器视觉验收。

## 2. 设计系统真实浏览器验收

```sh
npm run test:web:design
```

使用正式 `index.html`、真实 Web UI 模块、受控 HTTP/SSE Runtime 和受控管理数据，在 Edge/Chromium 中检查计算后的生产样式：

- Foundation/Component Token 实际接线。
- 主工作区顶栏、导航、建议、Composer 和按钮尺寸。
- Agent 组件与 Agent 工具的独立页面、紧凑行、开关、面板密度，以及冗余状态/策略标签不再出现。
- 模型供应商侧栏、头部、搜索、模型行、开关和按钮。
- 供应商 Dialog 的 16px 内边距、36px 控件、12px 圆角和单一关闭路径。
- 外观分段控件、皮肤卡、设置行、Select 和预览。
- 深色主文字对比度。
- 聊天正文、执行摘要和 28px 活动行。
- 390px/320px 单列设置与无页面级横向溢出。

截图和结果写入 `.tmp-verify/design-system-browser-report/`。fixture 不发送真实模型请求，也不证明 Tauri WebView 已验收。

## 3. 主题与皮肤验收

```sh
npm run test:web:themes
```

覆盖六套皮肤明暗组合、键盘切换、跟随系统、字号/圆角、刷新与浏览器重启、导入导出、存储失败、跨标签同步、损坏数据回退、320/390px 和减少动效。执行中的换肤还核对 HTTP 请求数、订阅数、聊天 DOM 身份和草稿，确认不会重启、取消或重新订阅任务。

报告和截图位于 `.tmp-verify/theme-browser-report/`。

## 4. 侧栏与搜索验收

```sh
npm run test:web:sidebar
```

覆盖正式侧栏结构、208–460px 拖动范围、宽度持久化、Tab/方向键、键盘焦点、`Ctrl+K`/Esc、真实快捷操作、收起/展开、390px 抽屉和 Composer 共享焦点环；任务分组检查包含分组标题、折叠控件与分组操作按钮（`⋯` 列表操作菜单、新建任务）的悬停出现、折叠状态写入浏览器本地并在刷新后保留，以及刷新入口位于 `⋯` 菜单内。该 fixture 不提供本地会话存储，因此任务行的平时/悬停/右键菜单状态由第 7 节真实宿主流程验证。Windows 清理使用带重试的独立临时 profile，避免浏览器锁文件造成假失败。

报告和截图位于 `.tmp-verify/sidebar-browser-report/`。

## 5. HTTP/SSE 交互 fixture

```sh
node apps/web/test/fixture-server.mjs
```

默认 Web UI 在 `http://127.0.0.1:4175`，API 在 `127.0.0.1:8789`。固定 token 仅用于本地测试，不写入请求日志。默认管理接口返回未安装，用于验证基础聊天降级；设计测试通过显式 `management: true` 注入受控供应商、模型和能力记录。

- 普通输入：包含中间消息、工具活动和最终回复。
- 包含“长任务”：可在执行中展开、追加、停止。
- 包含“失败”：工具和任务呈现失败。
- 长参数和 HTML 字符串只作纯文本显示。

## 6. 渲染器检查

启动正式 Web 静态服务后打开 `/apps/web/test/renderer.html`。页面复用正式 `TurnView`，验证折叠、增量状态保留、截断、HTML 安全、Markdown 子集、工具组、工具类别标签与线条图标、展开后不再重复命令摘要、卡片不再出现工具名与状态页脚、无详情行、空模型回合、ANSI 去除、Spill 分页和长命令有界滚动。它渲染 fixture，不是产品入口。

## 7. 本地会话端到端

```sh
cargo build --offline -p server
node apps/web/test/sessions-e2e.mjs
```

使用真实 Rust 宿主、正式 Web UI、受控本地模型和真实 `read` 工具。覆盖新会话保存、URL、进程重启恢复、真实历史续聊、重命名、搜索、新会话隔离、删除、异常中断、持久请求去重。任务列表同时验证平时状态（标题 + 相对时间）、时间与行内操作共用同一槽位且互斥、行内操作为置顶与 `⋯`、右键与 `⋯` 打开同一个五项菜单（重命名 / 置顶聊天 / 归档 / 标记为未读 / 删除，每项带线条图标，删除在最后且为危险色）、Esc 关闭菜单、置顶在宿主落盘并反映为按下状态、归档把任务移出当前列表且可由分组菜单的已归档视图恢复、未读圆点在未悬停时可见、打开任务后宿主未读标记被清除，以及执行中任务在标题前显示转圈指示：转圈确实在转动（比较两次计算后的 transform）、相对时间保留、任务结束后指示隐藏且标题横向位置不变（行节点复用，行刷新不会让动画重头开始）；并把任务行、任务行操作、任务分组、执行中状态和菜单放大截图保存为 `task-row-resting-zoom.png`、`task-row-hover-zoom.png`、`task-row-actions-zoom.png`、`task-header-zoom.png`、`task-running-zoom.png`、`task-menu.png`。工作空间、状态目录、端口和浏览器 profile 均隔离。

报告和截图默认位于 `target/browser-reports/sessions/`，可由 `MONA_TEST_REPORT_DIR` 指定。该流程使用真实宿主和工具，但仍不调用真实外部供应商。

## 8. 上下文与长会话真实宿主验收

```sh
cargo build --offline -p server --target-dir target/context-validation
node apps/web/test/context-e2e.mjs
```

使用单独的构建目录，避免替换正在运行的开发 server.exe。测试从空模型设置和空会话目录启动，通过正式页面设置模型窗口，再核对压缩、Skills/项目规则来源与真实用户锚点、跨 Run/宿主重启的摘要复用、完整历史保留、规则文件变更，以及真实 shell/Spill/read 归档在同会话后续 Run 和重启后可读、其他会话不可通过 URI 获得授权。它不读取仓库 `.env`，不使用真实供应商，进程和磁盘数据均为本测试创建。

可通过 `MONA_TEST_SERVER` 指定其他已构建的隔离二进制。报告和截图默认位于 `target/browser-reports/context/`，可由 `MONA_TEST_REPORT_DIR` 指定。完整设计与边界见[上下文管理](../../../docs/CONTEXT-MANAGEMENT.zh-CN.md)。

该场景特意让服务启动目录不同于 `AGENT_WORKSPACE_DIR`，并在启动目录放置不可加载的 Skills 夹具，验证发现只使用配置工作空间；未配置 `AGENT_SKILL_DIRS` 的默认装配同样经过真实宿主验证。

同一脚本还从模型设置页切换两个模型，检查普通历史可继续、私有历史不兼容时显示新建会话提示且不新增轮次或发起模型/摘要调用、用户自行新建后可用，以及恢复原模型和实际重启后私有协议原值继续回传但不暴露到 UI。摘要夹具返回严格 TaskSummary；该浏览器验证证明接线和存储行为，不证明真实模型的语义摘要质量。

## 9. Responses / Messages 原生协议

```sh
cargo build --offline -p server --target-dir target/context-validation
node apps/web/test/protocols-e2e.mjs
```

使用正式页面配置两种协议、自定义 API Base、API Key、推理参数、模型窗口和三态能力。端点校验对应鉴权头；执行真实 Rust read 工具，并验证多轮工具结果、摘要调用、跨实际宿主进程重启的摘要与私有回传保存、普通历史跨协议续聊、不兼容历史在保存前拒绝，以及输出中取消。390px 下检查展开的能力编辑框边界；截图与结果默认写入 `target/browser-reports/protocols/`，可通过 `MONA_TEST_REPORT_DIR` 指定。

端点、签名和摘要均为受控测试数据；该流程证明协议接线、保存和隔离，不证明真实供应商签名验证、图片语义或摘要质量。当前限制见 [Providers](../../../packages/providers/README.md)。

## 10. 浏览器与证据边界

浏览器脚本要求 Node 22+。Windows 默认 Edge，其他平台默认 `chromium`，可通过 `BROWSER_BIN` 指定。验证结论必须区分：

- 静态/单元通过。
- 受控浏览器 fixture 通过。
- 真实 Rust 宿主通过。
- 真实供应商联调。
- Tauri WebView 或生产部署验收。

前三级不能自动升级成后两级，也不能替代完整无障碍人工检查。
