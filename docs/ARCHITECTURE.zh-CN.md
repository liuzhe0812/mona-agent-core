# 架构说明

当前架构统一维护在 `docs/architecture/`。从[开发者文档首页](README.zh-CN.md)选择阅读路线，术语和协议版本也在首页集中说明。

| 文档 | 内容 |
|---|---|
| [总体架构](architecture/OVERVIEW.zh-CN.md) | 三层职责、生产依赖、执行调用、装配选择和二次开发入口 |
| [核心执行机制](architecture/RUNTIME.zh-CN.md) | 运行时序、模型网关、工具状态、预算、取消、审计和可靠提交 |
| [上下文与三类记忆](architecture/CONTEXT-MEMORY.zh-CN.md) | 正式历史、模型投影、压缩、持久会话、长期记忆与原文检索 |
| [扩展接口与装配](architecture/EXTENSIONS.zh-CN.md) | 普通接口、薄 Plugin、生命周期、权限与组合约束 |
| [应用层与 Web/桌面装配](architecture/WEB.zh-CN.md) | Server、Application、Bridge、Client 和界面之间的边界 |

本页只提供导航，不维护另一份按版本追加的架构正文。
