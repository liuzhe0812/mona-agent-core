# memory

可选、显式写入的有界记忆示例。`MemoryBackend` 提供 recall/remember，`InMemoryStore` 默认保留 64 项，按键匹配召回；不是语义检索数据库，也不承诺跨进程保存。

宿主可创建 `MemoryPlugin::new(backend)`，通过既有 HostBuilder.plugin 装配。插件仅发布后端服务、记忆来源和显式写工具；`memory_remember` 是副作用工具，仍需宿主最终授权，插件不能自行放宽权限。

API 7 的召回结果使用 `ContextTransform::sources` 的 `memory.recall` 来源，每次最多召回 4 项，参考文本最多 12 KiB。来源不改写正式 Transcript，Runtime 在压缩前预留其开销，不将召回内容误认作当前用户请求。真正的长期持久化、检索算法与隐私策略由后端/宿主实现。

验证：`cargo test -p runtime --test plugins` 覆盖记忆投影不改历史、写入默认拒绝。公共上下文变化见 [上下文契约](../../docs/CONTEXT-MANAGEMENT.zh-CN.md)。
