# workspace

可复用的根目录限定文件浏览，不依赖项目登记、Web、Session 或 Runtime。宿主提供绝对目录和私有状态排除目录，库不读取环境变量、不调用模型。

`canonical_root(path)` 校验和规范化目录；`Directory::open(root).excluding(private_roots)` 构造只读访问能力；`list(relative, offset, limit, revision)` 返回目录页；`read(relative, offset, limit, revision)` 返回文本页、内联栅格图或明确的 binary 描述。

`stat(relative)` 复用同一根目录、链接和私有目录检查，只返回名称、相对路径、类型和字节数，不读取或哈希文件正文。它可供文件引用卡片检查存在性，也可报告超过预览限制的大文件。元信息不是读取授权凭证或内容快照；后续打开/分页仍需重新校验路径和内容版本。

`contains_path(root, candidate)` 按路径组件判断包含关系，避免相似前缀匹配；Windows 私有目录检查同时处理大小写拼写。它不替代目录规范化、链接检查或宿主授权。

目录每页 1–200 项，扫描最多 10000 项；文本单页 4–65536 字节、单文件最多 16 MiB，UTF-8 边界安全；图片最多 4 MiB。每页带内容版本，后续分页传同版本，变化返回 conflict。拒绝绝对路径、父路径、驱动器/ADS、符号链接和 reparse point；宿主鉴权仍必需。这不是 OS 沙箱，不保证抵御具备本机写权限且在检查间恶意替换祖先目录的进程。

测试：`cargo test -p workspace`。它与可选 `projects` 包分开，因此无项目基础 Agent 仍可使用文件浏览。
