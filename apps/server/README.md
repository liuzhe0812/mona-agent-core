# server

正式 Web 宿主组合根：装配现有 Runtime、模型与能力组件，管理部署配置、凭据、持久会话，提供鉴权后的 HTTP 管理接口与通用任务 Bridge。不实现第二套 Agent 执行循环。

标准本地入口是在仓库根目录运行 `npm run dev:web`。第一次通过模型设置配置供应商；会话自动保存到用户状态目录，浏览器左侧可打开、搜索、重命名、删除和继续对话。单独运行 `cargo run -p server` 时，由部署者提供有效 `AGENT_SERVER_TOKEN` 及模型/存储配置；`--demo` 仅用于明确标识的离线演示。

部署能力见 [能力装配](../../docs/CAPABILITY-ASSEMBLY.zh-CN.md)，模型配置见 [模型管理](../../docs/MODEL-MANAGEMENT.zh-CN.md)，文件位置、接口、安全与恢复边界见 [本地会话](../../docs/LOCAL-SESSIONS.zh-CN.md)。`AGENT_SESSIONS_DIR` 可覆盖会话根目录，工作空间仍来自 `AGENT_WORKSPACE_DIR` 或启动目录。会话实现来自 `sessions` 扩展，规则实现来自 `instructions` 扩展；其他嵌入者可直接使用，不必安装本服务。

`/api/sessions` 是持久会话入口；原有 `/v1/runs` 仍用于临时 Run/实时事件，不向不可信客户端暴露任意历史注入能力。会话存储是未加密本地文件，部署者负责用户目录权限和备份。重启恢复记录并标识中断，不自动重做工具副作用。

会话格式 2 区分完整档案和经校验的模型工作集；摘要及覆盖范围通过原有检查点同步保存，跨 Run/重启复用。项目规则 `instructions` 扩展默认装配，可通过 Agent 组件页或部署配置关闭；可选工具在独立的 Agent 工具页管理。同会话旧 Spill 的访问由宿主验证结构化归属后路由，不放宽底层 Run 隔离。详见[上下文管理](../../docs/CONTEXT-MANAGEMENT.zh-CN.md)。

Shell 的流式归档适配器与结果变换器共享同一个 `SpillPlugin::archive()`，统一生成 `spill:` 引用，使用相同配额、存储和会话授权；未安装归档时明确标记省略内容未保留。会话与模型包装器共用拥有型等待的取消语义；项目规则在每次实际工具派发前检查，而不是缓存整批决定。

`session_routes` 只解析部署目录、处理 HTTP 鉴权并生成安全展示投影，启动流程复用 `application::sessions::SessionApplication`。不再保留本地 `sessions/store`、规则扫描器或会话执行包装器的第二份实现。Skills 的环境变量在产品层解析，实际根选择使用扩展接口及配置的工作空间。

必要验证：`cargo test -p server`、`cargo test -p server --no-default-features --bin server session_routes::tests`。真实 Web 重启流程见 [浏览器测试](../web/test/README.md)。
