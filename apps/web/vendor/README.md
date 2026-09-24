# Self-hosted preview assets

这些是产品按需加载的第三方静态依赖，不是额外的主题/Runtime 或来自 CDN 的动态代码。主 Web 界面保持原生 ES Module。第三方文档页原始颜色不受 Mona chrome Token 强制覆盖。

## Terminal

- `@xterm/xterm` 5.5.0，MIT，`xterm/xterm-LICENSE`。
- `@xterm/addon-fit` 0.10.0，MIT，`xterm/addon-fit-LICENSE`。

从 npm 对应固定版本 tarball 复制 `lib/xterm.js`、`css/xterm.css`、`lib/addon-fit.js`；获取时校验 npm 发布的 SHA-512 integrity。

xterm integrity: `sha512-hqJHYaQb5OptNunnyAnkHyM8aCjZ1MEIDTQu1iIbbTD/xops91NB5yq1ZK/dC2JDbVWtF23zUtl9JE2NqwT87A==`。

fit integrity: `sha512-UFYkDm4HUahf2lnEyHvio51TNGiLK66mqP2JoATy7hRZeXaGMRDr00JiSF7m63vR5WKATF605yEggJKsw0JpMQ==`。

## Office preview

`docx-preview@0.4.0`、`@extend-ai/react-xlsx@0.16.0`、`@aiden0z/pptx-renderer@1.2.4`，JSZip 与 XLSX 预览的 React 依赖编入专属 iframe 组件；主页面不加载 React。确切依赖见 `office/build-package.json` 与 `office/build-package-lock.json`。许可证见 `office/THIRD-PARTY-NOTICES.txt` 及打包 JS 尾部 notices。

重建：将上述 build-package 文件复制为 `.tmp-verify/preview-build/package.json` 和 `package-lock.json`，在该目录运行 `npm ci --ignore-scripts --no-audit --no-fund`，再在仓库根运行 `node scripts/build-preview-assets.mjs`。构建器只写 Office 自托管静态目录，不改主 npm 项目。

当前静态原始资源总量约 8.7 MiB（包括 WASM、终端、许可和锁文件），仅打开对应类型时加载；不会在普通聊天首屏同时下载全部预览引擎。文档输入不允许改变脚本地址，固定白名单只选择 docx/xlsx/pptx。
