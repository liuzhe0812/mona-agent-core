# instructions

扩展层的工作空间规则：发现适用 `AGENTS.md` / `CLAUDE.md`、生成上下文来源，并在工具派发前检查规则是否变化。生产依赖只有公共 `api` 和基础库，不依赖 sessions、Runtime、Application 或 Web。

## 接入

```rust,no_run
# use std::{path::Path, sync::Arc};
# async fn build(model: Arc<dyn api::Model>, workspace: &Path) -> api::Result<()> {
let rules = instructions::ProjectInstructions::new(workspace)?;
let mut host = runtime::HostBuilder::new()
    .model(model)
    .plugin(Arc::new(rules.plugin()))
    .build().await?;
// 按场景装配 Tool；插件不会自动注册工具或授予副作用权限。
host.shutdown().await?;
# Ok(()) }
```

普通接口同样可用：同一 `ProjectInstructions` 实现 `ContextTransform` 和 `ToolPolicy`。Plugin 只是同时注册两个接口的薄入口，共享同一份 Run 状态，不另建规则服务或执行循环。

会话摘要可能省略旧文件操作。此时用 `with_history(Arc<dyn HistorySource>)` 注入可信的完整历史读取函数，恢复已触达目录；该回调在有界阻塞工作中调用，也可直接使用闭包。会话身份、存储类型及访问授权由宿主负责，不把某个产品的会话键或数据库写进本包。无需历史来源时保持空装配。

## 行为

从指定工作空间根开始，每目录优先 AGENTS.md，不存在时用 CLAUDE.md。结构化 `read/write/edit/grep/find/ls` 路径触发相关目录及祖先规则，不递归扫描整个仓库；工作空间外的路径不触发规则扫描，也不会因无访问权限阻断后续轮次。每轮重新读取适用文件，来源不会混入正式用户历史。副作用工具发现新规则或规则已改变时，拒绝该次未执行调用，让模型获得新规则后重新发起；同批工具之间也检查，拒绝不表示撤销旧动作。

单文件及总原文上限 64 KiB，每 Run 最多 64 个相关目录、128 个祖先作用域。文件非普通文件、链接/reparse point、无权限、无效 UTF-8 或过大时明确失败；取消和 finish 清理后不重建缓存。规则不授予权限，不是任意 Shell 脚本分析器、操作系统沙箱或跨 Run 文件事务。

当前 Web 默认装配此扩展，部署开关仍由应用层管理。其他宿主按场景装配自己的规则内容；该包不将通用 Agent 固定为编程助手。

```sh
cargo test -p instructions
```
