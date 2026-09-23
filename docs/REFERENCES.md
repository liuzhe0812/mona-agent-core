# 架构参考来源

核对日期：2026-09-20。用于独立实现的概念参考；没有逐行移植上游代码，也不承诺上游插件或wire protocol兼容。

## 本次流式与传输

- OpenAI Codex App Server： https://developers.openai.com/codex/app-server/ （访问时转向 https://learn.chatgpt.com/docs/app-server ）
  - 借鉴 item/started、item/completed、独立增量和最终工作项状态；不复刻其完整JSON-RPC或thread体系。
- Tauri Calling Rust： https://v2.tauri.app/develop/calling-rust/
  - Command、Channel，前端向Rust调用与流式输出。
- Tauri Plugin Development： https://v2.tauri.app/develop/plugins/
- Tauri Capabilities： https://v2.tauri.app/security/capabilities/
- Tauri IPC Channel： https://docs.rs/tauri/latest/tauri/ipc/struct.Channel.html
- Tauri Plugin Builder： https://docs.rs/tauri-plugin/latest/tauri_plugin/struct.Builder.html
- Tauri plugin build示例： https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/dialog/build.rs
- Axum SSE： https://docs.rs/axum/latest/axum/response/sse/index.html

## 保留的长期参考

- Pi： https://github.com/earendil-works/pi ，Agent接口与上下文/执行事件边界。
- DeepSeek Harness： https://github.com/deepseek-ai/deepseek-harness ，服务定义、插件装配、生命周期和作用域思路。
- Tokio Channels： https://tokio.rs/tokio/tutorial/channels ，有界队列与背压。
- Rust Reference ABI： https://doc.rust-lang.org/reference/items/external-blocks.html ，原生Rust动态插件ABI的限制。

Pi/DSH参考沿用前版设计讨论；本次重点核对Codex/Tauri/Axum文档，不以这些链接证明当前源码已编译或性能已验证。


## v0.3 通用接口研究（本轮重新核对）

- Pi Agent类型： https://raw.githubusercontent.com/earendil-works/pi/main/packages/agent/src/types.ts
  - 参考文本/图片工具内容、结构化工具详情、执行前后边界和每轮更新接口；不照搬其允许修改工具错误标志等全部策略。
- Pi模型类型： https://raw.githubusercontent.com/earendil-works/pi/main/packages/ai/src/types.ts
  - 参考模型选项、图片内容以及签名等Provider回传数据的需求；本项目使用独立的命名空间载荷。
- DSH工具管道： https://raw.githubusercontent.com/deepseek-ai/deepseek-harness/master/docs/subsystems/tools.md
  - 参考模型可见声明与宿主执行元数据分离、规范化输出、受控管道；不移植完整DSL/动态作用域系统。
- LangGraph Persistence： https://docs.langchain.com/oss/python/langgraph/persistence
  - 参考执行检查点与跨任务存储的分离；本项目只实现提交边界，不实现LangGraph式完整持久化/恢复系统。

这些是同类需求的参考，不是通用Agent的强制行业标准。实现语义与上游并不完全一致；代码仍以本包测试和文档为准。源代码参考链接不证明Rust构建已通过。

## 2026-09-23 本地会话

- Pi 会话格式：https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md
- Pi SessionManager：https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/src/core/session-manager.ts
  - 参考工作目录分组、持久会话身份与打开/继续语义；不移植分支树、不承诺文件兼容。
- DSH 持久化后端：https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/session/session-persistence-jsonl/README.md
- DSH 检查点策略：https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/session/session-checkpoint-policy/README.md
  - 参考持久化与执行确认分工、单写者和失败关闭；本项目复用现有同步确认的完整 RunCheckpoint，不复制整套事件存储框架。

实现与上游差异、实际测试入口见[本地会话](LOCAL-SESSIONS.zh-CN.md)。

## 2026-09-23 上下文修复

- Pi Compaction：https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/compaction.md
  - 参考摘要和覆盖边界持久化、摘要加近期消息重建以及整组工具边界保护。
- DSH Token Meter：https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/llm/token-meter/README.md
  - 参考请求/路由限定的计量复用、计量不执行模型与不决定循环。
- DSH Compaction：https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/compaction/compaction/README.md
  - 参考持久替换与源范围核验，保留原始事实；不移植完整事件框架。
- DSH Agent Instructions：https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/context/agent-instructions/README.md
  - 参考有界适用目录规则、文件操作发现与恢复校对；本实现明确采用 AGENTS 优先/CLAUDE 回退，不自动读取工作空间外规则，也不悄悄裁切规则。

Pi 文档已直接读取；DSH 使用可获取的官方仓库文档和官方仓库检索内容，部分 Raw 页面直连不可用。独立实现及验证边界见[上下文管理](CONTEXT-MANAGEMENT.zh-CN.md)。
