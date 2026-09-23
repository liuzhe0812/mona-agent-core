# 统一应用接口

## 1. 调用面

| 方法 | 输入 | 输出/语义 |
|---|---|---|
| start_task | StartRequest(request_id,prompt) | run_id,reused；立即返回，不等待回答完成 |
| cancel_task | run_id | signalled；只确认取消信号，不承诺已撤销副作用 |
| send_input | run_id,InputRequest(request_id,text) | InputReceipt(applied=true)；等待安全轮次边界正式接纳 |
| subscribe_events | run_id,after? | Subscription，逐个 StreamFrame 读取 |
| get_snapshot | run_id | RunSnapshot，有界可恢复 UI 状态 |
| get_result | run_id | outcome 未结束时为 null，结束时为 RunOutcome |
| forget | run_id | 只允许已结束任务；释放保留数据及对应启动幂等键 |
| shutdown | grace | 可信宿主调用，不通过桥接暴露 |

所有实际执行最终都落到注入的 `AgentRuntime`。Application 不认识模型厂商、具体工具、HTTP 或 Tauri。

## 2. 幂等不是永久 exactly-once

StartRequest.request_id 在**一个应用实例的保留窗口内**去重：相同 key + 相同 prompt 返回同一 run_id；相同 key + 不同 prompt 冲突。TTL 清理、forget 或重启后，这个约束不再成立。

InputRequest 在每个 Run 内独立去重：相同 key + 相同 text 共享同一个等待结果；不能因为 HTTP 连接断了就重新把文本推入运行。ack/applied 只证明加入当前任务记录，不代表模型已经执行了它的要求。

如果要跨进程/跨重启防止业务动作重复，需要持久化任务幂等和工具后端幂等键。本版不作这样的承诺。

## 3. 请求边界

对外只接受 prompt / text / request_id 与运行 ID、游标。禁止客户端通过请求覆盖 system prompt、模型密钥、工具集、权限、Host 配置和任务限制。Serde deny_unknown_fields 防止“看起来接受了其实忽略”的越权字段。

启动 key 限 1..128 ASCII 字母数字/下划线/连字符/点；输入 body 有显式长度上限。身份及资源额度来自可信 ApplicationConfig，而不是由任意客户端传入。

结果公开面排除 Transcript、RequestAudit 与 provider reasoning_content。错误文本在应用边界脱敏；工具业务输出本身仍可能含敏感数据，必须通过权限域、工具输出治理和 UI 转义进一步处理。

## 4. 查询保留策略

默认最多保留 32 个任务，已完成后 600 秒惰性过期。达到容量时返回 capacity，不默认启动无限任务。忘记正在运行的任务返回 conflict，客户端应先取消并等待终态。

已有订阅可以继续持有任务视图直到结束/drop；TTL 从注册表移除并不“撤回已经交给调用者的 Arc”。drop 应用会给活动 Run 发取消信号，但可靠关闭请显式 await shutdown，随后关闭 Host。

订阅与任务生命周期不同：一个 Run 可被多个客户端观察，任意一个客户端断开都不应影响其他客户端。两个桥接若共享同一个 Application，就共享取消、输入、结果和保留语义。

## 5. 错误

公共错误码为 invalid_request、not_found、conflict、capacity、closed、internal。HTTP 额外有 unauthorized。not_found 包含不存在、已过期、已被forget；客户端不能据此推断任务过去是否执行过。

run 的失败是 RunOutcome，不一定是传输失败。成功收到了 HTTP 200/SSE 终止，不代表业务任务成功；检查 outcome.status。

## 6. 留在应用产品层的能力

用户账号、workspace、聊天会话、审批记录、TLS终结、租户路由、持久化、跨设备同步以及完整 UI 不属于这个最小 Application 包。它是可复用的调用门面，不是又一个全功能 Agent 平台。


## 7. v0.3 不扩大不可信输入面

ApplicationConfig现在还可提供可信model_options和allowed_tools，构造每个Run时使用；它们不由StartRequest覆盖。模型密钥、endpoint与工具权限仍不从UI接收。

Rust调用方可以直接构造多模态RunRequest。默认HTTP/Tauri桥接不顺手增加任意文件读取/图片URL上传功能；需要资源输入的产品应在可信应用层验证附件并构造请求。UI流式协议仍是2，私有协议数据和检查点不在公共响应中。

配置了CheckpointSink时，追加输入applied还要等该次检查点确认；没有sink时语义仍是加入内存运行记录。无论哪种，都不表示模型已执行这个要求。

## 8. 可信宿主继续历史

`start_task_with_history(StartRequest, Vec<Message>, metadata)` 是 Rust 可信入口，不是公共 HTTP/Tauri 新请求字段。宿主提供已经验证的正式历史和逻辑上下文身份；Application 使用当前系统提示词、模型和权限配置，将新 prompt 追加为新的 Run。历史 System 消息不覆盖当前宿主指令，历史大小和工具配对仍由 Runtime 校验。

同一 key 的 prompt 或 metadata 变化会冲突；宿主负责确保逻辑 key 对应不可变的历史身份，Application 不为重复比对再复制一份历史。内存 TTL/forget 语义不变。跨重启会话、revision、持久请求去重与文件锁由 `sessions` 扩展实现，见[本地会话](LOCAL-SESSIONS.zh-CN.md)。调用会话接口保存的历史不受内存 Run 过期影响，通用 `/v1/runs` 仍明确是临时任务入口。

启用 `application/sessions` 后，`SessionApplication` 连接会话扩展与同一个 AgentApplication。它使用当前配置预留系统消息成本，处理持久请求去重、启动、绑定及失败收尾；HTTP/Tauri 只需调用该接口，不复制会话启动流程。默认 Application 不引入 sessions。独立 Runtime 集成可直接使用 [sessions](../packages/sessions/README.md)，不经过 Application。
