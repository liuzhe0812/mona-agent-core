# memory

独立的精选长期 Markdown 记忆扩展，不依赖 Runtime 实现、Sessions、模型协议或 Web。不包含自进化、后台回顾、向量检索和完整聊天副本。跨包关系见[上下文与三类记忆](../../docs/architecture/CONTEXT-MEMORY.zh-CN.md)，装配契约见[扩展接入](../../docs/architecture/EXTENSIONS.zh-CN.md)。

## 普通接口与存储

`Backend::read(cancel)` 返回 `Snapshot`：条目、内容指纹 revision、容量和 Markdown；`apply(Change, Origin, cancel)` 原子执行 1–16 个新增、替换、删除操作。必须使用当前 revision；内容冲突、容量不足或条目不存在时整批拒绝，不覆盖旧版本。完全相同的 Add 不重复保存；原位替换保留条目身份，删除不重用旧 ID。

`FileStore::open(root, Limits)` 使用根目录内唯一正本 `MEMORY.md`；正文是可读的独立 Markdown，受限注释元数据保存 ID、时间及宿主提供的来源。文件锁、临时文件同步与原子替换保护写入，只有成功才返回新快照。缺失文件表示空记忆；损坏、链接和不完整外部编辑明确报错，不重置。文件修改请保留注释元数据及规范 LF 格式，推荐通过管理接口编辑。默认不加密，目录与备份权限由宿主负责。

默认正文每空间 8 KiB、单条 2 KiB、64 条，含元数据文件上限 64 KiB。对批次最终状态校验容量，允许先添加再显式删除旧条目，不静默淘汰。进程内锁等待响应取消，最长 5 秒；取消在提交前检查，原子替换后的取消不等于回滚。文件锁只协调遵守该锁的写入者，不保证任意外部编辑器的事务性；读写结果未知时应重新读取确认，不能无条件重放更新。

`InMemoryStore` 提供同一接口、校验与修改语义，仅用于显式的临时装配与测试，不是磁盘出错后的隐式回退。同步接口在异步宿主使用阻塞线程池，插件已做适配。

## 接入与权限

构造 `Binding::new(name, backend, writable)`，再调用 `MemoryPlugin::new(bindings)`。最多 4 个具名空间；宿主决定后端、身份和读写权限。可分别使用 `context()`、`tools()`，或通过 Plugin 注册；没有多余的 MemoryManager 或服务发现框架。

`memory_read` 返回当前条目和 revision；`memory_update` 使用受限批量操作，是副作用工具。只有 Binding 可写才发布写工具，且仍须宿主最终授权。工具不接受目录、任意用户或空间路径；来源取当前 Run/调用编号，不信任模型自报。记忆不能修改宿主权限或项目规则。

少量精选内容通过 `ContextTransform::sources` 稳定注入：按空间名和条目顺序，不把时间/revision噪声放进前缀。变更在下一次请求构造时刷新，不永久冻结旧内容；不截断事实、不对未匹配查询回退最近几条、不改写正式 Transcript。来源成本交给既有 Runtime 在压缩前预留。是否相信/维护一条事实仍需用户或获授权的策略判断，格式校验不是事实准确性或完整秘密检测。

## 产品默认

正式 Web 默认装配长期记忆，个人与当前实际工作目录为两个空间；工作区记忆按规范目录身份共享，普通独立会话目录不自动互通。`memory_update` 默认关闭，可在 Agent 工具页授权后重启生效；已授权后不逐次弹窗。用户可在独立记忆页直接管理、纠错和删除。未安装 Memory 不影响基础当前 Run 的历史与 Sessions 保存恢复。

## 验证

`cargo test -p memory` 验证 Markdown 重开、原子批量修改、冲突、容量、取消、损坏、作用域和上下文稳定性；`cargo test -p runtime --test plugins` 验证最终写入权限及投影不改历史。真实宿主路径见[浏览器测试](../../apps/web/test/README.md)。
