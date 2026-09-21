# 更新记录

## 0.3.0 · 通用能力契约（源码候选版）

- API_VERSION=3；UI STREAM_VERSION保留2；检查点协议1。
- Content和ToolOutput：文本/图片/资源描述、structured与is_error，富结果超限明确失败。
- Run工具上限和ToolSelector：逐轮可见工具选择，执行集合核对，仍保留冻结注册表与最终权限。
- 可选CheckpointSink：串行不可变快照、派发前intent、部分结果、终态确认，失败不盲目重试。
- ModelOptions和ProviderData：受控参数、命名空间私有回传、Chat适配扩展与显式不支持错误。
- Memory/Planner/示例/组合根同步迁移；Application支持可信工具/模型默认值，桥接请求不扩权。
- 不加入Session数据库、恢复引擎、Mona业务/FFI/JEV；新增测试、通用性判定和迁移文档。
- Rust构建/测试尚未执行；具体可执行验证结果见DELIVERY。

# Changelog

## 0.2.0 — 2026-09-20 — source candidate

- Breaking: API_VERSION/STREAM_VERSION 2，RunHandle/RunSession/AgentRuntime 进入公共API。
- Core新增工作项开始/更新/完成、文本/参数/工具日志增量、完整有界终态与原子快照。
- ToolProgress增加有界命名空间详情；不加入编码、审批、JEV等业务分支。
- 新增独立agent-application：任务注册、窗口幂等、取消/输入、UI-safe结果、回放/恢复、保留上限。
- 新增独立HTTP/Axum+SSE桥接与Tauri2 Command/ACK Channel桥接；均只调用统一Application。
- 新增无框架JS/TS客户端和共享RunView，HTTP与Tauri可分别导入。
- 新增HTTP演示组合根、Tauri装配函数/capability样例、迁移与接入文档。
- 原生Tauri以feature选择；默认构建不把原生GUI依赖引入Core。
- 本环境执行JS/TS与文件结构检查；Rust编译、Rust测试、原生Tauri、真实模型验收未执行。

## 0.1.0 — 初始候选版

Rust API/Core/providers，默认ReAct、插件宿主、Memory/Planner示例、取消/预算/工具校验、中文方案与测试源码。首次交付未完成Rust编译验收。
