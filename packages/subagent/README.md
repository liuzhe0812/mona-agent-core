# subagent

可复用的子任务委派扩展。依赖公共 `api` 和既有 `sessions`，不依赖 Runtime 实现、HTTP、Web 或具体模型供应商；没有第二套 Agent 执行循环。普通接口是 `Service` / `Driver` / `Binding`，薄 `SubagentPlugin` 仅注册工具和上下文生命周期。

## 代码来源与复用

`src/execution.rs` 从 OpenAI Codex 的 RAII 并发许可实现移植，并改为原子检查与预留。固定来源为提交 `aa380897f67b91e1a47d530d7286d497b6726d3f` 下的 [`agent/control/execution.rs`](https://github.com/openai/codex/blob/aa380897f67b91e1a47d530d7286d497b6726d3f/codex-rs/core/src/agent/control/execution.rs)。许可及修改说明见 [LICENSE-CODEX](LICENSE-CODEX)、[NOTICE](NOTICE)。不整体引入 `codex-core`：它的线程、配置、模型协议和持久日志绑定 Codex 自身；这里复用 Mona 的 Runtime、Sessions、模型路由、取消、预算和安全展示。

## 执行合同

| 工具 | 行为 |
|---|---|
| `spawn_agent` | 异步启动并返回子会话身份；明确任务、角色和 independent/fork 上下文 |
| `send_message` | 把补充信息保存到子会话有界 inbox，下次模型请求可见；不代表已读，也不启动空闲子任务 |
| `followup_agent` | 在同一空闲子会话中开始下一轮，保留其已确认历史；活动时拒绝，不暗中中断 |
| `wait_agent` | 等待终态与结果，每次最多 16 个子任务、20 秒；等待超时不取消子任务 |
| `interrupt_agent` | 停止准确的子任务执行并等待结算，不取消兄弟、不删除记录、不回滚文件 |
| `list_agents` | 查询已保存的子任务状态、身份和本轮用量，不把暂态当成完成 |

每次主任务默认最多 4 个并发子任务，委派深度默认 1。每个主会话累计最多 64 个子会话；同一子会话可在后续主任务中继续。角色 default/worker 继承父任务有效工具集合，explorer 默认只开放只读工具。角色白名单只能收紧，不能扩大父任务权限。角色定义由宿主配置；模型参数中不能上传 persona、工作目录、凭据或任意模型端点。

父子通过 `TaskControl::branch()` 共享总模型调用数、已报告 token 熔断和总期限。子分支的调用与用量逐级累计，上下文窗口观测独立；主会话统计不能被子请求覆盖。Token 熔断仍不是预付费保证。独立停止只取消子分支；主任务停止或完成时，其所属未完成子任务会停止并等待收尾。父任务试图带着未完成委派直接成功结束，会被记录为失败，不伪装为全部工作完成。UI 断开只是观察者消失，不自动停止执行。

## 上下文与存储

independent 只接收继承的系统规则、角色指令及委派任务，不自动复制父对话。fork 捕获父任务模型调用前的已配对工作上下文快照：可能包含已验证的压缩结果，但不包含随后生成的当前工具批次；不是父子共享可变历史。跨模型使用前继续执行模型历史兼容性和容量检查，不删除私有协议数据来强行切换模型。

子会话复用 Sessions 文件事务、检查点和重启恢复。`initialize_history` 保存初始历史但不伪造已完成轮次；root/owner 关系和角色快照随记录保存。补充消息使用 `aux.subagent.inbox`，最多 32 条、32 KiB，不静默丢弃。任务/单条消息最多 16 KiB。工具结果有界；截断明确标记，完整已确认工作过程由宿主安全分页展示。子任务完成声明不等于业务验收，主 Agent 仍须核对产物。

重启后只恢复已确认记录，中断状态不自动重跑。重复的 spawn 操作身份与内容相同则返回既有子会话，不再次执行；同身份不同内容拒绝。后续任务必须由当前主任务明确派发，共享当前主任务预算。主会话删除后由宿主调用 `purge_deleted_root` 清理子记录并释放沙箱临时资源；多文件清理失败会显式报告，不声称跨文件原子删除。共享工作区的用户文件不会因此删除。

## 嵌入方式

1. 创建并持有同一个 `Arc<sessions::Store>` 和 `Arc<Service>`。
2. 实现可信 `Driver::prepare(parent, role)`，返回已有 `AgentRuntime`、实际模型绑定及继承的权限/环境 metadata。禁止复用父 Session/Turn 的写入身份。
3. 用 `service.binding(driver)` 安装 Plugin，或分别装配其 ContextTransform/ToolSelector/tools。Runtime 的 CheckpointSink 和 `sessions::runtime` 必须使用同一个 Store；宿主原有副作用授权仍然生效。
4. 源码中的默认 Server Driver 使用弱引用绑定环境 Runtime，避免 Service/工具/执行器强引用环。默认继承父运行已固定的真实模型；角色指定模型时使用 `ModelManager::runtime_for`，不修改全局默认模型。
5. 宿主停止新接纳、结束父任务后调用 `Service::shutdown()` 并等待，再关闭底层 Runtime 与存储。

ContextTransform 的临时 hook 取消令牌不能保存为子任务生命周期令牌；Binding 自己持有从 capture 到 finish 的范围，并与 Task 总期限组合。子 Agent 不能修改父计划；标准宿主为其绑定自己的计划状态。计划模式不通过委派扩大工具权限。共享目录没有自动 worktree 或合并隔离；主 Agent 必须分配非重叠工作，冲突仍由现有文件版本/写入校验及最终集成处理。

## 产品接入与限制

标准 Web 默认装配，`AGENT_SUBAGENT=0` 可部署锁定关闭。设置页“子 Agent”配置并发、深度和角色模型/工具；修改保存后重启生效，已记录角色及已有任务不被后台改写。右侧“子 Agent”页面展示真实记录、执行片段、补充消息和停止操作。主任务已结束时，继续操作回到主会话创建明确的新任务，不绕过其预算独立启动子任务。

不包含外部 Codex/Claude Code CLI、任意 Agent 群聊、自动工作树/合并、长期脱离父任务运行或通用审批。它是可选扩展，未装配时核心单 Agent 执行仍然可用。

验证：`cargo test -p subagent`、`cargo test -p api --test task_branches`、`cargo test -p sessions --test initial_history`；`npm run test:web:subagent` 检查正式宿主与浏览器的完整委派链路，具体环境与边界见 [Web 测试说明](../../apps/web/test/README.md)。受控模型证明工程行为，不证明实际任务质量或付费模型收益。
