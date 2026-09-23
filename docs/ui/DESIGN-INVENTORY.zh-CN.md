# Mona Web UI 生产界面台账

更新时间：2026-09-23。`governed` 表示已接入 `design-system.css`、组件合同和当前静态/浏览器门禁；不表示真实供应商、Tauri 或完整无障碍认证完成。

## 正式 surface

| ID | 用户入口 | 主要文件 | 状态 | 主要验收 |
| --- | --- | --- | --- | --- |
| `shell` | `/` | `index.html`、`styles.css`、`app.mjs` | governed | design、sidebar |
| `task-sidebar` | 左侧任务导航 | `index.html`、`sessions.css`、`sessions-ui.mjs` | governed | sidebar、sessions |
| `task-search` | 品牌旁搜索 / Ctrl+K | `index.html`、`styles.css`、`sessions-ui.mjs` | governed | sidebar、sessions |
| `new-task-home` | 新建任务 | `index.html`、`styles.css` | governed | design |
| `chat-stream` | 提交任务后 | `run-view.mjs`、`styles.css` | governed | design、themes、renderer |
| `conversation-rail` | 多轮聊天左侧轮次导航 | `conversation-rail.mjs`、`index.html`、`styles.css` | governed | design、renderer |
| `execution-process` | 聊天中的执行摘要 | `run-view.mjs`、`styles.css` | governed | design、renderer |
| `composer` | 主界面底部 | `index.html`、`styles.css`、`app.mjs` | governed | design、sidebar、themes |
| `model-picker` | Composer 模型选择 | `index.html`、`styles.css`、`app.mjs` | governed | design、model tests |
| `connection-dialog` | 顶栏连接状态 | `index.html`、`styles.css`、`app.mjs` | governed | design、dev-web tests |
| `settings-shell` | 侧栏设置 | `index.html`、`design-system.css`、`app.mjs` | governed | design、themes |
| `settings-components` | 设置 → Agent 组件 | `index.html`、`styles.css`、`capabilities.mjs` | governed | design、capability tests |
| `settings-tools` | 设置 → Agent 工具 | `index.html`、`styles.css`、`capabilities.mjs` | governed | design、capability tests |
| `settings-models` | 设置 → 模型设置 | `index.html`、`styles.css`、`model-settings.mjs` | governed | design、model tests |
| `provider-dialog` | 添加/编辑供应商 | `index.html`、`styles.css`、`app.mjs` | governed | design |
| `model-dialog` | 添加模型 | `index.html`、`styles.css`、`app.mjs` | governed | design、model tests |
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
