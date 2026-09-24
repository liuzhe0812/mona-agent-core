# application

`application` 是与界面和传输无关的任务调用门面，通过公共 `AgentRuntime` 管理启动、取消、输入幂等、实时订阅、有限回放、快照和保留策略。默认不依赖存储、模型实现或具体 Runtime；可选 `sessions` feature 连接已有会话扩展，不复制磁盘实现。

普通宿主调用 `AgentApplication::start_task(StartRequest)`；需要继续可信会话时调用 `start_task_with_history(request, history, metadata)`。后者接收宿主从已验证存储读取的正式 `Vec<Message>`，不是浏览器发送的 UI 快照。它移除历史中的 System 消息并使用当前 ApplicationConfig 的系统提示词、模型选项、工具上限和预算。Runtime 仍校验历史配对与接纳上限。

幂等键标识一次不可变的逻辑调用：同 key、同 prompt、同 metadata 返回原 Run；prompt 或 metadata 不同则冲突。宿主负责为每个会话轮次构造不同 key，并保证它对应的历史身份不变；Application 不复制保存整份历史来做重复比对。持久去重、会话 revision 和文件事务由 `sessions` 扩展负责，宿主选择是否装配。

此方法只增加 Rust 可信调用入口。通用 HTTP/Tauri 的 `StartRequest` 不增加 history、metadata、权限或模型配置字段。正式 Web 宿主的实现见 [本地会话](../../docs/LOCAL-SESSIONS.zh-CN.md)。

默认保留最多 32 个 Run，已完成记录 600 秒惰性过期；单 Run 回放窗口、订阅和输入记录有界。这里的 TTL 与 forget 只释放内存运行视图，不表示外部宿主持久会话必须删除。完整语义和接口表见 [统一应用接口](../../docs/APPLICATION-API.zh-CN.md)。

模型协议或配置返回 Unsupported 时，应用使用固定的能力/参数不支持提示，引导检查模型设置；不将任意供应商或插件错误正文传给页面。该提示同样用于实时终态，不改变执行失败或重试语义。

`wait_report(run_id)` 是可信 Rust 宿主的只读等待入口，取得既有 Run 的完整结算报告；不启动第二套执行循环、不因观察者结束而自动取消任务。报告包含私有上下文，不由 HTTP/Tauri 通用接口透传。Web 的临时侧边对话用它保存本旁支的后续工作历史，浏览器仍仅接收安全快照，见[右侧工作面板](../../docs/ui/RIGHT-PANE.zh-CN.md)。

## 可选会话适配

启用 `application/sessions` 后，构造 `SessionApplication::new(store, app)`，调用 `start_turn(session_id, TurnRequest)`。Store、SessionSink 与 sessions::runtime 必须属于同一装配。适配器使用当前 ApplicationConfig 的真实初始/累计历史上限，并先预留当前系统消息成本；存储接纳失败不写入新轮次。接纳操作不因 HTTP/IPC 调用者断线而取消；相同已保存请求不会新建 Run。会话错误显式映射到现有 ApplicationError，不扩大通用 Bridge 的不可信输入面。

新轮次通过 `Store::prepare_checked` 在保存前调用当前 Runtime 的纯历史检查。不兼容私有模型历史映射为明确的“请新建会话”错误，不暴露签名、推理或端点。已保存请求的重发仍直接返回原轮次，不因切换模型重新执行。实际启动按绑定模型再检查，接纳后配置变化导致的失败只记录为失败输入，不会发出不兼容请求。

这一适配可被 Web、Tauri 或其他产品复用；纯 Runtime 嵌入可只使用 [sessions](../sessions/README.md)，不引入本包。Server 中只有管理路由与安全展示投影。

验证：`cargo test -p application --no-default-features` 检查无会话依赖的基础入口；`cargo test -p application --features sessions` 检查当前宿主策略、系统消息预算、并发请求去重和启动调用者断开后的可靠接纳。生产依赖不包含 Runtime；测试的 Runtime dev-dependency 不构成依赖倒置。
