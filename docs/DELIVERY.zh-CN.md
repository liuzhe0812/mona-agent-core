# 交付报告 · v0.3.1

## 本次实际更新

在 v0.3 通用 Core 契约上增加标准 Web UI 的本地开发组合入口：根命令 `npm run dev:web` 同时启动真实 Rust Runtime、HTTP/SSE Bridge 与 Web UI，并自动完成本机连接。

本次没有增加第二套 Agent Runtime，也没有提供假模型的 `dev:web:demo`。模型配置缺失时启动器直接失败。

## 本次实际执行的检查

- Node 22.16.0：37 项 JavaScript Bridge/RunView 测试通过。
- Node 22.16.0：3 项 Web 启动器测试通过，覆盖 dotenv、配置限制、模块化静态资源及内存配置端点。
- 使用受控的本机假进程做了启动器进程编排冒烟：UI 与 Runtime 就绪、资源服务、SIGINT 统一收尾。
- Python 静态预检：TOML/JSON、目录/模块、生产依赖图、Bridge 可选性、Markdown 链接和测试清单通过。

这些检查不等于真实 Rust 模型服务的端到端验收。

## 明确未执行

当前执行环境没有 Cargo/Rustc，因此没有执行：

- Rust 构建与 143 项 Rust 测试；
- Rustfmt、Clippy；
- 原生 Tauri 构建；
- 真实模型端点联调；
- 真实工具副作用、权限、持久化和性能基准。

GitHub Actions 已加入 Web 启动器测试，提交后的 CI 状态应作为下一步 Rust 验证依据。

## 目录与完整性说明

此前打包交付使用的 `MANIFEST.sha256` 是一次性快照，不适合持续演进的 Git 仓库；本次从仓库中移除。版本完整性由 Git commit、CI 与发布制品的独立校验负责。

`ui/index.html`、`ui/app.mjs` 与 `ui/styles.css` 构成可直接修改的标准参考 UI。`npm run dev:web` 通过环回配置端点提供临时 Bridge Token，不把它写入 URL 或本地存储。

## 仍未提供的能力

没有加入 Session 数据库、崩溃自动续跑、exactly-once、Mona/Python 迁移适配、业务工具、JEV、完整 Provider 集合或生产账户系统。

生产部署不能直接使用本地启动器替代 TLS、账户认证、租户隔离、密钥系统与正式前端构建。

**源码更新不等于生产验收；请以目标环境的 Cargo/CI、真实模型和真实工具测试为准。**
