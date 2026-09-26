# Desktop

`apps/desktop` 是 Mona 的 Tauri 产品壳。它直接加载 `apps/web/` 的正式页面，复用 `apps/server` 的服务装配，在同一进程启动只监听环回地址的 HTTP 产品服务，不创建第二个 Agent 执行循环。桌面数据和模型设置密钥保存在独立的本机状态目录。

仓库固定 Rust 工具链见根目录 `rust-toolchain.toml`；桌面构建使用 `tauri-cli 2.11.2` 与已提交的 `Cargo.lock`。未安装 CLI 时先运行 `cargo install tauri-cli --version 2.11.2 --locked`，并准备目标平台所需的 Tauri 桌面依赖。

桌面项目创建由 Tauri Dialog 弹出文件夹选择器。用户选择后，宿主通过带鉴权的项目管理接口登记目录；取消选择时不创建项目。Web 继续使用默认工作区内按名称创建项目的流程。

Windows 桌面会话顶栏提供“资源管理器 / 终端”系统打开方式。受信主窗口只传会话 ID 和应用选择；桌面壳通过本机带鉴权接口重新读取当前会话的工作目录，确认仍可访问后再启动 Explorer 或独立 `cmd.exe` 窗口。普通 Web 与未提供原生启动能力的平台不显示这组按钮，顶部单独的终端按钮仍使用内嵌 PTY。

Tauri `frontendDist` 指向 `apps/desktop/dist`。正式 Web 页面及其依赖由仓库根目录执行以下命令复制，并检查可静态解析的本地资源引用：

```powershell
node scripts/build-desktop-assets.mjs
```

开发时在仓库根目录运行 `npm run dev:desktop`；它在 `apps/desktop/` 执行 `cargo tauri dev --no-dev-server -- --locked`。Tauri 使用本地资产协议加载上述页面，Rust 宿主在随机环回端口提供同一套任务与产品接口。`MONA_DESKTOP_STATE_DIR` 可为隔离验收指定状态目录。

发布构建在同目录运行 `cargo tauri build -- --locked`；构建前会重新复制正式 Web 静态资源。桌面窗口只给受信 `main` WebView 暴露连接、项目文件夹选择和系统打开命令；系统打开不接受页面传入任意文件路径，项目登记仍由 HTTP 宿主鉴权与校验。退出窗口时先请求服务优雅关闭并等待资源释放。
