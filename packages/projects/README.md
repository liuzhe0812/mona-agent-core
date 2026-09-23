# projects

可选项目登记扩展，依赖通用 `workspace` 路径能力，不依赖 sessions、runtime、application 或 Web。基础 Agent 不装配此包也能使用显式/默认目录、文件浏览和会话历史。

`Registry::open(path)` 打开宿主提供的状态文件并持有单写者锁；`list/get` 查询登记；`add(request_id, revision, name, directory)` 校验真实目录、规范路径去重、幂等创建；`remove(id, revision)` 只删除登记。版本守卫和原子保存成功后才发布新列表。

项目 ID 与目录分开；项目允许非 Git 文件夹。不存在的目录不自动创建或回退。项目移除不操作会话或工作文件，历史实际 cwd 独立保存在 sessions。删除登记后即使再次使用相同请求键，也生成不同项目身份，不将旧会话归入另一个目录的新项目；未删除的同请求重试保持幂等。

最多 256 个项目、状态 1 MiB。测试：`cargo test -p projects`。该包不是执行权限来源，用户目录授权由宿主承担。
