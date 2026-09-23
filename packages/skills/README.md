# Skills

独立的 Agent Skills 发现与目录组件。Registry 和 Provider 负责发现、校验、重名优先级及可信宿主 API；`SkillsPlugin` 发布服务，并通过上下文投影向模型提供名称、简介和完整指南位置。组件不实现第二套 Agent 循环，也不注册专用 `skill` 工具。

正式 Web 发行版包含 Skills，但新环境默认关闭。用户在“设置 → Agent 能力”启用并重启后，宿主扫描项目和用户的 `.agents/skills`：

```text
.agents/skills/
  summarize/
    SKILL.md
    references/checklist.md
```

`SKILL.md` 使用 YAML frontmatter：

```markdown
---
name: summarize
description: 总结材料，提取主要结论、依据与待确认事项。
---
先阅读 references/checklist.md，再按照其中的清单总结。
```

每次模型投影只加入允许自动调用的 Skill 摘要和绝对 `SKILL.md` 位置。任务匹配时，Agent 使用正式宿主固定提供的 `read` 工具读取完整文件；相对资源继续通过 `read` 获取，需要执行的脚本通过已经授权的 `shell` 运行，命令语法以工具声明的实际解释器为准。启用 Skills 不增加模型工具数量。

工具整体关闭，或当前 Run 的工具上限排除了 `read` 时，不发布目录。目录只存在于模型投影，不写入正式 Transcript，也不会随轮次不断追加。Skill 内容不扩大文件、命令或其他工具权限。

API 7 使用 `ContextTransform::sources` 返回 `skills.catalog` 来源，普通 `transform` 保持消息不变。Runtime 在压缩前预留目录预算，随后统一插入；目录不再成为“最后一个用户请求”，也不因为排在压缩器之后才造成未计量的追加。单独使用变换器的嵌入者应同时调用来源接口并遵循[来源/压缩契约](../../docs/CONTEXT-MANAGEMENT.zh-CN.md)。

本地 Provider 支持一层目录 bundle 和根目录平铺 `.md`；不递归发现嵌套技能。目录和正文每次重新读取，以便下一轮看到文件更新。名称重合时 Provider 顺序靠前者胜出，包括禁止模型自动调用的条目。

默认限制：64 项技能、每根 1024 个目录项、单文件 64 KiB、正文或可信 API 资源 32 KiB、目录 32 KiB。名称必须是 1–64 字节的小写 kebab-case，`description` 必填。`disable-model-invocation` 控制是否进入模型目录，`user-invocable` 保留给可信应用入口。

默认宿主依次读取最近 Git 项目根下的 `.agents/skills` 和用户目录下的 `.agents/skills`。`AGENT_SKILL_DIRS` 可由部署者完全替换根集合，`AGENT_SKILLS=0` 可锁定关闭。普通任务和模型不能改变这些根。

Registry 仍保留 `list/load/read_resource` 的可信 Rust API，便于宿主或未来非文件 Provider 使用；模型消费统一走普通工具。关闭 Skills 时不扫描目录或发布上下文。

验证：

```sh
cargo test -p skills
```
