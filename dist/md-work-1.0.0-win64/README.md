# 墨笺 · md-work

Windows 优先的纯 Rust Markdown 笔记本。界面由 GPUI + gpui-component 驱动，渲染与导出基于 comrak 与 Typst，
MCP 服务基于 rmcp。笔记以普通文件夹保存，不锁定数据格式。

参考 VNote 的使用习惯，但针对其常见顽疾做了修正：资源管理器双击 `.md` 崩溃、PDF 导出质量差、
缺少书签与整本打包、MCP 能力不完整。

## 功能

- **笔记本**：普通文件夹即笔记本，支持多笔记本、树形目录、新建文件夹/笔记、重命名、移动、回收站。
- **编辑器**：Markdown 语法高亮、软换行、查找/替换（`Ctrl+F` / `Ctrl+H`）、预览（`Ctrl+T`）、大纲、字数/词数统计、选区统计。
- **预览代码块**：按语言做语法高亮，可逐块复制；长行可切换换行/横向滚动。
- **外部文件**：可直接打开工作区外的 `.md`，在标签栏一键“存入笔记本”（重名自动加序号，原文件不变）。
- **搜索**：跨笔记全文搜索，可选区分大小写、正则、按文件名；结果跳转到对应行。
- **书签**：为任意笔记或标题添加书签并快速跳转。
- **命令面板**：`Ctrl+Shift+P` 模糊检索全部命令。
- **面板**：笔记/大纲/搜索/书签/回收站面板可停靠左侧或右侧、横向或纵向排列；拖动分隔条调整宽度，双击恢复默认，宽度随配置保存。
- **导出**：Typst 渲染 PDF/PNG，支持页边距、字号、行高、目录深度、图片宽度、页码格式、两端对齐。
- **整本打包**：导出/导入笔记本 `.zip`（含导入预览、冲突策略），导入时校验路径逃逸、绝对路径、符号链接与体积上限。
- **编码与换行**：自动识别 UTF-8/GBK 等编码与 CRLF/LF，按原编码保存，可另存为指定编码。
- **崩溃恢复**：编辑中途异常退出时，下次启动可从快照恢复未保存内容；保存后自动清理快照。
- **图片**：本地 `assets/` 图床（可配置通用 HTTP / PicGo 兼容接口），图片写入限制在笔记本 `assets/` 内。
- **MCP**：stdio 与 Streamable HTTP（仅 `127.0.0.1`），默认拒绝（无令牌返回 401），DPAPI 加密令牌，
  按笔记本/笔记的 ACL、glob 允许/拒绝，审计 JSONL 不记录笔记正文。
- **单实例**：二次启动会把文件路径转发给已运行实例；不静默修改 Windows `.md` 默认应用。

## 目录

| 内容 | 位置 |
| --- | --- |
| 配置、日志、MCP 令牌、恢复快照、崩溃记录 | `%LOCALAPPDATA%\md-work\` |
| 默认工作区 | `%USERPROFILE%\Documents\md-work` |
| 安装目录（按用户） | `%LOCALAPPDATA%\Programs\md-work` |

安装包只写 HKCU 与当前用户目录，不申请管理员权限，不修改 HKLM，也不会静默更改 `.md` 文件关联。

## 使用

### 快捷键

| 快捷键 | 功能 |
| --- | --- |
| `Ctrl+S` | 保存当前笔记 |
| `Ctrl+N` | 新建笔记 |
| `Ctrl+F` / `Ctrl+H` | 在编辑器内查找 / 替换 |
| `Ctrl+T` | 切换编辑/预览 |
| `Ctrl+Shift+P` | 命令面板 |
| `Ctrl+B` / `Ctrl+I` | 加粗 / 斜体（有选区则包裹选区，否则插入标记并将光标置于中间） |
| `Ctrl+K` | 插入 Markdown 链接 |

### MCP

1. 打开 设置 → MCP，生成令牌并启用 HTTP 服务（默认端口 `11026`）。
2. 客户端连接 `http://127.0.0.1:11026/mcp`，使用 `Authorization: Bearer <令牌>`。
3. 无令牌或令牌错误返回 401；超出 ACL 的笔记本/笔记返回 403；GUI 中有未保存修改的笔记默认拒写。

也可以用 stdio：

```powershell
md-work-cli mcp serve --stdio
```

### CLI

```powershell
# 列出工作区内的笔记本
md-work-cli notebook list --json

# 搜索（支持正则、区分大小写）
md-work-cli search "渲染" --regex --limit 50

# 导出整本笔记本
md-work-cli notebook export "我的笔记本" -o notes.zip

# 导入并预演冲突
md-work-cli notebook import notes.zip --mode merge --policy keep-both --dry-run
```

`md-work-cli export --request <json>` 供 GUI 与外部工具调用 Typst 导出。

## 构建

要求：Windows 10/11 x64、Rust 1.97+（见 `rust-toolchain.toml`）、Visual Studio 2022 C++ 工具链。

```powershell
$env:CARGO_HOME = "E:/dependencies/cargo_caches"   # 本机缓存位置，可按需省略
cargo build --workspace
cargo test --workspace
```

### 图标与打包

```powershell
cargo run -p xtask -- icon        # 由 assets/icon.svg 生成 assets/icon.ico
cargo run -p xtask -- package     # release 构建 → dist/md-work-1.0.0-win64/ 与 .zip
cargo run -p xtask -- installer   # 调用 makensis 生成 dist/md-work-1.0.0-setup.exe
```

`installer` 需要安装 [NSIS 3](https://nsis.sourceforge.io/)（`makensis` 在 PATH 或默认安装目录）。
安装脚本位于 `packaging/nsis/md-work.nsi`，按用户安装、HKCU 卸载信息、卸载时保留笔记与设置。

## 已知限制（v1 范围）

- 不含 Mermaid / PlantUML / Graphviz 图表渲染。
- 不含 AI 助手（仅保留 `AiProvider` 扩展点）。
- 不含标签、云同步、多窗口。
- 不包含遥测与自动更新。

## 许可

MIT。第三方组件见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。
