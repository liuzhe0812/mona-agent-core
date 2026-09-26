# sandbox

Rust 原生的本机文件副作用沙箱，按 DSH 的本机 Sandbox 契约实现。可直接用于 CLI、桌面或服务，不依赖 `api`、Runtime、Sessions、Web、Node.js 或插件框架。它生成受限子进程启动参数，并提供直接文件修改的同策略检查；不执行 Agent 循环。

## 模式与平台

| 模式 | 文件修改范围 |
|---|---|
| `read-only` | 拒绝直接文件修改；子进程仅保留平台运行所需的特殊输出通道 |
| `workspace-write` | 工作区与对应后端临时区域可写 |
| `danger-full-access` | 不使用这一层限制，仍只有宿主原本的系统权限 |

三种模式不限制一般文件读取和联网。库的 `Mode::default()` 为只读；正式 Web 装配默认工作区可写。`Enforcement::Full` 只指所声明的文件副作用约束；`Partial` 表示平台/内核无法完整实现该约束，不是总体安全评分。

| 平台 | 实现与依赖 |
|---|---|
| Linux | 先实际探测 `bwrap`，失败再探测本包 Landlock 启动器；Bubblewrap 只读挂载宿主根，工作区可写时另挂工作区与 `/tmp` 临时层。Landlock 使用 Rust `landlock` 库申请 ABI 5 文件权限，旧 ABI 明确报告部分限制，完全未生效时拒绝 |
| macOS | `/usr/bin/sandbox-exec` + Seatbelt：允许默认操作、拒绝文件写入，按模式允许工作区和临时根；系统不再提供启动器时明确不可用 |
| Windows | Rust 调用受限令牌、DACL、Low integrity 和 `CreateProcessAsUserW`；真实载荷先暂停，加入 Job 后启动。报告 `Partial`；不依赖 Node.js 原生插件或 Docker |

Linux 的进程临时写区域是 `/tmp`；Windows 进程使用按会话/工作区分配的私有临时目录。直接文件检查与 Seatbelt 的临时根按 DSH 共享规则使用系统临时目录，Unix 另外包含 `/tmp`。这些后端差异是合同的一部分，不能用“临时区完全隔离”概括所有平台。

## 宿主接入

| 接口 | 用途 |
|---|---|
| `Policy::new(mode, absolute_workspace)` / `with_session(id)` | 由可信宿主绑定实际根与可选会话身份；工作区必须存在，调用参数不能扩大权限 |
| `Policy::check_write(path)` | 只读拒绝；其余模式返回允许操作的**准确规范路径**。真正修改必须使用返回路径，且紧邻副作用再次检查 |
| `LocalSandbox::new(Runner)` | 创建宿主生命周期内共享的后端选择器和 Windows 临时授权缓存 |
| `backend(cancel)` | 取得后端与完整程度；这是后端选择事实，不保证任意工作区 ACL、命令或后续系统状态都可用 |
| `prepare(argv, policy, cancel)` | 接收完整 argv，返回 `CommandPlan { program, args, info }`；从不解析或重写用户 Shell 脚本 |
| `release_session(id)` | 会话关闭/删除后释放该会话的私有临时授权；有活动命令租约时拒绝，不清理其他会话 |
| `shutdown()` | 停止新准备，清理缓存；调用方先停止并等待自己的命令。清理失败返回错误 |

`Runner::standalone(path)` 指向本包编译出的 `sandbox-run`；`Runner::embedded(path)` 指向嵌入启动入口的宿主可执行文件。嵌入者必须在**同步 main 的最前面、创建线程/Tokio/读取凭据之前**调用 `runner::dispatch()`；返回退出码时立即退出，不能继续启动正常应用。标准 Server 已这样接入，因此 `npm run dev:web` 不需要另行构建辅助程序。

`prepare` 不启动进程。调用方用返回的 program/args 替换原始 argv，保留自己的工作目录、标准流、预算、进程树、取消与等待机制；`CommandPlan` 必须持有到整个命令树停止后，防止提前撤销临时授权。Linux/macOS 无额外守护进程；Windows 启动器只负责受限令牌和原生子进程，原工具继续拥有输出与超时。

Mona 工具接入见 [Tools](../tools/README.md#optional-local-sandbox)。启用 `tools/sandbox` 后，`SandboxBinding` 将宿主固定的 Run 策略同时交给 Shell 和 `write/edit`；读工具不改变，其他自定义工具需要其拥有者显式接入，不能因装配本包就宣称全部进程受控。

## Windows 生命周期与已知行为

Windows 工作区和临时根必须允许宿主设置 DACL 与 Low 标签，即拥有 `WRITE_DAC`、`WRITE_OWNER`；仅有 Modify 不足。数据盘继承的普通 ACL 可能不满足该前提，此时明确拒绝并给出目标目录与所需权限，不自动提权、不取消限制。设置完整 SACL 的 `SeSecurityPrivilege` 不是本包只设置完整性标签的必需前提。原生测试使用用户 `LOCALAPPDATA` 下本次创建、位于临时根之外的隔离目录，不修改项目盘或现有用户目录的 ACL。

工作区 SID 按规范路径稳定派生；工作区 DACL 和 Low 标签为常驻授权，不在每次命令或退出时反复恢复。临时 SID 与目录随机身份绑定：只撤销本次能力，不覆盖其他会话的当前 ACL。没有 Session ID 时使用每次命令临时区；有 Session ID 时最多缓存 1024 个会话/根组合。宿主删除会话时调用 `release_session`，正常关闭时调用 `shutdown`。异常断电/强杀不承诺完成目录清理；工作区文件不会因释放授权而删除。

令牌使用 `DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED`，仅加入 logon、Everyone 与本次允许的能力 SID，并设 Low integrity。不提供自动提权或许可扩大。Windows 只读 PowerShell 可能进入 `ConstrainedLanguage`：常规读取 cmdlet 可用，不承诺任意 .NET API 可用；WMI/CIM 也可能被令牌拒绝。这些是 DSH 同类后端的能力边界，不通过扩大 SID 或取消 Low 标签绕过。

直接文件检查是可信 Rust 中的路径约束，不是内核隔离；按 DSH 的范围保留祖先符号链接在检查与系统调用之间变化的竞态边界。Windows 路径不同也可能引用同一文件对象，因此不承诺硬链接之间的完整隔离。原生同进程插件、宿主记录保存及用户交互 PTY 不在本包自动隔离范围内。

## 错误与验证

`FS_SANDBOX_DENIED` 表示文件策略拒绝；`SANDBOX_UNAVAILABLE` 表示要求限制但后端无法提供；配置、取消和容量错误分别保留机器码。Shell 的有界 stderr 诊断区分启动器失败迹象、文件拒绝迹象和普通命令失败。诊断不是“没有副作用”的证明；不自动重试，更不悄悄改为不受限执行。

`cargo test -p sandbox` 覆盖纯策略、路径、参数、平台配置以及**当前操作系统**的真实受限进程；原生用例缺少后端时失败，不以跳过算通过。`cargo check --target ...` 只能验证对应目标可编译，不能代替该平台的原生执行。正式产品验收入口为 `npm run test:web:sandbox`，使用隔离状态、真实工具和受控模型，不调用付费供应商。

## 来源

行为和平台实现参考 [DSH sandbox](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/sandbox)、[Windows ACL](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/sandbox/sandbox-windows-acl) 与 [fs-sandbox](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/fs/fs-sandbox)。DSH 的 MIT 许可随附于 [LICENSE-DSH](LICENSE-DSH)。本包不承诺与任意未来上游版本自动同步。
