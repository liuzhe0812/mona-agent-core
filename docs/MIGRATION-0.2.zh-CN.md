# v0.1.0 → v0.2.0 迁移

这是 0.x 破坏性接口更新，不是把旧事件换个名字就全部兼容。

1. API_VERSION 由 1 升至 2；使用 PluginManifest::new 的插件自动用当前值，手填版本的插件需更新并验证。
2. RunHandle 移到 agent-api。Core 保留便利用再导出；新代码应依赖 AgentRuntime/RunSession，而不是 Engine 私有类型。
3. 原有 AgentExecutor.execute 保留，仅返回最终报告。Planner 可继续使用，但没有默认复合流式 Run；不要把它强行当成 AgentRuntime。
4. 原始 ToolStarted/ToolProgress/ToolFinished、文本事件替换为工作项生命周期及带 item_id 的增量事件。更新 EventObserver 的匹配分支。
5. EventEnvelope 加入 protocol_version；新 UI 应使用提供的 RunView 和完整 seq 恢复规则。不要只根据 event 文本追加字符串。
6. 采用 Application 后，桥接拿到的是 UI-safe RunOutcome/RunSnapshot，而不是完整 RunReport；可信调试仍可从 Rust RunSession.wait 取执行报告。
7. 新增应用层不会自动接管 Host 生命周期。Tauri/HTTP组合根保留Host，并按 Application.shutdown → Host.shutdown 收尾。
8. 单轮 max_tools_per_step 必须小于256，以保证当前轮工作项可完整保留；默认32未改变。
9. ToolProgress 新增可选 set_detail；默认实现返回不支持，不影响只上报纯文本的旧工具。需要此功能的工具应处理返回错误。
10. request_id 幂等、内存 TTL/journal/订阅限制只存在于新 Application；直接调用 Engine 仍是低级库模式。
11. 依赖版本约束从完全等号固定改为兼容范围，以便与 Tauri/HTTP依赖解析。首次真实构建生成 Cargo.lock，并固定交付工具链；本包没有伪造已解析锁文件。
12. HTTP示例新增 AGENT_MODEL_KEY；原有 demo 使用 AGENT_API_KEY，分别见 .env.example，密钥不会自动相互复制。

新增 Application/Bridge 是可选包；原来直接嵌入 Core 的 CLI 无需为了这次迁移强制启动服务。

JEV、动态决策策略、第二套循环均没有加入。新增会话持久化、复合规划流或审批时，应先定义通用语义，而不是在HTTP/Tauri桥接各写一遍。
