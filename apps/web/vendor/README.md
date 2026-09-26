# Self-hosted preview assets

这些是产品按需加载的第三方静态依赖，不是额外的主题/Runtime 或来自 CDN 的动态代码。主 Web 界面保持原生 ES Module。第三方文档页原始颜色不受 Mona chrome Token 强制覆盖。

## Conversation content

正文依赖的完整固定版本与 integrity 在 `scripts/content-assets/package.json` / `package-lock.json` 集中维护：markdown-it 与脚注插件负责 Token，KaTeX 负责公式，Highlight.js 负责代码，Mermaid 负责图表，DOMPurify 清理受控 HTML/SVG 输出。原来的手写 Markdown、数学和图表语法分支不再保留。

重建时在仓库根执行 `npm ci --prefix scripts/content-assets --ignore-scripts --no-audit --no-fund`，随后执行 `node scripts/build-content-assets.mjs`。输出到 `content/` 并保留依赖许可证，日常启动不运行 npm install。正文基础组件自托管；高亮和隔离图表按需加载，KaTeX 字体只用于公式展示。不能将第三方引擎 CSS 当作任意业务组件绕过 Mona Token 的入口。

## Terminal

- `@xterm/xterm` 5.5.0，MIT，`xterm/xterm-LICENSE`。
- `@xterm/addon-fit` 0.10.0，MIT，`xterm/addon-fit-LICENSE`。

从 npm 对应固定版本 tarball 复制 `lib/xterm.js`、`css/xterm.css`、`lib/addon-fit.js`；获取时校验 npm 发布的 SHA-512 integrity。

xterm integrity: `sha512-hqJHYaQb5OptNunnyAnkHyM8aCjZ1MEIDTQu1iIbbTD/xops91NB5yq1ZK/dC2JDbVWtF23zUtl9JE2NqwT87A==`。

fit integrity: `sha512-UFYkDm4HUahf2lnEyHvio51TNGiLK66mqP2JoATy7hRZeXaGMRDr00JiSF7m63vR5WKATF605yEggJKsw0JpMQ==`。

## Office preview

`docx-preview@0.4.0`、`@extend-ai/react-xlsx@0.16.0`、`@aiden0z/pptx-renderer@1.2.4`，JSZip 与 XLSX 预览的 React 依赖编入专属 iframe 组件；主页面不加载 React。确切依赖见 `office/build-package.json` 与 `office/build-package-lock.json`。许可证见 `office/THIRD-PARTY-NOTICES.txt` 及打包 JS 尾部 notices。

重建：将上述 build-package 文件复制为 `.tmp-verify/preview-build/package.json` 和 `package-lock.json`，在该目录运行 `npm ci --ignore-scripts --no-audit --no-fund`，再在仓库根运行 `node scripts/build-preview-assets.mjs`。构建器只写 Office 自托管静态目录，不改主 npm 项目。

Office 和终端资源仅在打开对应类型时加载，不会在普通聊天首屏同时下载全部预览引擎。文档输入不允许改变脚本地址，固定白名单只选择 docx/xlsx/pptx。
