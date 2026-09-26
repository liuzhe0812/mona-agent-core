# Eval Harness

独立的本地 Agent 自动测评工作台。用同一组任务、环境和验收规则运行不同 Agent，查看执行证据、资源用量和前后差异。Web 与命令行调用同一个测评引擎。

本目录可以单独复制到另一个仓库运行；不导入 Mona 的 Rust 包、前端资源或私有类型，不要求启动 Mona Web。

## 已实现

- Web：选择 Agent、模型和任务集；设置重复次数、并发、期限及模型调用预算；开始／停止；查看日志、任务结果、历史记录；下载报告与完整单题证据；比较两次运行。
- CLI：同一套运行、查询报告、比较入口。生产运行不需要安装第三方 Python 包，不需要 npm 构建。
- Mona Adapter：通过公开 HTTP 接口启动并管理专属测试宿主，验证文件工具、多轮对话与实际进程重启；不会连接或停止用户正在使用的服务。
- 通用进程 Adapter：任意语言实现一个 JSON Lines 包装器即可接入，不必修改通用引擎。
- 8 个起始任务：精确输出、JSON、文件证据、实际文件产物、当前会话记忆、重启恢复、中文路径和指定文件保护。它们不是完整的行业基准题库。
- 8 类规则评分：精确文本、包含事实、JSON 结构、文件内容、文件 JSON、文件不存在、原文件不变、完整工具终态计数。
- 独立 trial 目录、持久化报告、显式请求去重、取消和期限、子进程树清理。重启将未完成的评测标记为中断，不自动重跑。

**三种结果严格区分：**`selftest` 是测评引擎自检；`controlled` 是受控模型驱动真实 Agent 的集成测试；`live` 才是配置的真实模型评测。前两种通过不等于模型能力已经验收。

## 快速启动

需要 **Python 3.11+（含标准库 sqlite3）**。启动脚本只检查环境，不自动下载软件。Windows 双击 `start.bat`；Linux/macOS 执行：

```sh
sh start.sh
```

默认页面：`http://127.0.0.1:4318`。没有模型密钥也能打开页面、运行免费自检。端口占用时使用 `start.bat --port 4319` 或 `sh start.sh --port 4319`；不会终止占用端口的其他进程。

命令行从本目录运行：

```sh
python -m harness run --adapter selftest --suite all
python -m harness serve --port 4318
python -m harness report e-你的运行编号
python -m harness compare e-基线编号 e-对照编号
```

全局参数 `--data`、`--config` 放在子命令前。默认数据位于 `.local/`；同一个数据目录一次只允许一个工作台／CLI 进程写入。页面打开期间要独立跑 CLI，可使用另一个目录：

```sh
python -m harness --data .local/cli run --suite basic
```

## 接入 Mona

先在被测项目自行构建 `server`。将环境变量 `MONA_EVAL_SERVER` 设为**已构建可执行文件的绝对路径**，然后启动本工作台。本项目不猜测父目录、也不自动编译被测代码。

Windows PowerShell 示例：

```powershell
$env:MONA_EVAL_SERVER = 'D:\path\to\mona-agent-core\target\debug\server.exe'
.\start.bat
```

在页面选择 Mona 和免费受控模型，明确允许本机执行后运行；或：

```sh
python -m harness run --adapter mona --model fixture --suite all --allow-local-execution
```

Mona 适配器使用独立状态和工作目录，固定所选模型，关闭项目目录自动发现、Skills 和项目规则，不继承实际 HOME、模型凭据、代理配置或运行状态。报告记录本次二进制哈希。**这是明确的被测配置，不代表已测遍 Mona 的所有扩展组合。**

## 配置真实模型

复制 `config.example.json` 为本地 `config.local.json`，填写实际模型 ID 与接口协议。协议取值为 `chat_completions`、`responses`、`messages`。`endpoint_env` 与 `key_env` 是环境变量名称，不是实际凭据。

设置 `EVAL_MODEL_ENDPOINT` 为完整接口地址、`EVAL_MODEL_KEY` 为 API Key，再设置 `EVAL_CONFIG` 为配置文件路径并启动。配置路径也可用 `python -m harness --config config.local.json serve` 指定。

真实模型调用必须在页面确认，或同时提供 CLI 的 `--allow-paid` 和被测进程的 `--allow-local-execution`。自建服务即使免费也需要显式授权；代码中不得保存真实密钥。

本地模型网关只转发原请求、观测调用与用量并执行预算准入，不生成答案、不改提示词、不补做工具调用、也不重试。实际供应商密钥只留在网关进程；Agent 使用一次性本地凭据。

## 结果怎样理解

| 结果 | 含义 |
|---|---|
| passed | 所有必需规则通过，且适配器正常结束 |
| failed | 验收不符、Agent 报告失败或预算边界触发 |
| error | 接入、环境、存储或测评执行异常 |
| inconclusive | 证据不足，不推定成功 |
| skipped | 适配器未声明任务要求的能力；不计通过 |
| timeout / cancelled / interrupted | 超时／操作者停止／进程中断，不自动续跑 |

成功率分母是**全部计划 trial**，同时单独列出状态计数。报告区分未知用量，不把未知写成 0。耗时包含冷启动与清理，并提供分段数据；数据目录大小不是累计磁盘 I/O。当前未测量目标进程内存峰值、实际磁盘 I/O 或费用金额，字段明确为 `null`。

调用次数是网关观察值，不是模型自报。Token 用量来自供应商返回：达到上限阻止后续调用；一条在途响应可能超出剩余额度，超出会记录并使本题失败。供应商漏报用量时后续请求被拒绝，不能承诺严格美元成本上限。通用 Adapter 必须使用提供的网关，不经过网关的外部请求无法计量。

比较页会提示任务集、评分器、运行性质或预算不同；小样本快几毫秒不代表统计显著优化。此工程不生成一个可以掩盖越权等硬错误的综合总分。

## 本地执行安全与数据

**仅在可信本机运行经过审核的 Agent 与任务。独立目录、进程树控制、环境变量隔离不是操作系统沙箱。** 当前不提供网络防火墙、用户权限隔离或磁盘配额，不应在有生产凭据的机器上测试恶意 Agent／提示注入攻击；这类测试应在一次性 VM/容器中运行整套工作台。

Web 仅监听 loopback，校验 Host、Origin 和请求令牌；不是多用户互联网服务。目录权限仍由当前系统用户负责。

`.local/` 保存报告数据库、日志、原始工作文件、状态和证据。报告会清除已知连接凭据，但无法识别所有业务敏感内容，任务输入和产物也可能包含私密数据。不要提交此目录，分享报告前复核。删除页面中的已结束记录会同时删除对应测试目录，不影响被测源码或原有会话。

## 二次开发与验证

- [架构与运行边界](docs/ARCHITECTURE.zh-CN.md)
- [任务、评分与 Agent 接入契约](docs/ADAPTERS-TASKS.zh-CN.md)

```sh
python -m unittest discover -s tests -v
```

浏览器验收使用 Node 22+ 的内置 WebSocket 和本机 Chromium/Edge，不需要 npm 安装；设置 `BROWSER_BIN` 和可选 `MONA_EVAL_SERVER` 后执行 `node tests/browser.mjs`。测试生成的报告和截图在 `.local/browser-*/`。

当前没有 Pi／DSH 专用适配器、LLM Judge、Harbor/Inspect 格式导入、完整 Memory/Planner 专项题库或安全沙箱。通用适配契约是接入这些能力的基础，不把未实现内容列作已支持。
