# planner

可独立装配的单 Agent 计划与进度管理。Agent 在同一条执行上下文中建立计划、调用业务工具、更新进度；本包不调用模型，不派发子 Run，不根据清单自动执行步骤。默认普通模式无需审批；显式计划模式的配置和宿主接口可供 CLI、桌面或 Web 接入。

生产依赖只有公共 `api` 和基础库，不依赖具体 Runtime、Sessions、Memory、数据库或 UI。标准 Web 默认装配 Planner，配套产品 UI 通过插槽提供模式选择与计划面板；本包本身不包含页面、审批服务或操作系统沙箱。

## 接入

```rust,no_run
use api::{AgentExecutor, Model, Result, RunRequest};
use planner::Planner;
use std::sync::Arc;

async fn run(model: Arc<dyn Model>) -> Result<()> {
    let planner = Planner::default();
    let mut host = runtime::HostBuilder::new()
        .model(model)
        .plugin(Arc::new(planner.plugin()))
        // 业务工具由宿主另行注册、授权；Planner 不内置任何业务工具。
        .build().await?;
    let report = host.engine().execute(RunRequest::new("分析任务并按需维护计划")).await?;
    println!("{:?}", report.status);
    host.shutdown().await
}
```

`Planner::new(PlannerConfig)` 返回可复用实例；`plugin()` 同时注册它的 `context()`、`selector()`、`policy()` 和 `tools()`。不使用 Plugin 时可分别注册这四部分，但计划模式要保留全部选择和派发检查，不能仅挂一段提示词。每个 Host 只装配一份 Planner；工具名和来源名不能被另一组件复用。

普通接口 `PlanSnapshot::update/submit/enter_plan_mode/resume_execution/reset` 是有界、无 I/O 的纯状态变换：输入修订号必须与接收快照一致，成功返回新快照，失败不修改原值。实际运行状态仍在已装配的 Planner 内；不能通过修改外部快照改变正在执行的 Run。

## 模式与工具

| 模式 | 行为 | 可用计划工具 |
|---|---|---|
| `Normal`（默认） | 简单任务直接回答；复杂任务按需列计划并继续，保持原有工具权限和预算 | `plan_read`、`plan_update` |
| `PlanOnly`，尚未提交 | 先调研、形成方案，不执行业务变更，也不把步骤标成已执行 | 上述工具与 `plan_submit` |
| `PlanOnly`，已提交 | 提示模型展示原方案并结束回复；不自动切换执行模式 | 只允许 `plan_read`；同批提交后的其他调用也被策略拒绝 |

`PlannerConfig.initial_mode` 只决定没有绑定状态、也没有历史计划时的初始模式。显式进入或恢复模式应使用 `bind_state`，不能期望修改配置覆盖已经恢复的计划。

计划模式下的业务调研工具必须同时满足：位于宿主明确配置的 `planning_tools` 集合、声明 `side_effects=false`、已注册并处于当前 Run 和上游选择器允许的集合。默认调研集合为空，不按名称自动开放 Shell，也不解析命令字符串推测“只读”。这些检查只约束本 Runtime 调度的可信工具，不隔离插件、辅助调用或宿主进程本身。

| 工具 | 参数与结果 |
|---|---|
| `plan_read` | 无参数，返回当前快照与 revision |
| `plan_update` | `revision`、`goal`、完整 `steps`，可选 `explanation`、`new_plan`；整批验证后替换 |
| `plan_submit` | `revision` 与完整 Markdown `plan`（以 `# ` 标题开始）；保存待查看方案，不代表获准执行 |

计划工具只改变 Run 内的协作状态，因此声明为无业务副作用；保存随普通工具结果的检查点完成。它们仍受 Schema、工具选择、策略、取消、结果大小等现有约束。`plan_update` 不是文件写入或长期 Memory 写入授权。

## 步骤、修订与完成语义

每步有稳定的 `id`、`text`、`status`；状态为 `pending`、`in_progress`、`completed`，最多一步 `in_progress`。更新提交完整清单，不能根据显示文字中的勾选标记修改状态。

已完成步骤保留身份、正文、相对顺序和完成状态。添加、删除、改写、重排剩余步骤，以及将进行中步骤退回待处理，必须附原因。现有计划的目标不能被普通更新悄悄替换。所有步骤完成后，`new_plan=true` 可显式开始新任务；放弃未完成计划或清空状态由宿主调用 `reset`，历史记录不被删除。

新快照保留最近一次调整原因；历次更新可从正式工具历史查看。计划提交、仍处于 `PlanOnly` 时，不允许模型修改清单，宿主可请求继续规划后再次提交。`awaits_host()` 表达这个协作状态，不是 Core 的暂停状态。

宿主发起执行后，`proposal` 仍保留已提交的完整 Markdown 作为基准，避免其中没有写进步骤标题的约束在压缩后丢失。普通进度更新不改写这份正文；当前 `steps` 与 `explanation` 反映后续获准调整。开始新计划或由宿主重置时才清除旧基准。

**清单完成是 Agent 的进度声明，不是业务验收结果。** Planner 不读取工具成功率来自动打勾，不因 Run 结束而自动完成剩余步骤，不因取消而回滚业务事实。中断时原 `in_progress` 保留为待核查的进度声明；宿主应结合 Run/工具终态呈现，不解释为后台还在执行。

## 宿主切换与产品接入

`bind_state(&mut RunRequest, &PlanSnapshot)` 把状态放入既有可信 metadata 的 `planner.seed`。它校验快照及 metadata 总量，不改变用户历史、权限、模型或预算。进入计划模式可绑定 `PlanSnapshot::new(PlanMode::PlanOnly)`；已有计划使用当前快照的 `enter_plan_mode(revision)`，同时撤销旧提交、允许细化。

方案提交并结束 Run 后，宿主拿到实际已确认状态，调用 `resume_execution(revision)` 得到普通模式的新快照，再绑定到**同一会话的下一轮**。新轮次继续传递原任务历史。这里的新 Run 对应用户下一轮，不是给每个计划步骤新建 Run。

模式方法不是审批系统：宿主负责鉴权、读取最新状态、对用户看到的版本作冲突检查，并串行化同一会话的启动。旧快照对象上的方法不能自行发现外部已发生的修改；不得拿缓存版本调用后就跳过宿主的会话 revision 检查。模型没有更改模式、选择会话或自我批准的工具参数。

等待用户期间无需挂住 Run、调用额外模型或启动后台任务。`plan_submit` 通过指导要求结束回复，派发策略阻止后续业务调用；若模型仍反复读计划，既有运行预算仍会终止循环，不保证模型一定在下一条回复自行结束。

完整的离线装配例子见 [`examples/demo/src/bin/planner.rs`](../../examples/demo/src/bin/planner.rs)：计划、提交、宿主显式继续、真实加法工具、完成。没有 UI 弹窗或自动批准服务。

## 状态、保存与恢复

运行中快照按实际 `run_id` 隔离。`live_snapshot(run_id)` 供可信宿主查看暂态进度，可能领先于保存确认；`finish` 清理后返回 `None`。共享同一 Planner 不会按目录或模型身份自动共享计划，容量满也不驱逐其他活动 Run。

每次成功的计划工具结果包含 `structured.planner = { run_id, call_id, state }`。现有可靠检查点保存这些结果；`planner.plan` 展示详情只是有界 UI 投影，不是状态正本。关闭或丢弃展示信息不影响恢复。

| 接口 | 使用条件 |
|---|---|
| `recover_history(messages)` | 宿主授权的完整、已确认正式历史；只读取匹配计划工具调用的成功结构化结果，不读取助手正文、搜索结果或摘要中的伪造记录 |
| `recover_checkpoint(checkpoint)` | 宿主取得的确切已确认检查点；合并绑定 seed、规范历史和工具批次内已经结算的结果，按计划 revision 恢复，不把 intent/Unknown 当成功 |
| `bind_state(request, state)` | 将选定会话的当前状态带到下一 Run；允许在工作历史已压缩时仍保留精确计划 |

持久宿主应在启动时绑定状态，并优先从**最新已确认检查点**恢复。只改模式、尚未调用计划工具时，变化保存在 checkpoint.metadata 中，仅扫描消息不能恢复它。结合 Sessions 时，使用现有 Store/SessionSink/运行包装保存，再从 `Document.body.checkpoint` 恢复；无需给 Sessions 增加 Planner 专属字段或第二份文件。

未安装可靠保存时，可由宿主保留 `RunReport.transcript` 和自己绑定的状态；这不提供重启恢复。保存失败后不要从看似成功的实时详情或失败报告推断已经落盘，应检查实际确认状态。确认丢失仍可能已提交，不能盲目重放业务动作。

来源和工具结果参与原有请求/历史预算。Compaction 可摘要旧计划调用，但当前 `planner.state` 来源在压缩前计量并保留；跨 Run 必须从原档案或检查点提取快照，再绑定到已压缩工作历史。结果变换器不能丢弃成功计划结果的 `structured.planner`，也不能将其中正文改写成另一份“计划”。恢复遇到实际计划记录损坏、缺字段、截断或冲突时明确失败，不假装空计划。

### 标准 Web 的可靠状态投影

Server 的 `planning` 适配器将当前计划保存在 Sessions 的通用 `Body.state["planner"]` 中，不改变本包与 Sessions 的生产依赖。`PlanningSink` 从正在确认的检查点派生计划，调用 `commit_with_state` 在一次会话文件替换中保存二者。空闲时的用户模式切换使用会话 revision 和计划 revision 双重检查，不等待下一次模型调用才保存。

接纳新轮次时，`SessionContext` 在同一会话锁下读取当前状态并绑定 seed，然后交给原 Runtime。运行中拒绝模式切换；组件已关闭但旧会话仍处于 PlanOnly 时拒绝续跑，不能因为 UI 消失就绕过限制。页面通过安全计划查询恢复状态，不把实时 `planner.plan` 详情当作唯一记录。

默认执行模式不增加逐次审批；输入器选择计划模式后可使用宿主明确列出的只读工具。提交方案后的“按此计划执行”由页面先保存宿主确认的模式，再在同一会话发起下一轮；“继续修改”恢复规划状态，用户输入修改要求。关闭面板仅释放展示资源，不执行、取消或删除任务。

## 容量与限制

默认最多 64 个活动 Run，可配置为 1–1024；每份计划最多 32 步、单步正文 1 KiB、目标和调整原因各 2 KiB、提交方案 6 KiB，整个序列化快照不超过 12 KiB。字节限制按 UTF-8 和 JSON 转义后的实际成本检查，不截断半条步骤或半份方案。Run 的 metadata、工具结果及上下文限制更小时仍以其为准。

本包不提供跨会话共享清单、后台自进化、多 Agent 调度、任务自动续跑、强制逐步验证或审批/沙箱。可信宿主决定从哪个会话取状态；计划内容本身不能扩大权限。标准 Web 是否开放新操作，以宿主实际装配和当前会话状态为准；UI 被隐藏不能代替服务端限制。

## 设计参考

参考 [DSH todo_write](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/todo/tool-todo/src/index.ts) 的结构化整表更新、明确进度状态，以及 [DSH plan-mode](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/plan/plan-mode/README.md) 对协作模式与审批/沙箱的职责区分。DSH 的计划模式自身是软指导；本包通过现有选择器和派发策略另外收紧获准的调研工具，不复制其审批框架。

参考 [Pi Todo 示例](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/examples/extensions/todo.ts) 从原生工具结果重建状态，以及 [Pi Plan Mode 示例](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/examples/extensions/plan-mode/index.ts) 的可选规划阶段。没有照搬其正文 `[DONE:n]` 解析、Shell 字符串白名单或 UI 交互实现。

## 验证入口

`cargo test -p planner` 覆盖状态、限额、修改原因、计划模式、同批派发检查、取消、跨 Run 隔离、检查点失败、独立进程重开及实际 Compaction 组合。`cargo run -p demo --bin planner` 运行离线样例。测试使用受控模型；它验证执行边界，不证明真实模型一定合理拆分任务、正确判断完成或自动选择何时规划。
