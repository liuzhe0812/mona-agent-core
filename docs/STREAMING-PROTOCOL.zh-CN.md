# 流式输出协议 v2

## 1. 与 Codex 的关系

借鉴 Codex app-server 的两点：工作项有稳定标识与开始/完成生命周期；完成项是权威状态，不能只依赖此前的增量拼接。具体参考链接见 [来源](REFERENCES.md)。

本协议不是 Codex 兼容 JSON-RPC。我们的 run 是单次执行；step 是一次 ReAct 模型轮次，不能把 step 与 Codex turn 混为同一概念。没有实现 Codex 的 thread、审批问答、所有 item 类型或可见推理摘要。

## 2. 三层协议

- Core `EventEnvelope`：protocol_version=2、run_id、seq、event。每个 Run 序号从 1 连续递增。
- Application `StreamFrame`：event、snapshot 或 fault。两种桥接传递相同内容。
- Tauri 外包装：subscription_id、delivery_id、frame。delivery_id 只负责这个 Channel 的 ACK，**不是** Run 的回放游标。

`API_VERSION=6` 是 Rust 插件接口的兼容检查；`STREAM_VERSION=2` 是 UI 协议版本；包版本是 0.3.0，`CHECKPOINT_VERSION=1` 属于独立的检查点协议。三个值相关但不是同一个概念。

## 3. 事件类型

| type | 数据 | 前端含义 |
|---|---|---|
| run/started | 无 | Run 已开始 |
| step/started | step | 一次模型决策轮开始 |
| item/started | 完整 WorkItem | 建立卡片/文本块 |
| item/updated | 完整 WorkItem | 替换已有项；例如参数验证完成、工具进入执行、结构化详情变化 |
| item/agentMessage/delta | item_id,text | 暂态文本增量 |
| item/toolCall/argumentsDelta | item_id,call_id?,name?,delta | 暂态参数文本，不是执行命令 |
| item/toolCall/outputDelta | item_id,text | 工具日志/进度增量 |
| item/completed | 完整 WorkItem | 权威最终 UI 状态，覆盖旧项，不重复追加 |
| input/applied | 无 | 至少一条输入已经进入运行记录；具体输入请求以 InputReceipt 为准 |
| step/completed | step | 此模型轮次结束 |
| run/completed | RunOutcome | Run 结束，可以是成功、失败、取消、超时或限额 |

因协议错误、取消或超时结束时，尚未结算的项也会终结。工具 Pending→Skipped；已经 Running 但无法确认副作用→Unknown。不能把它们画成“已撤销成功”。

状态有两种时间含义：工具参数生成中是 Pending，经过权限并真正进入工具才变成 Running。前端不能把看到 tool call 参数当成已经操作了外部系统。

## 4. ID 与最终状态

每轮文本项为 `step-N-message`，工具项为 `step-N-tool-I`；I 是该轮从零开始的模型调用索引。真正的 tool call id 在完整参数到来后关联。item_id 只在 run_id 内唯一，DOM/缓存键必须包含 run_id。

`ItemContent` 是 agent_message 或 tool_call。工具项保存参数预览/完整已验证参数（超大时省略）、日志预览、有界 UiToolResult（不同于模型层的富ToolResult），以及命名空间详情。最终结果和执行日志是两种不同字段：不能简单拼接，否则会重复显示或误当同一种信息。

**“完整完成项”指全部有界 UI 字段都由完成事件给定，不代表包含无限长原始日志。**检查 truncated、arguments_truncated、output_truncated、original_bytes、pruned_items 与 ArtifactRef。ArtifactRef 只提供定位符，不能自动打开或执行；本版没有归档服务。

大于 64 KiB 的文本完成项有裁剪标记；最终 RunOutcome.output 仍受 Core 响应上限约束，可以与文本项预览长度不同。前端可把最终结果单独显示，不应把两者再拼接。

## 5. 恢复规则

客户端先创建 RunView，然后选择：

- `after` 未提供：先拿当前 snapshot，不追求重放已经过去的打字效果。
- `after=0`：从第一条事件重放；如果已过保留窗口，改发恢复 snapshot。
- `after=N`：表示已完整应用到 N，继续拿 N+1。比当前序号更大的 N 是无效请求。

收到 event 时验证版本、run_id、seq。seq<=当前值可忽略重复；出现缺口不要继续拼接，重新请求 snapshot 或以当前游标重订阅。收到 snapshot 时用它替换本地投影，并将游标移动到 snapshot.seq。原因包括 initial、cursor_expired、source_resync。

应用层因 Core broadcast Lagged 恢复时，会清除旧 journal 并设置恢复边界；慢客户端不能误以为中间无缺口。仍有序号的最后一条回放不等于窗口完整性保证。

HTTP SSE 的 `id:` 是帧对应 seq；重连通过 Last-Event-ID 或 after 查询参数，两个都给时必须一致。**原生浏览器 EventSource 无法方便设置 Bearer Header**，客户端使用 fetch 读取 SSE，不把 token 放 URL。

本版 JS 客户端不会自动重试启动或永久自动重连。断线后提供最后成功处理的 cursor，由应用决定重订阅策略；这避免断线时偷偷启动另一个任务。已完成游标可能没有后续帧，客户端可用 snapshot/result 判断结束。

## 6. 背压与取消

HTTP 订阅是 pull-based，慢网络只读取当前可发送的帧；窗口过期后快照恢复，不建立每个浏览器的无界队列。

Tauri Channel 每包需要前端完成处理后 ACK，默认 30 秒无确认就关闭该订阅。JS 原生客户端有可配置的空闲超时（默认 120 秒），用于发现通道失效；耗时很长的业务应调整或让工具适度报告进度。ACK 超时/空闲超时都不表示任务取消。

共享 RunView 是框架无关投影器。界面负责渲染、虚拟列表、批量更新和转义；不要每个 token 都重建整个页面，也不要将模型字符串直接写入 innerHTML。

## 7. 扩展显示

工具调用 `ToolProgress.set_detail` 返回可校验结果，键如 coding.diff，值为有界 JSON。它只能更新当前正在运行的工具项，不能篡改状态、调用身份、授权或创建任意控制命令。

本版没有通用“插件伪造 RunEvent”的入口，也没有让可丢弃 EventObserver 接管运行事实。审批、规划子任务和长期历史需另行定义语义后扩展，不通过命名空间详情偷偷实现。


## 8. v0.3兼容与数据隔离

Rust模型层新支持Content/structured/ProviderData，但UI帧仍是v2。UiToolResult与之前的文本JSON结构一致，图片和资源描述生成安全占位预览，structured和私有回传数据不自动透传；truncated表明未展示完整富结果。显式artifact定位符仍由宿主负责公开授权。

完成消息项表示模型文本已结算，不等于Run已成功落库。配置了检查点时必须继续等待run/completed：如果终态提交失败，模型文本可能已显示，但RunOutcome必须是失败，不能用“看到了回答”推断整个任务Completed。

已有浏览器/桌面客户端不需要处理model provider_data，不应把CHECKPOINT_VERSION/revision混成事件游标。JS客户端测试仍是传输mock测试，未代替Rust运行和真实桥接联调。
