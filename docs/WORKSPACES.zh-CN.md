# 工作区、项目与右侧文件栏

状态：本版已实现，Windows 真实 Rust 宿主与浏览器受控验收通过。本文是 mona-agent-core 的工作区功能正本；实际完成范围、测试证据和交付限制见第 10 节。

## 1. 目标与非目标

Mona Agent Core 的标准 Agent 开箱具有可配置的默认工作区。普通持久会话在默认根目录下分配独立子目录；项目会话使用项目已绑定的真实目录。基础集成不安装项目管理包，也能执行文件任务、多轮对话、长任务及恢复会话。

这是一套执行环境和文件访问能力，不是新的执行循环或完整 IDE。首版不做 Git Worktree、复制仓库、自动迁移目录、项目间文件同步、文件编辑器、操作系统沙箱或跨设备协作。项目可以是普通资料目录，不要求 Git。

## 2. 已确定的产品行为

| 操作 | 工作目录 |
|---|---|
| 普通入口新建会话 | 默认根目录 / 稳定会话 ID |
| 在项目下新建会话 | 项目登记的规范化实际目录 |
| 可信集成方明确提供目录 | 直接使用所给目录，不额外追加子目录 |
| 继续已有会话 | 会话创建时保存的实际目录，不跟随界面或默认配置漂移 |

- 一会话分配一次目录；每一轮新 Run 复用它。重命名聊天不重命名目录。
- 修改默认根目录只影响之后新建的普通会话，不搬动或改绑旧会话。
- 指定项目或目录失效时明确报错，不能回退默认目录。历史仍应可查看。
- 非项目会话不是没有工作区，而是没有项目归属。基础版不能靠创建一个隐藏的默认项目运行。
- 项目中的多个会话默认共享项目文件、聊天历史各自独立。共享文件并非并发编辑隔离。
- 删除/归档聊天不删除工作目录或产物；移除项目仅移除登记，不删除目录或历史。移除后的历史会话保留原目录及项目名称快照，可以独立恢复。
- 新建会话及其绑定先可靠保存，成功后才允许执行；失败不发布可用会话。目录分配只创建当前请求拥有的新目录，失败时不清理不明归属文件。

## 3. 架构边界

遵循仓库 AGENTS.md：核心层 + 扩展层 + 产品层，不维护旧文件格式兼容，不把可复用能力锁进 HTTP。

| 位置 | 职责 |
|---|---|
| api / runtime | 保持唯一执行循环和现有运行契约；不引入 project_id，不维护项目列表或文件树 |
| sessions | 一个宿主权限域下的会话目录；保存实际 workspace 路径及通用有界 metadata，项目扩展不是依赖 |
| workspace | 可复用的路径规范化、受根限制文件浏览/读取与版本检查；不依赖 projects、Web 或 Runtime |
| projects（可选） | 有稳定 ID 的项目登记，名称、规范化目录和 revision；不接管会话历史或执行 |
| tools / instructions / skills | 沿用已有实现，宿主传入一致且固定的 cwd；不查询项目注册表 |
| apps/server | 默认目录配置、目录分配、可信会话绑定、按目录装配执行环境、鉴权及 HTTP 适配 |
| apps/web | 左侧项目/普通任务、工作区路径设置、右侧文件树和只读预览 |

基础路径：默认/显式目录 → 会话绑定 → 同一套工具与执行环境。
项目路径：project_id → projects 解析并验证目录 → 相同的会话绑定和执行环境。

项目管理必须能从 Cargo 装配中移除。没有 projects 时，普通会话、默认根设置、文件浏览仍可使用，项目入口不伪装可用。

### 3.1 基础集成与项目扩展的可移除性

基础执行依然只接受可信宿主确定的 cwd，不为默认目录创建隐藏项目。`workspace` 提供目录和只读文件访问，`sessions::Store::create_in` 保存显式工作目录；两者都不依赖 `projects`。宿主负责默认根策略和每会话子目录分配。Web 的 `projects` Cargo feature 只添加项目登记和目录选择，不改变工具、会话、压缩或归档实现。基础发行验收应使用 `--no-default-features --features model-management,skills,compaction,spill`：仅去掉项目能力，而不是同时关掉所有可靠性组件。

项目选择只解析一次可信目录；执行目录固定到会话，不通过活动项目指针二次路由。既有 `/v1/runs` 是无持久会话的低层入口，不由 UI 将其伪装成保存会话；Web 的普通新任务统一走持久会话入口。多项目登记不等于同时编辑安全隔离，也不引入多租户授权。

## 4. 会话与持久化

会话的实际工作目录是执行事实。项目 ID/名称只是宿主写入的组织元数据，不是恢复目录的前置条件。浏览器不能在续聊请求中传入新的 cwd、history 或权限。

会话存储与用户工作文件分离。会话 Store 的单写者锁针对宿主状态目录，不能再以进程 cwd 哈希决定整个 Web 的可见历史，否则更换默认根会使旧历史消失。会话头保存不可变 workspace；通用 metadata 保存分组来源，不建立第二份项目成员账本。

当前为 Demo 阶段，文件格式变化更新版本并拒绝旧格式，不隐式迁移、清空或覆盖旧状态。测试使用隔离状态目录。文档必须说明版本变化，不动正在运行的用户服务及其数据。

工作文件示例（不把该路径硬编码进可复用包）：

```text
<用户配置的默认根>/
  s-<会话ID-A>/
    调研报告.md
  s-<会话ID-B>/
    data.csv
<用户选择的项目目录>/
  原有项目文件
<宿主私有状态>/
  sessions/             # 历史、检查点、绑定
  workspace-settings.json
  projects.json        # 仅安装项目扩展时
  spill/               # 有保留期限的大结果归档
```

普通会话默认根由宿主使用用户目录确定（用户主目录下 `~/.mona-agent/workspaces`），首次允许创建。已配置目录不存在时不自动换地址。UI 可修改，部署环境覆盖时只读。会话、凭据和 Spill 的状态目录不显示在普通文件树中；不允许把状态目录登记成可浏览根。

## 5. 执行环境与生命周期

不调用 process.chdir，不在运行中修改共享 ToolConfig.cwd。每个装配环境绑定一个规范化目录；需要不同目录时复用同一构建函数装配环境，不复制循环或工具实现。

Web 可以同时登记多个项目、打开多个历史会话；查看文件不启动 Agent。一次 Run 启动前由宿主从已保存会话解析目录并绑定相应环境。界面切换只切换展示，不影响已在执行的 Run。

宿主对执行环境保留量设界限，只有没有持有者/活动 Run 的环境可释放；关闭时先关闭应用接入并等待任务，再释放 Host。模型选择、预算、取消、可靠 SessionSink、Compaction 和 Spill 继续使用已有公共实现。Compaction 跨环境使用同一受管理状态源，避免会话 Store 的压缩状态指向另一个环境。

## 6. HTTP 契约

所有接口沿用同一 Bearer 权限域、明确 CORS、no-store 和大小限制；不是多租户认证。

- GET /api/workspace-settings：返回 revision、default_root、locked、projects_enabled。
- PUT /api/workspace-settings：提交 revision、default_root。校验并原子保存后生效，只影响新普通会话。
- GET /api/projects：项目列表与 revision；未安装时返回明确不可用状态。
- POST /api/projects：提交 request_id、revision、name、path；目录由用户显式选择并由宿主授权，路径规范化去重。
- POST /api/projects/{id}/remove：revision 守卫，仅删除登记。
- POST /api/sessions：request_id 和可选 project_id。无项目时分配独立目录；不能从页面传任意 cwd。
- GET /api/sessions：保持分页/搜索/归档行为，返回的会话头带 workspace 和通用 metadata，支持按项目或普通任务筛选。
- GET /api/sessions/{id}/workspace：返回当前会话 cwd 的可浏览信息和可用状态；不查询模型。
- GET /api/sessions/{id}/files：相对 path、offset、limit、可选 revision；列出直接子项，不递归全盘扫描。
- GET /api/sessions/{id}/file：相对 path、offset、limit、可选 revision；返回有界文本或支持的栅格图片，二进制有明确不可预览反馈。

目录信息结构：path、entries[{name,path,kind,bytes}]、revision、next_offset。文件信息结构：path、name、bytes、media_type、revision、offset、next_offset、eof，加 text 或 base64。所有 offset/limit 非负并有上限；分页变化返回 conflict，不把两代内容拼接。

## 7. 文件访问安全与容量

- 根目录只由宿主根据会话身份加载。页面只提供相对路径。
- 拒绝绝对路径、父目录跳转、NUL、Windows 驱动器/UNC/ADS 语法。校验组件而非字符串前缀。
- 不跟随符号链接和 Windows reparse point；目录项可显示为不可打开链接，但不会授权其目标。校验前后文件身份/版本，普通文件读取有上限。
- 不把路径限制包装成操作系统沙箱。默认 Shell 仍遵守现有宿主权限；抵御能够同时恶意替换祖先目录的本地进程需要独立的 OS 沙箱/句柄能力。
- 目录单次扫描有上限并显式报告超限；每页最多 200 条。文本每页最多 64 KiB，总单文件预览有界；图片最多 4 MiB，仅 PNG/JPEG/WebP/GIF，不将 HTML/SVG 当活动内容。
- 文本按 UTF-8 安全分页，版本不一致提示重新加载。请求中止后不更新失效会话的面板。
- 文件发生变化时支持手动刷新；工具运行终态后刷新。第一版不宣称操作系统级实时监听。
- 普通文件、任务 Artifact 与内部状态含义分开。最近修改的文件不能冒充当前会话产物。Spill 的保留期限不因右侧栏而改变。

## 8. Web UI

- 左侧项目分组可添加目录，选择项目后显示其会话及新建入口；普通新建任务不继承最近项目。
- 工作区设置可修改默认根，保存失败保留原值。项目能力缺失时不显示项目分组。
- 右侧栏可收起；跟随选中会话显示目录名称/路径、文件树和只读预览。新任务还没有持久会话时显示真实空态，不创建目录刷存在感。
- 文件树目录按需展开/返回，列表分页；文件预览文本、代码和栅格图片，过大/二进制/拒绝/失效有明确反馈。
- 切换会话或宿主时取消旧请求并递增请求代次，迟到响应不能污染当前视图。文字通过安全 DOM 输出；图片通过有界授权数据展示，不将凭据置入 URL。
- 保持 Mona 紧凑设计 Token。桌面三栏，窄屏右侧作为独立可关闭面板，不把聊天压至不可读宽度。键盘可操作，不增加无意义状态标签。

## 9. 验收

1. 无 projects 包基础测试：显式目录、两普通会话不同目录、跨轮/重启绑定不变、规则与工具 cwd 一致。
2. 默认根变更只影响新会话，重命名/归档/删除不删产物；创建请求重复不改变绑定。
3. 两项目/多会话分组与共享目录正确，移除登记不丢历史，失效目录拒绝执行且不回退。
4. 路径跳转、绝对路径、符号链接/reparse、状态目录、伪造会话、无鉴权、并发 revision、容量边界与分页文件变化测试。
5. 真实 Rust 宿主受控模型调用真实文件工具，文件实际出现在正确会话目录；重启恢复后继续。
6. 真实浏览器检查项目入口、默认根设置、会话切换、文件树/预览/刷新、收起、浅深色和窄屏。
7. 回归已有会话、模型设置、主题、内容渲染。未修改 Core 执行循环；不自动提交 Git，不重启真实服务。

## 10. 实施状态

### 已落地

- `packages/workspace`：无项目依赖的路径规范化、根目录限定访问、版本化目录/文件分页。
- `packages/projects`：可选登记、规范路径去重、版本守卫、幂等创建和仅移除登记。移除后的项目 ID 不用于另一个新登记，旧会话不因请求键复用而误归属。
- `packages/sessions`：格式 3，宿主权限域独立存储，真实 cwd 与通用元数据随会话保存。默认路径变化不隐藏旧历史。
- `apps/server`：默认根配置、独立会话目录分配、按目录装配现有能力和鉴权接口。目录分配失败清理只针对确认未保存、由本请求创建的空目录；提交状态不明时不删除目录。
- `apps/web`：项目与普通任务、工作区设置、右侧逐层文件浏览、文本/代码只读预览、栅格图片与有版本的分页。设置读取和保存期间禁用对应字段，避免迟到的配置刷新覆盖输入；会话切换后的文件响应不写入新视图。

### 本次验证证据

| 验证范围 | 结果 |
|---|---|
| `cargo test -p workspace -p projects -p sessions -p server --no-default-features --features model-management,skills,compaction,spill,projects --target-dir target/workspace-validation` | 48 项通过；明确选择本次工作区装配，含会话恢复、摘要、Spill、目录隔离、Windows 私有目录大小写与项目身份不复用 |
| `cargo build -p server --no-default-features --features model-management,skills,compaction,spill --target-dir target/workspace-validation` | 通过；编译中移除 projects，保留可靠性组件，再运行基础版端到端测试 |
| 基础发行版正常生产依赖图 | 无 projects；包含 workspace、sessions、runtime、tools、compaction、spill |
| `npm run test:web` | 57 项通过；含鉴权、迟到响应丢弃、路径限制、字节/版本/分页响应校验 |
| `npm run test:web:design` | 32 项浏览器断言通过；覆盖现有设计、内容渲染与窄屏 |
| 现有持久会话浏览器回归 `apps/web/test/sessions-e2e.mjs` | 37 项通过；真实 read、重启续聊、取消/中断、请求去重、归档/重命名/删除和搜索；报告 `.tmp-verify/workspace-browser-report/session-regression/result.json` |
| 工作区全功能真实宿主 + Edge/Chromium | 32 项断言通过；报告 `.tmp-verify/workspace-browser-report/full/report.json` |
| 不编译 projects 的真实宿主 + Edge/Chromium | 25 项断言通过；项目路由不存在，不只是隐藏菜单；报告 `.tmp-verify/workspace-browser-report/base/report.json` |

工作区端到端测试使用同一 `apps/web/test/workspaces-e2e.mjs`，通过 `MONA_TEST_SERVER` 选择本次构建二进制、`MONA_TEST_REPORT_DIR` 区分报告目录。基础版设置 `MONA_TEST_NO_PROJECTS=1`，必须配合真正去除 projects feature 的二进制；只设置运行时关闭标记不能通过缺失路由断言。测试在独立工作/状态/用户目录和随机环回端口启动自己拥有的宿主及浏览器，使用受控本地模型调用真实 `write/read/shell`，并重启测试宿主验证续聊。Compaction 与 Spill 保持装配，不以关掉可靠性组件冒充基础发行验证。

### 并行改动与全默认装配回归

收尾时工作区内同时出现 Memory / history-search 的并行实现，Server 默认 features 随之增加。最新未指定 feature 的 Server 全默认回归中，`capabilities::tests::management_view_separates_components_and_tools_and_hides_fixed_entries` 仍期望仅 `grep/find/ls`，实际清单新增 `memory_update`，该断言未通过；其他工作区相关断言通过。上述 48 项成功属于显式工作区 feature 组合，不是宣称当前包含并行 Memory 的所有默认配置已全量验收。此项应随 Memory 的能力合同与测试一起收口；本工作区任务没有删除或禁用其他任务的实现来消除失败。

### 交付边界与启用### 交付边界与启用

这是源码、编译、受控集成和真实浏览器验收，不是实际供应商、Linux/macOS/Tauri 原生界面或生产部署认证。当前页面仍维持一个前台执行任务，运行中不切换会话；没有宣称多任务并行 UI。文件栏是只读、逐层目录浏览，支持刷新与任务结算后更新，不包含编辑、上传、下载、OS 文件监听或 Worktree。

会话格式变为 3，不自动迁移旧版本。旧哈希分组状态会明确拒绝启动，不清空旧记录。首次验证新版本时，开发者应自行设置独立的 `AGENT_SESSIONS_DIR`（只隔离会话）或 `MONA_DEV_STATE_DIR`（隔离整套开发状态），再重新构建和启动宿主。修改默认根目录的页面操作本身不需要重启，只影响之后的新普通会话。

本次没有提交/推送 Git，没有重启开发者实际服务，也没有删除旧会话、项目或工作文件。
