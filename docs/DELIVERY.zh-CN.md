# 交付报告 · v0.3.1

本文记录 Web 组合入口与组件目录迁移后的当前验证范围；未覆盖的生产联调仍需在目标环境完成。

## 本次实际更新

在 v0.3 通用 Core 契约上增加标准 Web UI 的本地开发组合入口：根命令 `npm run dev:web` 同时启动真实 Rust Runtime、HTTP/SSE Bridge 与 Web UI，并自动完成本机连接。

本次没有增加第二套 Agent Runtime，也没有提供假模型的 `dev:web:demo`。模型配置为空时启动器仍可打开设置页，发送任务前必须完成真实供应商配置。

## 当前已执行的检查

- Cargo workspace：161 项 Rust 测试通过（`cargo test --workspace --all-targets --offline`）。
- Rust 文档测试通过；`cargo check -p server --no-default-features` 通过。
- Windows Tauri 原生 feature 编译通过（`cargo check -p tauri-composition --features tauri --locked`）；这不是 GUI 窗口验收。
- Node 24.19.0：37 项 JavaScript Bridge/RunView 测试、4 项启动器测试和 3 项 Web 管理测试通过；TypeScript 5.8.3 客户端声明检查通过。
- 隔离环境（无 `.env`、无既有模型配置）：`npm run dev:web` 启动成功，浏览器自动连接并打开空模型设置页。

这些检查不等于真实付费模型服务、原生 Tauri 窗口交互或生产部署策略的端到端验收。

## 仍需目标环境联调

- Rustfmt、Clippy（若发布流程要求）。
- 真实模型端点、真实工具副作用、权限、持久化和性能基准。
- 生产 TLS、账户认证、租户隔离、密钥系统与正式前端构建。

## 目录与完整性说明

此前打包交付使用的 `MANIFEST.sha256` 与静态检查记录已保留在 `verification/release-v0.3.0/`，作为历史归档，不作为当前目录校验。迁移后的版本完整性由 Git commit、CI 与新的验证记录负责。

`apps/web/index.html`、`apps/web/app.mjs` 与 `apps/web/styles.css` 构成正式通用 UI。`npm run dev:web` 通过环回配置端点提供临时 Bridge Token，不把它写入 URL 或本地存储。

## 仍未提供的能力

没有加入 Session 数据库、崩溃自动续跑、exactly-once、Mona/Python 迁移适配、业务工具、JEV、完整 Provider 集合或生产账户系统。

生产部署不能直接使用本地启动器替代 TLS、账户认证、租户隔离、密钥系统与正式前端构建。

**源码更新不等于生产验收；请以目标环境的 Cargo/CI、真实模型和真实工具测试为准。**
