# application

`application` 是与界面、传输和存储无关的任务调用门面，只依赖公共 `AgentRuntime`。管理启动、取消、输入幂等、实时订阅、有限回放、快照、终态和保留策略，不拥有模型实现或磁盘会话。

普通宿主调用 `AgentApplication::start_task(StartRequest)`；需要继续可信会话时调用 `start_task_with_history(request, history, metadata)`。后者接收宿主从已验证存储读取的正式 `Vec<Message>`，不是浏览器发送的 UI 快照。它移除历史中的 System 消息并使用当前 ApplicationConfig 的系统提示词、模型选项、工具上限和预算。Runtime 仍校验历史配对与接纳上限。

幂等键标识一次不可变的逻辑调用：同 key、同 prompt、同 metadata 返回原 Run；prompt 或 metadata 不同则冲突。宿主负责为每个会话轮次构造不同 key，并保证它对应的历史身份不变；Application 不复制保存整份历史来做重复比对。持久去重、会话 revision 和文件事务由外层宿主负责。

此方法只增加 Rust 可信调用入口。通用 HTTP/Tauri 的 `StartRequest` 不增加 history、metadata、权限或模型配置字段。正式 Web 宿主的实现见 [本地会话](../../docs/LOCAL-SESSIONS.zh-CN.md)。

默认保留最多 32 个 Run，已完成记录 600 秒惰性过期；单 Run 回放窗口、订阅和输入记录有界。这里的 TTL 与 forget 只释放内存运行视图，不表示外部宿主持久会话必须删除。完整语义和接口表见 [统一应用接口](../../docs/APPLICATION-API.zh-CN.md)。

验证：`cargo test -p application`，包含通用调用、可信历史、当前宿主策略和接纳限制。生产依赖不包含 Runtime；测试的 Runtime dev-dependency 不构成依赖倒置。
