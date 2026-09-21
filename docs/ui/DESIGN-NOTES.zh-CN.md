# Mona Agent Web UI 设计说明

- 参考 Codex 工作台的布局节奏，但不复制品牌、协议或完整功能。
- 稳定的工作项 ID、增量事件与权威完成项来自 Agent Stream v2；UI 不从零猜测最终状态。
- Web 和 Tauri 复用一份界面与 RunView，只替换传输客户端。
- 本地一键启动器是进程组合工具，不是第二套 Runtime，也不进入生产执行链。
- 模型密钥只存在于服务端；本地开发 Bridge Token 由环回启动器临时生成，不放 URL 或 localStorage。
- 不提供 `dev:web:demo`。离线 `preview.html` 只用于视觉参考，不代表 Runtime。
