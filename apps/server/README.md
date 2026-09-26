# server

Web 与桌面共用的产品服务组合根：装配现有 Runtime、模型与能力组件，管理部署配置、凭据、持久会话，提供鉴权后的 HTTP 管理接口与通用任务 Bridge。`lib.rs` 供桌面同进程复用，`main.rs` 是独立 Web 服务入口；不实现第二套 Agent 执行循环。

标准本地入口是在仓库根目录运行 `npm run dev:web`。第一次通过模型设置配置供应商；会话自动保存到用户状态目录，浏览器左侧可打开、搜索、重命名、删除和继续对话。单独运行 `cargo run -p server` 时，由部署者提供有效 `AGENT_SERVER_TOKEN` 及模型/存储配置；`--demo` 仅用于明确标识的离线演示。

部署能力见 [能力装配](../../docs/CAPABILITY-ASSEMBLY.zh-CN.md)，模型配置见 [模型管理](../../docs/MODEL-MANAGEMENT.zh-CN.md)，文件位置、接口、安全与恢复边界见 [本地会话](../../docs/LOCAL-SESSIONS.zh-CN.md)。`AGENT_SESSIONS_DIR` 可覆盖会话根目录，默认使用 `sessions`；只支持当前会话格式，不包含旧格式迁移或兼容分支。默认工作根来自已保存设置、`AGENT_WORKSPACE_DIR` 部署覆盖或首次 `~/.mona-agent/workspaces`；普通会话各分配子目录，项目会话固定项目目录。会话实现来自 `sessions` 扩展，规则实现来自 `instructions` 扩展；其他嵌入者可直接使用，不必安装本服务。

`/api/sessions` 是持久会话入口；原有 `/v1/runs` 仍用于临时 Run/实时事件，不向不可信客户端暴露任意历史注入能力。会话存储是未加密本地文件，部署者负责用户目录权限和备份。重启恢复记录并标识中断，不自动重做工具副作用。

`/api/workspace-settings` 报告 `project_create_mode`：装配 Projects 的 HTTP 宿主为 `name`，未装配为 `none`。鉴权后的 `POST /api/projects/create` 只接收项目名和版本，在默认工作区下创建同名目录并登记；名称必须是单层目录名，路径仍经过宿主的私有目录校验。`/api/projects` 保留已有目录的显式登记接口。

会话格式 5 区分完整档案、经校验的模型工作集和有界宿主扩展状态；摘要及覆盖范围通过原有检查点同步保存，跨 Run/重启复用。项目规则 `instructions` 扩展默认装配，可通过 Agent 组件页或部署配置关闭；可选工具在独立的 Agent 工具页管理。同会话旧 Spill 的访问由宿主验证结构化归属后路由，不放宽底层 Run 隔离。详见[上下文管理](../../docs/CONTEXT-MANAGEMENT.zh-CN.md)。

Shell 的流式归档适配器与结果变换器共享同一个 `SpillPlugin::archive()`，统一生成 `spill:` 引用，使用相同配额、存储和会话授权；未安装归档时明确标记省略内容未保留。会话与模型包装器共用拥有型等待的取消语义；项目规则在每次实际工具派发前检查，而不是缓存整批决定。

`session_routes` 只解析部署目录、处理 HTTP 鉴权并生成安全展示投影，启动流程复用 `application::sessions::SessionApplication`。不再保留本地 `sessions/store`、规则扫描器或会话执行包装器的第二份实现。Skills 的环境变量在应用层解析，实际根选择使用扩展接口及配置的工作空间。

固定模型与模型管理共用 Providers 工厂，支持 Chat Completions、Responses、Messages。固定入口通过 `AGENT_MODEL_PROTOCOL` 明确协议、`AGENT_MODEL_ENDPOINT` 提供完整端点；常规 Web 从设置页保存协议和密钥。新建空配置有效，不用假模型掩盖缺失；协议、能力和历史检查均保留扩展层职责。

Memory 与历史检索分别由 `memory` / `history-search` Cargo feature 默认装配，在 `environment` 按实际 Run 的 cwd 与会话身份绑定。`memory_routes` 只管理本机授权接口，不实现存储算法。长期文件默认位于宿主私有状态目录的 `memory/`（可由 `AGENT_MEMORY_DIR` 指定）；SQLite 索引位于 Sessions 目录。Agent 写工具默认关闭，权限设置重启生效；独立“记忆”页面提供用户主动管理和历史查询。接口、默认范围和容量见[上下文与三类记忆](../../docs/architecture/CONTEXT-MEMORY.zh-CN.md)。

右侧工作面板的应用接口位于 `workbench_routes`，复用 `workspace_routes` 解析可信文件根；`review` 仅执行受限只读 Git 查询，`terminal` 管理用户交互 PTY，`side_routes` 管理临时旁支并调用既有 Application。它们不修改 Core 执行协议。终端可用 `AGENT_TERMINAL=0` 禁用，文件和 Office 预览保留明确大小限制与独立沙箱。见[应用层接口与面板生命周期](../../docs/architecture/WEB.zh-CN.md)。

`conversation_metrics` 提供 `/api/workbench/session/{id}/statistics`，按已保存的会话档案和检查点汇总真实用量分项、模型/工具耗时和上下文估算；现有只读 ContextTransform 观测实际请求与绑定窗口，读数随检查点保存。指标查询不调用模型；供应商缺失缓存分项时命中率为空，缺少有效生成样本时速度为空。详见[展示数据与私有数据分离](../../docs/architecture/WEB.zh-CN.md#7-展示数据与私有数据分离)。

## 产品 UI 与计划接入

`GET /api/ui` 返回版本化的本地模块 ID 和 compiled/enabled/active/configurable/restart_required 能力状态，沿用产品鉴权与 CORS，不返回脚本 URL、执行历史或凭据。清单只选择当前构建内的 UI 模块；保存开关不即时改装活动 Host。前端未实现对应模块时报告该组件不可用，不把元信息当作可执行代码。

`planner` 为默认可选 feature；`environment` 在实际工作环境装配 Planner，普通模式不增加审批。`GET /api/sessions/{id}/plan` 返回有限安全状态，POST 只接收会话 revision、计划 revision 和枚举操作，不接收任意状态或 seed。运行期间禁止切换模式，提交后不允许用通用 Normal 操作绕过显式继续。计划投影与 Sessions 共用一次原子提交；空闲状态通过 Sessions 的通用 HostState 保存。关闭 Planner 保留已有计划供只读查看，仍处于计划模式的会话不能在未装配限制时续跑。

该产品接口不改变公共 Core 契约。更换传输时需显式接入产品接口，不能认为 Tauri Bridge 自动包含模式控制。二次开发见[UI 模块架构](../../docs/architecture/WEB.zh-CN.md#ui-modules)。

## 子任务委派

`subagent` 为默认可选 feature；`subagent_setup` 用既有环境 Runtime 和 Models 路由实现薄 Driver，子任务使用独立会话身份、父任务工作目录与收紧后的工具权限。默认模型来自父 Run 已固定的绑定；角色指定模型通过独立路由绑定，不修改全局默认。实现与许可证来源见 [subagent](../../packages/subagent/README.md)。

`GET/POST /api/subagents` 管理并发、深度与角色配置；私有状态目录中的 `subagent-settings.json` 使用文件锁和原子保存，配置变更重启生效。`AGENT_SUBAGENT=0` 锁定停用，`AGENT_SUBAGENT_CONFIG` 提供部署控制的完整配置，`AGENT_SUBAGENT_SETTINGS_PATH` 可覆盖保存位置。工具参数不能提供凭据、工作目录或任意模型端点。

`GET /api/sessions/{root}/agents` 及 `/{child}` 返回根会话内子任务和分页安全过程；`message/followup` 必须绑定仍活动的父 Run 与幂等请求身份，`interrupt` 必须匹配所见子 Run。顶层导航不列出子记录，普通会话启动接口不接受直接续跑子记录。主任务结束后的继续由新的主任务明确派发；停用组件、刷新和重启只恢复记录，不自动执行。关闭宿主先排空父子任务，再清理沙箱及底层 Runtime。

## MCP

`mcp` 为默认可选 feature，初始服务器列表为空，`AGENT_MCP=0` 锁定停用。`mcp_setup` 在启动时建立共享 SDK 连接；关闭时先排空父子任务和工作环境，再关闭 MCP。stdio 程序须预先安装，不根据模型参数动态安装或更换。

`GET /api/mcp`、`POST /api/mcp/servers`、`/api/mcp/delete` 和 `/api/mcp/reconnect` 复用鉴权、CORS 与有界请求。配置使用独立 `mcp-settings.enc`，可用 `AGENT_MCP_SETTINGS_PATH` 指定路径。密钥优先取 `AGENT_MCP_STORE_KEY`、显式应用密钥或 `AGENT_MODEL_STORE_KEY`，均未提供时在宿主目录生成独立 `.key`。复用 Models 的加密存储，不要求启用模型管理界面；密文与密钥同目录仍依赖文件权限保护。

配置变更和工具目录变化重启生效；页面只返回 env/header 的键名，不返回值，换端点或程序身份不能自动沿用旧凭据。宿主声明的只读工具可进入 Planner，其他 MCP 工具在只读 Sandbox 模式中被拒绝；MCP 进程和远端服务本身不受本机沙箱隔离。完整协议边界见 [MCP](../../packages/mcp/README.md)。

## 本地沙箱

`sandbox` 为默认可选 feature，配套 Tools 使用 `tools/sandbox`；当前 Web 默认 `workspace-write`，不加审批。`sandbox_setup` 解析部署配置、在会话接纳时绑定策略，将其交给实际 cwd 的工具；原生实现位于独立 `packages/sandbox`。输入框模式与 Planner 独立，不能用取消沙箱绕过计划模式工具限制。

`GET /api/sandbox` 查询部署默认；`GET/POST /api/sessions/{id}/sandbox` 查询或按会话 revision 修改模式。模式只能由可信用户管理入口选择，不接受根目录、任意 metadata 或自动提权参数。默认策略也随检查点持久保存；ProductSink 一次提交计划投影、沙箱策略和原检查点。正在执行的 Run 不受后续 UI 或会话变更影响。

`AGENT_SANDBOX_MODE` 可锁定三种明确模式之一，`AGENT_SANDBOX=0` 可关闭装配，两者不能同时指定。组件开关按原规则重启生效；模式在空闲会话中保存后立即作用于下一 Run。关闭/未编译 Sandbox 时，之前要求限制的会话拒绝继续，不静默降级。临时侧边任务继承父会话沙箱策略，拥有独立临时资源；用户直接 PTY 终端不受该 Agent 工具模式约束。

Server 在同步 main 最前面分流原生 helper，不额外维护 Node 进程或第二套构建目录。先关闭任务，再清理 Sandbox；删除会话只释放该会话临时授权，不删除工作目录或反复恢复 Windows 常驻工作区 ACL。机制、系统依赖及部分限制见 [sandbox](../../packages/sandbox/README.md)，UI 与保存路径见[应用层架构](../../docs/architecture/WEB.zh-CN.md)。

必要验证：`cargo test -p server`、`cargo test -p server --no-default-features --lib`。真实 Web 重启流程见 [浏览器测试](../web/test/README.md)。
