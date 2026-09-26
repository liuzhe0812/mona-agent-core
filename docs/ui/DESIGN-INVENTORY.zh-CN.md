# Mona Web UI 生产界面台账

更新时间：2026-09-23。`governed` 表示已接入 `design-system.css`、组件合同和当前静态/浏览器门禁；不表示真实供应商、Tauri 或完整无障碍认证完成。

## 正式 surface

| ID | 用户入口 | 主要文件 | 状态 | 主要验收 |
| --- | --- | --- | --- | --- |
| `shell` | `/` | `index.html`、`styles.css`、`app.mjs` | governed | design、sidebar |
| `task-sidebar` | 左侧任务导航 | `index.html`、`sessions.css`、`sessions-ui.mjs` | governed | sidebar、sessions |
| `project-navigation` | 可选项目列表与添加/移除 | `workspace-ui.mjs`、`workspace.css`、`sessions-ui.mjs` | governed | workspaces、base-only |
| `workspace-settings` | 设置 → 工作区 | `workspace-ui.mjs`、`workspace.css` | governed | workspaces |
| `workspace-files` | 右侧文件页、目录展开、名称搜索 | `workspace-files.mjs`、`workspace-ui.mjs`、`workspace.mjs`、`workspace.css` | governed | right-pane-e2e |
| `right-pane` | 顶部面板切换、多标签、概览与拖宽 | `right-pane.mjs`、`workspace.css` | governed | right-pane-e2e |
| `file-preview` | 文件只读预览与鉴权下载 | `file-preview.mjs`、`document-preview.*` | governed | right-pane-e2e |
| `workspace-review` | 右栏 + → 审查 | `workbench-ui.mjs`、`workspace.css` | governed | right-pane-e2e |
| `workspace-terminal` | 右栏 + → 终端 | `workbench-ui.mjs`、自托管 xterm | governed | right-pane-e2e |
| `side-conversation` | 右栏 + → 侧边对话 | `side-conversation.mjs`、`workspace.css` | governed | right-pane-e2e |
| `subagent-panel` | 右栏 → 子 Agent，独立任务与执行片段 | `ui/modules/subagent.mjs`、`ui/subagent-view.mjs`、`ui/extensions.css` | governed | subagent unit、subagent-e2e |
| `settings-subagent` | 设置 → 子 Agent，并发/角色/模型/工具上限 | `ui/modules/subagent.mjs`、`ui/extensions.css` | governed | subagent unit、subagent-e2e |
| `task-search` | 品牌旁搜索 / Ctrl+K | `index.html`、`styles.css`、`sessions-ui.mjs` | governed | sidebar、sessions |
| `new-task-home` | 居中 LOGO、标题与共用输入器 | `index.html`、`styles.css`、`shell-layout.mjs` | governed | design、new-task |
| `chat-stream` | 提交任务后 | `run-view.mjs`、`content-renderer.mjs`、`styles.css` | governed | design、themes、renderer |
| `conversation-reading` | 查找、连续历史、回到底部、选区引用 | `conversation-controls.mjs`、`conversation-find.mjs`、`conversation.css` | governed | content-ui、right-pane-e2e |
| `conversation-statistics` | 输入器下方统计与详情 | `conversation-metrics.mjs`、`conversation.css` | governed | right-pane-e2e、policy tests |
| `message-content` | 代码/表格/公式/图表/图片操作 | `content-renderer.mjs`、`content-dom.mjs`、`diagram-view.mjs` | governed | content-ui、content tests |
| `message-file-reference` | 回答中的文件链接与卡片 | `presentation-path.mjs`、`file-reference-cards.mjs`、`workspace-ui.mjs` | governed | right-pane-e2e |
| `message-diff` | 工具 Diff 与右侧 Git 审查 | `diff-view.mjs`、`conversation.css` | governed | content tests、right-pane-e2e |
| `conversation-rail` | 多轮聊天左侧轮次导航 | `conversation-rail.mjs`、`index.html`、`styles.css` | governed | design、renderer |
| `execution-process` | 聊天中的执行摘要与富工具结果 | `run-view.mjs`、`content-renderer.mjs`、`styles.css` | governed | design、renderer |
| `composer` | 新任务居中／会话底部 | `index.html`、`styles.css`、`shell-layout.mjs`、`app.mjs` | governed | design、sidebar、themes、new-task |
| `composer-commands` | 输入框 `+`，扩展命令分组菜单 | `ui/command-menu.mjs`、`ui/commands.mjs`、`ui/modules/`、`styles.css` | governed | commands tests、extensions、new-task |
| `model-picker` | Composer 模型选择 | `index.html`、`styles.css`、`app.mjs` | governed | design、model tests |
| `connection-dialog` | Runtime 连接设置 | `index.html`、`styles.css`、`app.mjs` | governed | design、dev-web tests |
| `settings-shell` | 侧栏设置 | `index.html`、`design-system.css`、`app.mjs` | governed | design、themes |
| `settings-components` | 设置 → Agent 组件 | `index.html`、`styles.css`、`capabilities.mjs` | governed | design、capability tests |
| `settings-tools` | 设置 → Agent 工具 | `index.html`、`styles.css`、`capabilities.mjs` | governed | design、capability tests |
| `settings-memory` | 精选长期记忆、范围和变更管理 | `memory*.mjs/css`、`index.html` | governed | memory tests、memory-e2e |
| `history-original` | 关键词检索、有界原文与相邻消息 | `memory-ui.mjs`、`memory.css` | governed | memory-e2e |
| `settings-models` | 设置 → 模型设置 | `index.html`、`styles.css`、`model-settings.mjs` | governed | design、model tests |
| `provider-dialog` | 供应商、接口协议与原生推理参数 | `index.html`、`styles.css`、`app.mjs` | governed | design、protocols-e2e |
| `model-dialog` | 模型窗口、三态能力与输出上限 | `index.html`、`styles.css`、`app.mjs` | governed | design、model tests、protocols-e2e |
| `settings-appearance` | 设置 → 外观 | `appearance.mjs`、`appearance.css`、`theme.mjs` | governed | design、themes |
| `session-history` | 左侧任务列表与历史恢复 | `sessions*.mjs/css` | governed | sessions、sidebar |
| `spill-result` | 工具详情中的长结果 | `spill.mjs`、`run-view.mjs`、`styles.css` | governed | renderer、spill tests |
| `responsive-main` | 390/320px | 正式 CSS 全部 | governed | design、themes、sidebar、sessions |

## 共享设计资产

| 文件 | 角色 |
| --- | --- |
| `apps/web/design-system.css` | Mona 自包含的 Foundation/Semantic/Component Token 与基础组件 |
| `apps/web/theme.mjs` | 受控皮肤配色、外观偏好、导入/导出和保存 |
| `docs/ui/DESIGN.zh-CN.md` | 设计规则正本 |
| `docs/ui/COMPONENTS.zh-CN.md` | 组件尺寸与状态合同 |
| `docs/ui/DESIGN-GOVERNANCE.zh-CN.md` | 变更、例外、门禁和证据规则 |
| `scripts/check-web-design.mjs` | 可机器检查的设计门禁 |

隔离文档预览 iframe 的 `document-preview.css` 与第三方引擎样式保留纸张/文档自身色彩；它们不用于 Mona 导航、按钮或主界面，不套用主应用的任意颜色静态门禁。正文的公式字体和图表输出也属于受限内容而非可执行皮肤。脚本/CSP 与权限见[应用层接口](../architecture/WEB.zh-CN.md)，固定版本和许可证见 [vendor](../../apps/web/vendor/README.md)。

## 非正式/历史 surface

| 路径 | 定位 | 约束 |
| --- | --- | --- |
| `apps/web/preview.html` | 早期离线视觉预览 | 不连接正式 Runtime，不作为截图或组件权威 |
| `apps/web/src/controller.ts`、`element.ts`、`styles.css` | 早期自定义元素实验 | 当前正式 `index.html` 不加载；不纳入正式设计门禁。重新启用前必须先登记 surface 并迁移到正式 Token |
| `apps/web/test/renderer.html` | 受控渲染验收页 | 使用正式 RunView，但内容是测试 fixture，不是产品入口 |

## 当前边界

- 正式 Web UI 仍是原生模块，没有发布为独立 UI 组件包。
- 浏览器设计验收使用 Chromium/Edge；Tauri 复用页面但尚未完成同等 WebView 截图矩阵。
- 设计 fixture 提供受控模型/能力数据，不发送真实供应商请求。
- 全局鼠标提示由 `tooltip.mjs` 与 `design-system.css` 统一显示，沿用各控件文案和可访问名称，支持动态列表与弹窗。
- 主题首帧使用 `blocking="render"` 的浏览器能力和基础回退，不宣称所有引擎绝对无闪色。

## 台账更新规则

新增、删除、改名或改变正式页面职责时，在同一改动中更新本表、架构文档和对应浏览器检查。仅复用共享样式不代表新 surface 已完成迁移；必须验证其真实入口、状态和窄屏。
