# Packages · 包介绍

本页是可复用组件的统一入口：先了解每个包负责什么、何时接入，再通过链接查看具体用法。应用宿主与样例见 [Apps 与 Examples](../apps/README.md)，跨包依赖与执行机制见 [架构说明](../docs/ARCHITECTURE.zh-CN.md)。

项目分为核心层、扩展层和产品层（Web），分别提供最小执行底座、可复用 Agent 能力和 Web 产品。组件实现能力，公共接口定义边界，宿主负责组合。某场景必需不代表具体实现内置 Core；扩展不是样例的同义词。需要注册或统一生命周期时提供薄 Plugin，复用同一份实现。

## 包目录

| 包 | 职责 | 接入定位与说明 |
|---|---|---|
| [api](api/README.md) | 模型、工具、运行、事件和 Plugin 的公共契约 | 内层组件共用；见 [插件契约](../docs/PLUGIN-GUIDE.zh-CN.md) |
| [runtime](runtime/README.md) | 唯一执行循环、模型网关、工具调度、累计历史与本轮请求预算、权限与生命周期 | 核心层，可独立嵌入；见 [架构](../docs/ARCHITECTURE.zh-CN.md) |
| [providers](providers/README.md) | 具体模型协议、鉴权、请求与响应转换 | 宿主按模型协议选择；见 [内容与模型协议](../docs/CONTENT-AND-PROVIDERS.zh-CN.md) |
| [tools](tools/README.md) | 文件、命令和检索工具，以及可注入的流式输出归档接口 | 当前 Web 使用四工具默认组合；其他宿主按场景选择，沿用公共 `Tool` 接口 |
| [models](models/README.md) | 供应商设置、凭据、模型目录、默认选择和路由 | 可选模型管理组件，由可信宿主接入 |
| [sessions](sessions/README.md) | 线性持久会话、可靠检查点、工作集恢复和归档归属 | 扩展层；不依赖 Application/HTTP/Runtime 实现，宿主提供目录与授权 |
| [instructions](instructions/README.md) | 工作空间规则发现、来源注入与派发前刷新检查 | 扩展层；普通接口或薄 Plugin，可信历史来源可注入 |
| [skills](skills/README.md) | 技能发现、校验和模型可见的摘要目录 | 可选；指南和资源通过普通 `read` 渐进加载 |
| [compaction](compaction/README.md) | 摘要较早的已结算历史，导出可验证的压缩状态 | 可选；通过统一模型网关计费和取消，宿主保存/恢复状态，不覆盖完整档案 |
| [spill](spill/README.md) | 长文本结果与流式输出共用的归档、配额和生命周期 | 可选；宿主为同会话历史引用授权，不替代通用附件系统 |
| [application](application/README.md) | 任务调用管理、可信历史接纳、幂等、订阅、回放和保留策略 | 产品共享调用层；可选 sessions 适配连接会话扩展，见 [统一应用接口](../docs/APPLICATION-API.zh-CN.md) |
| [http-bridge](http-bridge/) | HTTP/SSE 传输与鉴权适配 | 浏览器或远程接入；见 [桥接指南](../docs/BRIDGES.zh-CN.md) |
| [tauri-bridge](tauri-bridge/) | Tauri Command、Channel 与 ACK 适配 | 本机桌面接入，可与 HTTP 独立选择；见 [桥接指南](../docs/BRIDGES.zh-CN.md) |
| [client](client/README.md) | JavaScript 协议客户端及 `RunView` 状态归并 | 不绑定 UI 框架 |
| [memory](memory/README.md) | 记忆后端接口和示例实现 | 可选扩展；见 [插件指南](../docs/PLUGIN-GUIDE.zh-CN.md) |
| [planner](planner/) | 通过已有执行接口进行上层任务编排 | 可选扩展；复用同一执行器，见 [插件指南](../docs/PLUGIN-GUIDE.zh-CN.md) |

Rust crate 名通常与目录一致；`tauri-bridge` 的 Cargo 包名为 `tauri-plugin-bridge`，工作区别名仍为 `tauri-bridge`。JavaScript 包名为 `client`。

## 默认装配与使用细节

正式 Server 的四个基础工具固定提供，三个检索工具按配置追加；Skills 默认关闭，compaction/spill 默认启用。项目规则 `instructions` 是默认启用、可关闭的独立扩展；上下文来源预算、压缩状态及会话归档之间的边界见[上下文管理](../docs/CONTEXT-MANAGEMENT.zh-CN.md)。包本身的接口与正式宿主的装配策略分开维护，配置来源、覆盖规则和重启生效行为见 [能力装配与管理](../docs/CAPABILITY-ASSEMBLY.zh-CN.md)。

各包 README 记录其公共接口、接入示例、配置、限制和必要验证方式。本页只保留摘要与链接，具体参数、限额和错误语义以所属包文档为准；尚无包内 README 的条目暂链接源码目录及现有契约说明。

维护要求见 [AGENTS.md](../AGENTS.md#包介绍与文档维护)：包或公共行为变化时，同步受影响的包介绍；根 README 保持可发现的概览入口。
