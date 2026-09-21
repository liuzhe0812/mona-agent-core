# 交付报告 · v0.3.0

## 本次实际更新

在v0.2源码基础上补四类通用能力：多模态内容/工具输出、Run与轮次工具视图、可等待检查点、模型参数与私有协议保真。Memory/Planner/示例和组合根同步迁移，API协议升到3，UI流协议保持2，两个bridge仍独立可选。

这是**待Rust编译验收的源码候选版**，不是已验收发行版，不是Mona业务迁移完成包，也不是完整桌面安装包。

## 实际执行的检查

- Node 22.16.0：本轮重新执行37项JS测试，通过；日志 `verification/client-tests.tap`。它们使用mock HTTP/Tauri，不代表Rust服务或原生WebView已联调。
- TypeScript 5.8.3：本轮重新执行客户端声明/类型检查，通过；日志 `verification/client-types.txt`。
- Python：TOML/JSON、目录/模块、生产依赖图、bridge可选性、文档链接和测试清单检查；结果见 `STATIC-CHECK.json`。
- Rust源码额外做词法和括号配对扫描。该扫描不是Rust语法/类型/借用检查，不可以替代cargo/rustc。
- 打包时核对SHA256与ZIP可读取性。MANIFEST记录交付快照，后续格式化/修改会改变哈希。

## 明确未执行

当前环境依然没有cargo/rustc；本轮下载尝试失败，环境记录见根目录verification-environment.txt。因此**Rust构建、143项Rust测试、Rustfmt、Clippy、原生Tauri、真实模型、真实持久化后端和性能基准都没有执行**。

新增53项Rust测试，总143项，覆盖通用契约和失败路径。它们是测试源码，不是通过记录。没有生成Cargo.lock；需在可联网Rust环境解析依赖、保留锁文件并固定发布工具链。

## 没有做的事

没有加入Session/数据库产品、自动恢复、持久化UI事件、exactly-once、Mona/Python适配、业务工具、JEV、完整模型Provider集合或可见隐藏思维。

Resource是引用描述；默认Chat适配器遇到未解析资源会显式拒绝。图片支持只覆盖协议与转换，AQ==测试样本不是有效PNG的视觉验收。默认HTTP/Tauri仍是文本入口，附件授权与富请求由应用组装。

## 后续验收入口

1. 运行scripts/verify.sh或verify.ps1，包含新增generic_extensions离线示例；修正实际编译/测试发现的问题。
2. 单独开启Tauri原生feature，在目标Windows/WebView中验证；HTTP仍要检查真实浏览器、代理缓冲、鉴权和断线恢复。
3. 用真正支持图片/私有回传字段的模型端点验证，不能只用脚本模型。
4. 安装需要的CheckpointSink并做事务/超时/确认丢失/取消测试，明确恢复策略；Core不自动重放。
5. Mona的Provider、Python工具、会话、工作流、事件和脱敏适配在Mona側完成。

**“源码已补齐”不等于“已经编译并通过生产验收”；本报告不替代实际验收。**
