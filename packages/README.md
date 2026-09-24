# Packages · 包介绍

本页是可复用组件的统一入口：先了解每个包负责什么、何时接入，再通过链接查看具体用法。应用宿主与样例见 [Apps 与 Examples](../apps/README.md)，跨包依赖、执行机制和二次开发阅读路线见 [公开开发者文档](../docs/README.zh-CN.md)。

包按核心层、扩展层和产品层（Web）组织：核心提供执行底座，扩展提供可复用能力，产品层负责装配与交互。组件可通过普通接口使用，也可通过薄 Plugin 接入宿主。

## 包目录

| 包 | 职责 | 接入定位与说明 |
|---|---|---|
| [api](api/README.md) | 模型、工具、运行、事件和 Plugin 的公共契约 | 内层组件共用；见 [扩展契约](../docs/architecture/EXTENSIONS.zh-CN.md) |
| [runtime](runtime/README.md) | 唯一执行循环、模型网关、工具调度、累计历史与本轮请求预算、权限与生命周期 | 核心层，可独立嵌入；见 [核心执行机制](../docs/architecture/RUNTIME.zh-CN.md) |
| [providers](providers/README.md) | Chat Completions / Responses / Messages、能力校验、鉴权及私有历史回传 | 宿主按模型协议选择；见 [模型适配契约](../docs/architecture/EXTENSIONS.zh-CN.md) |
| [tools](tools/README.md) | 文件、命令和检索工具，以及可注入的流式输出归档接口 | 当前 Web 使用四工具默认组合；其他宿主按场景选择，沿用公共 `Tool` 接口 |
| [models](models/README.md) | 协议与供应商设置、凭据、模型能力、目录及固定运行路由 | 可选模型管理组件，由可信宿主接入 |
| [sessions](sessions/README.md) | 持久会话、可靠检查点、工作集恢复、归档归属及可选 SQLite 历史检索 | 扩展层；不依赖 Application/HTTP/Runtime 实现，宿主提供目录与授权 |
| [workspace](workspace/README.md) | 规范化工作目录、受根限制的目录列表和版本化文件预览 | 不依赖项目、会话或 Runtime；用户浏览不调用模型 |
| [projects](projects/README.md) | 可移除的项目登记、目录绑定与修订控制 | 可选扩展；不创建默认项目，不管理执行循环，不删除工作文件 |
| [instructions](instructions/README.md) | 工作空间规则发现、来源注入与派发前刷新检查 | 扩展层；普通接口或薄 Plugin，可信历史来源可注入 |
| [skills](skills/README.md) | 技能发现、校验和模型可见的摘要目录 | 可选；指南和资源通过普通 `read` 渐进加载 |
| [compaction](compaction/README.md) | 生成结构化任务摘要，导出可验证的整组压缩状态 | 可选；通过统一模型网关计费和取消，宿主保存/恢复状态，不覆盖完整档案 |
| [spill](spill/README.md) | 长文本结果与流式输出共用的归档、配额和生命周期 | 可选；宿主为同会话历史引用授权，不替代通用附件系统 |
| [application](application/README.md) | 任务调用管理、可信历史接纳、幂等、订阅、回放和保留策略 | 产品共享调用层；可选 sessions 适配连接会话扩展，见 [产品调用面](../docs/architecture/WEB.zh-CN.md) |
| [http-bridge](http-bridge/) | HTTP/SSE 传输与鉴权适配 | 浏览器或远程接入；见 [桥接与产品装配](../docs/architecture/WEB.zh-CN.md) |
| [tauri-bridge](tauri-bridge/) | Tauri Command、Channel 与 ACK 适配 | 本机桌面接入，可与 HTTP 独立选择；见 [桥接与产品装配](../docs/architecture/WEB.zh-CN.md) |
| [client](client/README.md) | JavaScript 协议客户端及 `RunView` 状态归并 | 不绑定 UI 框架 |
| [memory](memory/README.md) | 精选长期 Markdown、受控批量维护与有界稳定注入 | 可独立装配，不依赖 Sessions 或自进化；见 [上下文与记忆架构](../docs/architecture/CONTEXT-MEMORY.zh-CN.md) |
| [planner](planner/) | 通过已有执行接口进行上层任务编排 | 可选扩展；复用同一执行器，见 [扩展接入](../docs/architecture/EXTENSIONS.zh-CN.md) |

Rust crate 名通常与目录一致；`tauri-bridge` 的 Cargo 包名为 `tauri-plugin-bridge`，工作区别名仍为 `tauri-bridge`。JavaScript 包名为 `client`。

## 默认装配与使用细节

正式 Web 默认装配 Memory 和 Sessions 历史检索，Agent 自主写入工具默认关闭，可独立授权；用户管理入口位于“记忆”设置页。两种记忆独立关闭，不影响核心当前工作历史。

正式 Server 的四个基础工具固定提供，三个检索工具按配置追加；Skills 默认关闭，compaction/spill 默认启用。项目规则 `instructions` 是默认启用、可关闭的独立扩展；上下文来源预算、压缩状态及会话归档之间的边界见[上下文与记忆](../docs/architecture/CONTEXT-MEMORY.zh-CN.md)。包本身的接口与正式宿主的装配策略分开维护，配置来源、覆盖规则和重启生效行为见 [产品层装配](../docs/architecture/WEB.zh-CN.md)。

具体接口、配置、限额和错误语义见各包 README；尚无包内 README 的条目链接到源码目录及相关架构章节。
