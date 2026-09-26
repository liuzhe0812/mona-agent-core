# Agent 接入、任务与验收契约

## 接入其他 Agent

通用 `command` Adapter 每个 turn 启动一次包装器，传递独立 `state_dir`，跨 turn 需要的会话身份由包装器保存。Agent 可以用 Rust、Python、JavaScript 等语言实现，测评引擎不依赖目标内部类型。

本地配置只接受 literal argv，不执行 shell 拼接。示例：

```json
{
  "version": 1,
  "agents": [{
    "id": "my-agent",
    "label": "我的 Agent",
    "kind": "command",
    "argv": ["/absolute/path/to/adapter", "--eval-jsonl"],
    "capabilities": ["text", "files", "multiturn", "restart", "tool_events"],
    "revision": "明确的版本或构建标识",
    "env_keys": []
  }],
  "models": []
}
```

Python 可使用 argv 中的 `{python}` 替换为当前解释器，随后指定包装器的绝对路径。命令不会自动从父仓库寻找。`env_keys` 是需显式继承的额外环境变量名称，内容不返回浏览器；不要借此继承真实 HOME 或持久会话目录。

### 输入与生命周期

stdin 只有一行 UTF-8 JSON，随后关闭：

```json
{"protocol":1,"prompt":"本轮任务","workspace":"/trial/workspace","state_dir":"/trial/state","turn":0,"deadline_seconds":120}
```

评分器、参考答案、task manifest 和原始上游密钥不出现在输入中。实际模型连接通过环境变量提供：

| 变量 | 含义 |
|---|---|
| `EVAL_MODEL_ENDPOINT` | 本次 trial 专有的完整本地接口地址 |
| `EVAL_MODEL_KEY` | 本次临时鉴权令牌，不是实际供应商密钥 |
| `EVAL_MODEL_NAME` | 本次选定模型 ID |
| `EVAL_MODEL_PROTOCOL` | `chat_completions` / `responses` / `messages` |

stdout 仅允许 NDJSON。日志写 stderr。支持事件：

```json
{"type":"text","text":"可展示的输出片段"}
{"type":"tool","id":"call-1","name":"read","status":"success"}
{"type":"result","protocol":1,"status":"completed","output":"完整最终回答","tool_events_complete":true}
```

- `tool` 是执行后的终态，不是计划调用或模型提出的动作。单题单轮同一调用编号只能出现一次。
- `result` 必须恰好一个、是最后一条，子进程还需正常退出。`completed` 不是自动通过，仍要独立评分。
- 未完整观察所有工具时必须 `tool_events_complete=false`；涉及完整工具计数的检查返回 inconclusive。
- 每条 stdout 行最多 64 KiB，每个输出流最多 2 MiB，总事件有界；过量输出会终止，不无限堆积。
- 暂停或进程被终止时，不应自动再次执行工具。新进程根据提供的 state_dir 恢复已经确认的状态。
- Capability 是 Adapter 的明确声明，不是平台猜测；缺少要求的 capability，该任务显示 skipped，不计算为通过。

Pi、DSH 等项目可用包装器映射此契约；仓库当前没有宣称已经提供其专用 Adapter。

## 编写任务

任务是 `tasks/*.json`，当前只接受格式 1。示例：

```json
{
  "version": 1,
  "id": "config-update",
  "title": "修改指定配置并保护无关文件",
  "suite": "configuration",
  "requires": ["files"],
  "fixtures": {"keep.txt": "必须保留"},
  "turns": [{"prompt": "在 {workspace}/settings.json 写入 JSON，enabled 为 true。不要修改其他文件。"}],
  "checks": [
    {"type":"file_json","path":"settings.json","value":{"enabled":true}},
    {"type":"unchanged","path":"keep.txt"}
  ]
}
```

每个任务最多 20 个 turn、32 个 fixture、32 个检查；fixture 合计最多 1 MiB。路径为工作目录相对路径，拒绝 `..`、绝对路径、链接／junction。`{workspace}` 只在 prompt 内替换为当前独立目录。

第二个 turn 可设置 `restart_before:true`；这不是复制一个新空任务，而是重启本题的宿主并沿用本题状态。无此能力的 Adapter 跳过本题。

## 规则评分

| type | 判定 | 主要字段 |
|---|---|---|
| exact | 去除首尾空白后的文本严格一致 | value，turn 可选 |
| contains | 完整输出中包含给定字符串 | value，turn 可选 |
| json | JSON 结构、值与类型一致，忽略对象键顺序 | value，turn 可选 |
| file_exact | UTF-8 文件完整文本一致 | path、value |
| file_json | 文件 JSON 结构与类型一致 | path、value |
| absent | 指定路径不存在 | path |
| unchanged | 指定 fixture 内容未修改 | path |
| tool_count | 完整终态证据中指定工具成功次数符合要求 | name、count、turn 可选 |

未指定 turn 时检查最后一轮。文件检查发生在整个任务执行结束后，而不是历史文件快照。单个文件评分读取上限 1 MiB。所有检查默认 required；可显式设为 false 作为辅助观察，但至少需要一个必需检查才能通过。

`absent` 仅验证指定路径，不能证明没有其他副作用。`tool_count` 的可信度受 Adapter 观测范围限制；Mona 当前使用快照观测，展示裁剪后不伪造完整工具证据。不要使用这些规则代替操作系统沙箱。

当前提供规则评分，不包含 LLM Judge。开放式报告的专业性、完整性、规划合理性，不能用几条 contains 就宣称已获全面质量验证；应为真实业务设计相应验收和校准过的评分器。

## 运行报告

每次报告 format version 1，包含：

- `config`：实际选择与每 trial 预算；`mode` 区分 selftest/controlled/live。
- `manifest`：本次固定的任务和评分规则；`taskset_hash` 与 `task_hash` 定位题目版本。
- `provenance`：测评代码、Adapter、实际二进制、模型、协议和环境；未知字段为 null。
- `trials`：每次重复的状态、分项检查、预览、指标、证据哈希。
- `summary`：状态计数、总任务分母、耗时统计、调用／用量覆盖。

完整单题 `evidence.json` 与网页预览分开；网页预览可截短，评分使用完整 Adapter 输出。证据被外部修改后，下载会拒绝哈希不符的文件。

只在任务与评分器版本一致、运行性质一致、预算与环境可比时分析差异。对比的重复次数较少时，不自动将时延或成功率差异称为改进。
