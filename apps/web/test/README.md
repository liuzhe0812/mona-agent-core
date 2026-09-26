# Web UI 验收

本目录只提供本地受控测试，不接真实供应商，不进入正式启动流程。所有脚本使用独立端口/profile或明确的固定 fixture 端口；结束时只清理自己创建的进程和数据。

## 产品 UI 模块与 Planner

`npm run test:web:extensions` 使用当前正式 Web、隔离 Rust 宿主、受控协议端点和真实文件工具，验证模块清单鉴权、默认执行、模式保存/刷新/宿主重启、版本冲突、只读规划、同批提交后拒绝、明确继续后的单次真实写入、Memory 模块迁移、待重启与实际禁用状态、历史只读展示，以及移动端布局与坏渲染器回退。 同页重连验证模板重新挂载；`ui-module-checks.mjs` 在浏览器内补充坏插槽隔离、设置顺序与移除、旧计划按钮、并发模式点击、草稿保护和连续输出刷新。后者的竞态使用独立 UiHost 与受控响应，不冒充真实宿主写入验证。

先构建 `cargo build -p server`，再运行 `npm run test:web:extensions`，默认复用 `target/debug` 中的正式二进制；遇到 Windows 文件占用时，在获得停服授权后停止本项目开发服务再构建，不要求额外构建目录。`MONA_TEST_SERVER` 可指定已有二进制。测试仍使用临时状态目录、随机端口和独立浏览器 profile，不修改日常开发数据；结果默认写入 `target/ui-extensions-validation/browser/`，不包含正式用户数据。测试不自动清理用户旧格式会话，不验证真实供应商的规划质量。UI Registry 的纯逻辑、加载竞态和释放规则包含在 `npm run test:web` 中。

## 输入框命令菜单

`commands.test.mjs` 纳入 `npm run test:web`，验证分组、可见性、容量、失败与执行前检查。`command-menu-checks.mjs` 由现有浏览器脚本调用：`test:web:extensions` 使用真实隔离宿主验证计划切换、权限选择器复用及禁用后的菜单；`test:web:new-task` 用受控管理接口验证模型选择器与配置保存。两者检查输入框同宽定位、菜单尺寸、键盘和草稿保留，打开菜单本身不创建模型调用。受控模型配置保存不代表真实供应商调用。

## 子 Agent

先构建 `cargo build -p server`，再运行 `npm run test:web:subagent`。脚本复用默认构建，在独立目录、端口和浏览器 profile 下运行正式 Rust 宿主、实际文件/Shell 工具与受控模型。检查并行委派、独立/继承上下文、模型绑定不改变默认值、用量只累计一次、消息和后续激活、独立停止与父级取消、实际沙箱和 Planner 限制、跨根拒绝、重启恢复、配置保存以及只读历史。

结果和截图位于 `target/subagent-validation/browser`；测试失败不以假后端降级，退出只清理本次创建的进程和目录。Windows 测试目录位于用户 `LOCALAPPDATA/mona-agent-core/subagent-tests`，不修改日常工作区 ACL。浏览器与平台原生工具的运行证据只适用于执行该测试的环境，不代表其他操作系统、原生桌面 WebView 或真实模型任务质量。纯 UI 契约与空工具上限检查纳入 `npm run test:web`。

## 本机沙箱

先 `cargo build -p server`，再 `npm run test:web:sandbox`。测试调用当前操作系统的真实沙箱，覆盖 Shell 与 write/edit 的文件边界、只读读取、显式不受限操作、会话模式保存/重启、并发版本拒绝、原生子进程取消、临时目录清理、关闭组件后的拒绝降级及部署锁定。模型仅为受控端点；缺少平台后端时测试失败，不以假后端或跳过充当原生验证。

结果与截图保存在 `target/sandbox-validation/browser`。Windows 的隔离工作区创建在用户 `LOCALAPPDATA/mona-agent-core/sandbox-test-workspaces` 下，其他平台使用报告目录下的随机子目录；每次运行独占临时环境和浏览器 profile，并在结束时删除自己创建的随机目录，不修改日常会话或既有项目权限。Windows ACL 后端要求目标目录可设置 DACL 与完整性标签；权限不足的拒绝路径另有原生回归，不通过自动提权让测试通过。`cargo check --target ...` 只代表对应平台编译检查，不等于真实操作系统执行验收。

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

覆盖正式侧栏结构、208–460px 拖动范围、宽度持久化、Tab/方向键、键盘焦点、`Ctrl+K`/Esc、`Ctrl+B` 侧栏切换、收起/展开、390px 抽屉和 Composer 共享焦点环；项目与任务分区使用相同的六点手柄，受控真实宿主测试验证拖拽交换分区与键盘复位，此 fixture 验证顺序在刷新后恢复。任务分组还检查标题、折叠控件与右侧手柄和新建任务按钮的悬停出现、折叠状态写入浏览器本地，以及点击任务手柄仍能打开刷新列表和已归档菜单。该 fixture 不提供本地会话存储，因此任务行的平时/悬停/右键菜单状态由第 7 节真实宿主流程验证。Windows 清理使用带重试的独立临时 profile，避免浏览器锁文件造成假失败。

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

### 正式会话内容交互

`npm run test:web:content` 使用正式页面和受控宿主，验证标准 Markdown、中文/标识符/单列表格、KaTeX 上下标、完整 Mermaid 图表及安全沙箱、延后高亮、稳定块/代码展开/选区、长文本 Worker、完整原文兜底、工具日志更新、整轮复制、格式间查找与选区引用、局部权限错误、观察性重连和窄屏。默认报告在 `.tmp-verify/content-ui-browser-report/`。它不证明真实模型输出质量或所有 Markdown/图表输入都可绘制。

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

## 精选记忆与历史原文

构建 `cargo build -p server --target-dir target/context-validation` 后执行 `node apps/web/test/memory-e2e.mjs`。从独立空目录启动正式宿主，验证页面写入 Markdown、作用域/版本拒绝、模型自动注入、工作区隔离、旧原文搜索和分页、工具授权、实际 Agent 写入、编辑删除刷新以及宿主进程重启。同时验证关闭长期记忆和历史检索后，基础会话仍正常续聊、数据不被删除、管理控件如实不可用。报告默认在 `target/memory-validation/browser/`，可用 `MONA_TEST_REPORT_DIR` 指定；只清理本测试创建的目录和进程，不读取用户配置。

`memory-ui.test.mjs` 专项验证连接切换时旧保存/查询/原文回调不会覆盖新操作或保留旧内容；`memory.test.mjs` 检查请求/流式响应上限与取消。它们是状态单测，不替代真实浏览器布局验收。

受控模型只验证工具/权限/保存/检索接线，不证明自动筛选事实、真实模型记忆效果或语义检索质量。没有后台自进化任务。

## 会话布局与披露状态

`npm run test:web:conversation-layout` 在正式页面构造确定的运行、结束、失败、长过程、流式段落和窄屏场景；验证正文层级、分组滚动意图、模式即时生效、复制反馈、文件回调更新和未改变内容的选区保留。截图与度量默认写入 `.tmp-verify/conversation-batch1/after`；`MONA_TEST_REPORT_DIR` 可指定隔离位置。`MONA_CAPTURE_BASELINE=1` 仅记录改造前的失败对照，报告明确 `captureOnly=true`，不能作为通过证据。复制反馈用例使用隔离页面的受控剪贴板，不修改用户系统剪贴板。

## 会话与文档生命周期

`npm run test:web:conversation-lifecycle` 使用正式页面、实际正文/会话/右栏控制器和可控延迟，验证来源关闭后放大视图释放、媒体局部重试、过期文件动作和解析等待取消、历史加载期间用户滚动、内部日志滚动、查找/搜索失效，以及二进制刷新成功后提交、失败保留旧文档。长历史场景包含 70 轮富内容，属于有界交互测试，不是无限历史容量或帧率认证。

默认报告和截图位于 `.tmp-verify/conversation-batch4/after`；`MONA_TEST_REPORT_DIR` 可覆盖。`MONA_CAPTURE_BASELINE=1` 仅记录失败对照，不作为通过证据。Worker 停顿与文件读取失败用可控夹具复现；实际 Mermaid 渲染在浏览器中执行。PDF 字节夹具仅验证 iframe 生命周期，真实 Office/Git/PTY 和会话持久读取由 `test:web:right-pane` 验证，不把伪造文档头当作 PDF 格式兼容验收。

## 新任务页

`npm run test:web:new-task` 是正式页面的空态专项脚本，覆盖居中标题/输入器、无建议卡及假能力、同一 textarea 在首条消息前后的身份、浅深色和短窄屏草稿。报告默认写入 `.tmp-verify/new-task/after`；命令或报告存在不代表已经执行通过。执行模式真实性由 `cargo test -p planner` 和现有 `test:web:extensions` 的真实隔离宿主、模式持久化、只读限制与明确继续执行用例验证。

## 三栏与输入器

`npm run test:web:shell-layout` 使用正式 HTML/CSS 与实际布局、统计、阅读控制器，构造有界草稿、历史和统计夹具；验证左右宽度联动、共享水平轴、宽度偏好恢复、草稿增高/收缩、输入选区、统计顶层面板、左右模态焦点与快捷键隔离。窗口覆盖 1600、1440、1100、1024、800、390、320px，以及 390×320/900×360 短屏。报告默认在 `.tmp-verify/layout-batch3/after`；基线捕获仍明确标为非通过证据。

`layout-geometry.test.mjs` 验证宽度组合、短屏阅读预算和模态所有权。真实宿主联动继续由 `test:web:right-pane` 验证，基本宿主无模型管理模块的导航由 `test:web:sidebar` 验证。视口模拟与输入选择区测试不代表 Android/iOS 真实软键盘、输入法或 Tauri 平台认证。

## 右侧工作面板

`npm run test:web:pane-layout` 使用正式页面结构、样式和实际面板类，将应用启动入口替换成有界文件夹具；覆盖双窗格、实际缩放切换、工具栏、键盘、二进制菜单可见焦点、目录搜索竞态、失败保留、源码行引用及 iframe 身份。深色用例通过正式主题管理器切换配色，不只修改数据属性。报告默认在`.tmp-verify/right-pane-batch2/after/`，`MONA_CAPTURE_BASELINE=1`仅记录失败对照。`pane-layout.test.mjs`覆盖布局操作、跨窗格去重、存储限额和保存失败。组件夹具不能代替下面的真实宿主验收。

使用默认构建目录的 `cargo build -p server` 后运行 `npm run test:web:right-pane`；已有最新二进制可直接复用，不另建构建链。使用自己的用户/会话/工作目录、随机环回端口、实际 Git 与 PTY，验证顶部任务菜单、工作区信息、Web 隐藏系统打开方式及桌面命令夹具中的两项菜单与参数、任务后退/前进、内嵌终端按钮及快捷键，连续历史与阅读页恢复、正文文件/图片的来源会话绑定、多标签关闭/重开/刷新恢复偏好、目录展开、版本化分页保留 DOM、HTML sandbox、DOCX/XLSX/PPTX 真实预览、实际合并/左右 Diff、旁支隔离及窄屏。桌面命令夹具不实际启动操作系统窗口；桌面壳源码另以 `cargo check -p desktop --locked` 编译。`workbench-fixtures.mjs` 生成最小原始 OOXML 文件，不需要安装 Office。Office iframe 是独立来源，测试通过其 DevTools target 验证实际渲染内容，不在产品放宽同源权限。

`MONA_TEST_SERVER` 可指定测试构建，`MONA_TEST_REPORT_DIR` 可指定报告位置；默认 `.tmp-verify/right-pane-browser-report`。多格式样例通过不代表 Office 全格式一致性或恶意文件审计，产品接口与权限边界见[Web 装配](../../../docs/architecture/WEB.zh-CN.md)。

### 会话过程与输入器统计

`conversation-policy.test.mjs` 校验连续工作摘要、失败/补充输入的折叠规则，以及未知用量不伪造为缓存或速度。`test:web:content` 使用实际 TurnView 检查固定过程展示、历史前置的组身份和详情展开保留。`test:web:right-pane` 通过真实宿主创建多于默认历史页数的会话轮次，核对全会话 Token 与步数、上下文来源和缺失字段，并操作统计说明、右侧文件页、全屏和并看。夹具报告的 Token 是固定受控数值，只用于核对统计链路；字节估算与真实 tokenizer 精度不属于该夹具的证明范围。

## 10. 浏览器与证据边界

浏览器脚本要求 Node 22+。Windows 默认 Edge，其他平台默认 `chromium`，可通过 `BROWSER_BIN` 指定。验证结论必须区分：

- 静态/单元通过。
- 受控浏览器 fixture 通过。
- 真实 Rust 宿主通过。
- 真实供应商联调。
- Tauri WebView 或生产部署验收。

前三级不能自动升级成后两级，也不能替代完整无障碍人工检查。
