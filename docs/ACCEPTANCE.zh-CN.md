# 验收矩阵 · v0.3

状态区分：**已执行通过**、**已通过但仍需目标环境联调**、**未覆盖/需人工联调**。

| 能力 | 本次状态 | 验收入口 |
|---|---|---|
| JS文本/工具增量、最终项覆盖、游标去重 | 已执行通过 | reducer.test.mjs |
| SSE分包UTF-8/CRLF/多行/尺寸/截断 | 已执行通过 | sse.test.mjs |
| HTTP客户端鉴权header、游标、失败与取消订阅 | 已执行通过（mock） | http.test.mjs |
| Tauri客户端首包竞态、ACK、超时、订阅清理 | 已执行通过（mock） | tauri.test.mjs |
| TS客户端公共声明与两种实现可替换 | 已执行通过 | test/types.ts |
| Rust基础编译与旧58项回归 | 已执行通过（cargo test --workspace --all-targets --offline） | Cargo workspace |
| Rust公共流式契约/终态/Tool详情/不执行半成品参数 | 已执行通过 | api/runtime 测试 |
| Application幂等/输入去重/取消/容量/回放/隐私 | 已执行通过 | application 测试 |
| Axum实际Router路径/HTTP Body/SSE | 已执行通过 | packages/http-bridge/tests |
| Rust Channel单包ACK/归属/超时 | 已执行通过（headless） | packages/tauri-bridge/tests |
| HTTP创建的任务经Tauri通道看到同一状态 | 已执行通过 | apps/server/tests/shared_application.rs |
| Tauri2原生命令、权限生成与宿主装配 | 已通过原生 feature 编译，未运行 GUI | cargo check -p tauri-composition --features tauri --locked |
| Web 启动器与空模型设置页 | 已执行通过（隔离环境） | npm run dev:web；浏览器 smoke |
| 真实模型流/授权/取消/特殊错误 | 未联调 | 配置真实测试端点 |
| 高并发、代理缓冲、XSS/CSP、密钥与租户策略 | 需业务验证 | 压力和安全测试 |
| 重启恢复/持久化幂等 | 未实现 | 不作为本版能力 |

当前工作树已完成 Rust workspace 测试、文档测试、服务无默认 feature 编译、Tauri 原生 feature 编译以及 Web 启动器和空设置页验收。真实付费供应商流、原生 Tauri 窗口交互、高并发与生产安全策略仍需在目标环境联调；不能把本地模拟端点或 JS mock 测试等同于这些验收。


## v0.3 通用契约验收（Rust workspace 已执行通过）

| 范围 | 新测试/验收要求 |
|---|---|
| 内容 | 旧字符串序列化、内容块顺序、Base64编码规则、资源不冒充内容、富结果上限 |
| 工具错误 | is_error与structured一起保留，后处理不能改执行身份/状态 |
| 工具选择 | 每轮重新计算、Run上限、连续收紧、隐藏调用拒绝、最终权限不绕过、选择超时 |
| 检查点 | 单Run序号、快照不可变、intent先于动作、局部结果、派发前/结果后/终态确认失败 |
| 取消 | 取消后仍提交Unknown/最终状态；存储有独立短期限 |
| 确认 | 输入回执等待提交；重复sink注册拒绝；超大快照不进入模型/存储IO |
| 协议 | ProviderData命名空间、签名/数组保持、UI不泄露、参数白名单和模型允许列表 |
| 组合 | 辅助调用共享预算并继承选项，Planner子运行不扩大工具上限 |
| UI | UI协议仍为2；JS客户端37项mock测试与声明检查重新执行 |

真实端点必须测试可用图片、不同模型参数/签名协议、工具图片投影；示例AQ==只测试编码形状，不是有效PNG验收。持久化sink必须另外进行真实DB故障注入（写成功但确认丢失、事务失败、重启、未知工具结果核查）。本包不提供该后端或自动恢复。
