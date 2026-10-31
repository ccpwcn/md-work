# 墨笺 md-work 实施计划

## Context

从零实现一个 Windows 优先、纯 Rust 的 Markdown 笔记本应用，目标是用“VNote 的轻量 + 原生 Markdown 编辑”解决现有工具的具体痛点：VNote 从资源管理器打开 `.md` 会崩溃、PDF 导出弱、无书签/整本 zip、MCP 不完整；SiYuan 资源占用高、导出目录跨笔记本串用、预览复制文本多空行、行距过大、原生 Markdown 编辑体验弱。首版不做片段/任务/提醒，AI 写笔记能力只预留接口。

已确认的技术事实与决策：

- 目标系统 Windows 10 1903+（当前 19045）已满足 Zed/GPUI 的 DirectX 11 要求。
- 采用 GPUI + gpui-component（Apache-2.0）。gpui-component 0.8 提供代码编辑器、Markdown/HTML TextView、Dock、主题与 i18n，能显著减少自研控件；GPUI 仍为 pre-1.0，因此先做 M0 技术验证并保留回退 Iced 的开关。
- “双击 Shift 打开命令面板”是 Zed 默认键位，不是 GPUI 固有问题；本项目不注册双击 Shift，命令面板使用 `Ctrl+Shift+P`。
- 中文名定为“墨笺”，英文名 md-work；磁盘/安装目录使用 ASCII `md-work`，界面与安装包显示“墨笺”。
- PDF/PNG 导出使用纯 Rust 的 Typst 管线，不使用 wkhtmltopdf、WebView 或外部浏览器。
- 公式支持（mitex + Typst）；Mermaid/PlantUML/Graphviz 首版不做。
- 不兼容 VNote 的 `vx.json` 磁盘格式；使用新的、可自描述、可直接用资源管理器管理的笔记本格式。

## 0. 版本范围

### v1.0 包含

- 工作区/多笔记本：默认在“我的文档”创建 `md-work` 工作区；多笔记本同时打开、切换；记住上次打开的笔记本、每个笔记本的标签页与光标；外部复制进工作区的笔记本自动识别，失败给出原因。
- 笔记本树、笔记、文件夹、新建/重命名/移动/删除/回收站、外部 `.md` 文件标签页。
- 编辑/预览一键切换（`Ctrl+T`）；CommonMark + GFM（表格、任务列表、删除线、脚注）；代码高亮；预览与导出分别可控的代码块换行；无多余空行的预览复制；拖拽/粘贴图片。
- 书签（笔记 + 标题）、大纲、搜索（文件名/内容/正则、当前或全部笔记本、可取消）、状态栏行数/字数/字符数、左侧树自动定位当前笔记。
- 编码：UTF-8（含 BOM）默认，GB18030/GBK 与 Windows-1252（ANSI）读写；可另存为指定编码；记住每个文件的编码。
- 图床：默认笔记本内 `assets/`；可配置通用 HTTP 图床（含 PicGo HTTP 预设）。
- 导出：单篇 PDF/PNG；PDF 支持封面、目录、页码、页眉页脚、纸张/边距、字体、代码换行、语法高亮、图片、公式；每个笔记本分别记忆上次导出/导入目录。
- 整本笔记本 zip 导出/导入；同名冲突提供合并/覆盖/重命名。
- MCP：独立 CLI 提供 stdio 与 Streamable HTTP（默认 `127.0.0.1:11026/mcp`）；按笔记本、按笔记授权；令牌、只读/读写/搜索/删除等权限、审计日志。
- 设置：简体中文/英文运行时切换；界面字体与笔记字体；亮色/暗色/跟随系统主题；`Ctrl+Shift+P` 命令面板。
- Windows：单实例、资源管理器右键“用墨笺打开 .md”、用户级安装包、图标、开始菜单快捷方式。
- 预留 `AiProvider` 接口，首版不实现任何供应商。

### v1.0 不包含

- 片段、任务、提醒、标签、双链、同步、多窗口、自动更新、代码签名。
- Mermaid/PlantUML/Graphviz、AI 供应商接入、VNote 旧格式导入、静默抢占 `.md` 默认关联（Windows UserChoice 有哈希保护，无法静默设置）。

## 1. 技术基线与依赖

| 项 | 选择 | 说明 |
|---|---|---|
| UI | `gpui-component = 0.8.0` + 其精确匹配的 `gpui-pre` | M0 用 `cargo tree -i gpui-pre` 锁定版本；**不要**同时依赖 crates.io 的 `gpui 0.2.2`，避免两份 GPUI 类型不兼容 |
| 编辑/预览 | gpui-component Editor + TextView + 自研块渲染 | 先验证软换行、中文 IME、选择/复制；不足则 fork 到 `crates/mdwork-editor`（Apache-2.0） |
| Markdown | `comrak`（开启 sourcepos） | 预览、导出、大纲共用同一 AST |
| 高亮 | `syntect` + `two-face` | 预览与 Typst 代码着色共用 token |
| PDF/PNG | `typst` 0.15、`typst-pdf`、`typst-render`、`typst-svg`、`typst-as-lib`、`typst-kit-fonts` | M0 验证与 `mitex` 的版本组合；必要时直接用 Typst 原生 crates |
| 公式 | `mitex` | LaTeX → Typst；预览转 SVG 缓存，导出直接嵌入 Typst |
| 搜索 | `grep-searcher`、`grep-regex`、`ignore` | 查询时并行扫描，不维护索引，保持轻量；支持取消 |
| MCP | `rmcp 3.7`（`transport-io`、`transport-streamable-http-server`） | stdio + Streamable HTTP |
| 文件监控 | `notify` + `notify-debouncer-full` | 外部修改、复制笔记本入工作区、保存回环抑制 |
| 编码 | `encoding_rs`、`chardetng` | 检测 + 无损往返 |
| Windows | `windows` | `ReplaceFileW`、DPAPI、注册表、DirectWrite 字体枚举 |
| 其他 | `zip`、`uuid`、`sha2`、`serde`、`schemars`、`thiserror`、`tracing`、`rust-i18n`、`nucleo`、`image`、`resvg`、`ico`、`known-folders`、`directories` | 归档、ID、校验、Schema、日志、国际化、模糊匹配、图片解码、图标生成 |

Rust 工具链：1.97.1 stable-x86_64-pc-windows-msvc。安装包使用 NSIS 3（构建机需要 `makensis`；xtask 检查并提示，不自动下载）。

## 2. 工程结构

```
md-work/
  Cargo.toml                     # workspace + [workspace.dependencies] 精确锁版本
  rust-toolchain.toml
  crates/
    mdwork-core/                 # 无 UI 依赖：笔记本/笔记模型、扫描、存储、原子写、编码、回收站、搜索、归档、ACL
    mdwork-render/               # Markdown AST -> PreviewIr / Typst source；代码换行与高亮；公式
    mdwork-export/               # Typst World、PDF、PNG 拼接、导出选项、worker 协议
    mdwork-mcp/                  # rmcp server、工具、认证、ACL、审计
  apps/
    gui/                         # bin: md-work.exe
    cli/                         # bin: md-work-cli.exe（mcp serve / export / search / notebook）
  xtask/                         # SVG->ICO、NSIS 调用、打包、golden 更新
  packaging/nsis/md-work.nsi
  assets/                        # icon.svg/ico、模板、字体、locale 之外的静态资源
  crates/gui/locales/            # en.yml、zh-CN.yml
  tests/fixtures/                # Markdown 语料、zip 样本、编码样本
  tests/e2e/windows-checklist.md
```

`mdwork-core`、`mdwork-render`、`mdwork-export`、`mdwork-mcp` 不依赖 GPUI；UI 回退 Iced 时只重写 `apps/gui`。

## 3. 磁盘格式与数据模型

### 3.1 工作区

默认 `%USERPROFILE%\Documents\md-work`（通过 `known-folders` 解析，兼容 OneDrive 重定向），可设置。笔记本是工作区的直接子目录，方便在资源管理器中整体复制。

```
Documents/md-work/
  .mdwork/workspace.toml          # 格式版本 + 笔记本注册缓存（可重建）
  Work/                           # 笔记本 A
    .mdwork/
      notebook.toml               # {format:"md-work/notebook@1", id, name, created}
      bookmarks.toml              # 笔记/标题书签
      encoding.toml               # 非 UTF-8 文件的编码/EOL/BOM
      trash/                      # 回收站（meta.toml）
    assets/                       # 默认图片目录（可配置，保留名）
    notes...                      # 笔记/子文件夹直接位于笔记本根目录
  Life/                           # 笔记本 B
```

- **唯一事实来源**：`.md` 文件、`.mdwork/notebook.toml`、`bookmarks.toml`、`encoding.toml`；`workspace.toml` 只是缓存，启动时扫描 `.mdwork/notebook.toml` 重建。旧版 JSON 元数据在首次打开时自动迁移为 TOML 并改名为 `*.json.migrated`。
- 笔记身份使用**规范化相对路径**（路径分隔符统一，Windows 下大小写不敏感比较）。重命名/移动时同步更新书签与 ACL；外部重命名按“删除+新增”处理，书签显示失效并可手动重链。
- 保留目录：`.mdwork/`、`assets/`；不允许笔记本树内出现同名保留目录。
- 外部复制笔记本：扫描到同 `id` 不同路径时，把副本的 `id` 换成新 UUID、名称加 ` (2)`，保留两者并提示；同名不同 `id` 也保留，侧栏用路径提示区分；不可读/TOML 无效时给出 `EACCES/ENOENT/行号` 等明确原因。
- 支持手动“打开其他位置的笔记本”（注册到 `workspace.toml`）。

### 3.2 应用数据

```
%LOCALAPPDATA%/md-work/
  config/core.toml                # 核心配置（顶部提示不可修改）：格式版本、内部上限等
  config/runtime.toml             # 工作区、语言、面板停靠、自动保存、图床、MCP、崩溃上报
  config/theme.toml               # 皮肤、主题颜色、字体
  workspaces.toml                 # 工作区列表与别名（可手动编辑）
  sessions/<workspace-key>.toml   # 打开的笔记本、每个笔记本的标签/光标、外部标签、Dock 尺寸
  notebooks/<notebook-id>/prefs.toml
                                  # last_export_pdf_dir / last_export_png_dir / last_zip_dir
                                  # last_import_dir / 导出选项 / 预览换行 / 视图状态
  recovery/<doc-id>.json          # 未保存内容与基准哈希
  cache/{math,images,thumbs}/     # 可丢弃
  mcp/{tokens.bin,audit-YYYYMM.jsonl,server.json}
  logs/md-work-YYYY-MM-DD.log
  crashes/crash-<ts>.json         # 可选自动上报后改名 .json.reported
  dirty.toml                      # 未保存缓冲区标记（与 MCP 共享）
```

**关键修正**：导出/导入目录按 notebook id 存在应用数据中，不放在全局配置，也不随笔记本 zip 移动；切换笔记本后各自恢复自己的目录。目录失效时回退到笔记本根目录。

### 3.3 编码、回收站、归档

- 打开文件：BOM → `chardetng` → 严格 UTF-8 校验 → GB18030 → Windows-1252。非 UTF-8/非 LF 的信息写入 `encoding.toml`；保存按原编码/EOL 原子写回。
- 删除：移动整篇/整个文件夹到 `.mdwork/trash/<uuid>/payload`，`meta.toml` 记录原相对路径、类型、时间；恢复冲突时加 ` (restored)`。MCP 删除默认进回收站，永久删除需要单独权限。
- 笔记本 zip：
  - 根目录 `manifest.json`（格式、应用版本、笔记本 id/名称、文件列表 + sha256 + 大小）。
  - 包含笔记、`assets/`、`notebook.toml`、`bookmarks.toml`、`encoding.toml`；**不包含** `trash/`、缓存、应用数据中的机器相关目录。
  - 导入先解压到 `<workspace>/.import-staging/<uuid>/` 校验，再原子移入；拒绝绝对路径、`..`、UNC、符号链接逃逸、异常文件数/解压体积（默认 5 万文件 / 2 GB）。
  - 冲突：**合并**（按路径取并集，哈希相同跳过；不同则逐文件选择保留现有/采用导入/两者都留）、**覆盖**（先把现有笔记本移入其回收站，再导入）、**重命名**（新名称 + 新 id）。三种模式均在 UI 预览冲突数量。

## 4. GPUI 应用架构

### 4.1 视图与状态

- 单主窗口；设置、命令面板、导入冲突、关于为模态层，不做多窗口。
- 根 `AppShell`：工作区服务、偏好、i18n、主题、MCP 服务监控、单实例 IPC。
- `WorkspaceState`：笔记本注册表、已打开笔记本、后台扫描任务。
- `NotebookEntity`：树行、展开状态、筛选、书签、回收站、文件监控。
- `DocEntity`：缓冲区/选区（编辑器持有）、保存状态、编码、基准哈希、是否外部文件、自动保存与恢复任务。
- `TabsEntity`：每个笔记本各自的标签集合 + 外部文件标签组；激活标签决定当前笔记本与侧栏定位。
- `PreviewEntity`、`SearchEntity`（带 generation 取消）、`OutlineEntity`、`StatusBarEntity`、`ExportDialogEntity`、`McpSettingsEntity`。
- IO 在 `cx.background_executor()` 执行；核心服务同步、`Send`，不在持有锁时 await。

布局：gpui-component Dock（左侧可调整宽度的笔记本树/大纲/搜索/书签，中间标签 + 编辑/预览），底部自绘状态栏。左侧树使用 `uniform_list` 扁平化虚拟滚动，支持 1 万节点；激活标签时展开祖先、滚动到行并短暂高亮。

### 4.2 编辑器

- 以 gpui-component 代码编辑器为基座，复用缓冲区、选区、撤销/重做、IME、剪贴板、查找。
- 增加“散文模式”：隐藏行号槽、软换行、Markdown 语法高亮、列表/引用自动续行、Tab/Shift-Tab 缩进、`Ctrl+B/I`、`Ctrl+K` 链接、`Ctrl+T` 切换预览、`Ctrl+Shift+V` 纯文本粘贴。
- 粘贴/拖入图片：保存到 `assets/` 或上传图床，插入相对路径/URL。
- M0 重点验证：微软拼音连续输入 500 字无丢字/乱序、p95 输入延迟 <30ms、撤销分组、软换行、1 MB 笔记可用、1 小时连续输入不崩。若软换行/IME 不可用且 10 人日内无法修复，触发回退 Iced 的决策。

### 4.3 预览

- `mdwork-render::preview` 把 comrak AST 转为 `PreviewIr`（标题、段落、列表、任务项、引用、表格、代码块、图片、公式、脚注、分割线），保留源行号。
- 行内文本优先用 TextView；代码块、表格、任务列表、图片、公式用自研块渲染，避免受第三方 Markdown 子集限制。
- 代码块：等宽字体、背景、可选行号、**换行开关**（预览与导出分别持久化）、复制按钮、高亮 token。
- 表格：自定义网格、列宽分配、表头、单元格换行、横向滚动；复制为 TSV。
- 图片：相对路径由 core 解析，后台解码 + 缓存；远程图片默认占位。
- 公式：mitex → Typst → SVG，按（公式、字号、主题）缓存；错误显示 TeX 原文。
- 复制：不依赖视觉行，按 `PreviewIr` 生成纯文本投影：块之间一个换行、代码块保留内部换行、列表加前缀、表格用制表符、结尾不补空行。这是解决“每行后多一个空行”的验收点。
- 滚动同步：标题锚点 + 长块内插值；默认仅在切换时定位，分栏模式可选实时同步。

### 4.4 主题、字体、国际化、状态栏

- 主题：亮色/暗色/跟随系统；系统检测读 `AppsUseLightTheme`，窗口获得焦点时复查；颜色全部走语义 token。
- 字体：界面字体与笔记字体分开设置；字体枚举 + CJK 回退链（微软雅黑 UI、Segoe UI、Noto Sans SC）；代码字体等宽。PDF 使用系统字体 + 内置 OFL CJK 回退，保证换机导出不乱码。
- i18n：`rust-i18n`，仅 `zh-CN`/`en`；切换后立即刷新窗口，所有界面文案走 `t!`。
- 状态栏：总行数、字数（CJK 按字、拉丁按词）、字符数、光标行列、选区统计、编码、EOL、保存状态、当前笔记本、编辑/预览模式、MCP 服务状态。
- 命令面板：`Ctrl+Shift+P`，命令注册表 + `nucleo` 模糊匹配；**不注册双击 Shift**。

## 5. 编辑与文件生命周期

- 打开：读字节 → 检测编码 → 统一内存 EOL 但记住原始值 → 计算 sha256。笔记本内笔记与外部 `.md` 都支持；外部文件单独标签组。
- 保存：按原编码/EOL 序列化；同目录临时文件 + `ReplaceFileW`（回退 `MoveFileExW`）；更新哈希与编码 sidecar；绝不原地截断。
- 自动保存：停止输入 800ms 后保存；切换/关闭标签时若开启自动保存则先保存；失败在状态栏持续提示并可重试。
- 崩溃恢复：脏缓冲区每 2 秒写 `recovery/`（合并写入，限制大小）；启动时对比基准哈希，提供恢复列表；正常保存后删除。
- 外部修改：watcher 去抖后比较哈希。缓冲区干净则重载并尽量保持光标/滚动；脏则显示冲突条“磁盘已修改：重新加载/保留我的/另存为”；文件被删则标记缺失，可重建或关闭。
- 保存回环抑制：记录 `(路径, 期望哈希)` 短时集合，忽略自己产生的 watcher 事件；比较哈希而非 mtime，兼容 OneDrive/杀毒软件。
- GUI 与 MCP 并发：脏缓冲区在应用数据 `locks/` 下持有按笔记的咨询锁（`fs4`）；MCP 写入前检查并返回 `DOC_DIRTY`，不覆盖未保存内容；两边都用同一原子写。
- 自动加载复制入工作区的笔记本：监听工作区根目录，去抖后校验 `.mdwork/notebook.toml`；成功加入并提示，失败提示具体原因（无标记、TOML 错误、权限、路径过长）。

## 6. 导出设计

### 6.1 管线

`comrak AST → mdwork-render::typst → Typst 文档 → typst-pdf / typst-render`。

- 标题 → Typst 标题 + `#outline()` 目录（页码由 Typst 生成，无需两遍）。
- 段落/列表/引用/脚注 → 原生 Typst；表格 → `table()`，保留表头与列对齐；任务列表 → 复选符号 + 完成项删除线。
- 代码块：按页面可用宽度预换行，syntect token 转为 Typst `text(fill: ...)` 序列；保留可复制文本。若富 token 不稳定，回退单色 `raw()`。
- 公式：mitex → Typst 数学；错误降级为代码文本 + 诊断。
- 图片：相对笔记目录/笔记本根解析；缺失显示占位框 + 路径。
- 维护 Markdown 行号 → Typst 行号映射，Typst 报错可回到源码行。

### 6.2 选项与输出

- 页面：A4/Letter/A5/自定义，边距、页眉页脚（`{title}`、`{page}`、`{date}`）、页码样式、目录开关/深度、封面、字体/字号/行高、代码换行与字号、主题模板、图片最大宽度。
- PDF：Typst World 以笔记本根为资源根；字体搜索 = 系统字体 + 内置 CJK 回退；字体自动嵌入/子集化；临时文件成功后重命名。
- PNG：`typst-render` 按 dpi/72 渲染每页，逐行流式拼接到 PNG；超过 16384px 自动拆分 `name-001.png`；可选每页一个文件。
- 每个笔记本在 `prefs.toml` 中分别记忆 PDF/PNG/zip 输出目录与最近导入目录；导出对话框默认打开该目录。
- 导出在 `md-work-cli.exe export` 子进程执行，GUI 通过 stdout JSON 行显示阶段/页码；取消直接终止子进程并清理临时文件；不留下半成品。
- 验收：50 页中文 PDF <3 秒；代码换行开关对 PDF/PNG 生效；目录/页码/页眉页脚/公式/图片正确。

## 7. MCP 与 CLI

### 7.1 进程模型

- `md-work.exe`：GPUI 主程序。
- `md-work-cli.exe`：对外 CLI，子命令 `mcp serve --stdio|--http`、`export`、`search`、`notebook list/export/import`。MCP 逻辑只实现一次。
- Settings 打开“MCP HTTP 服务”时，GUI 启动 `md-work-cli.exe mcp serve --http --port 11026` 子进程并监控；GUI 退出时结束子进程。CLI 也可独立运行 HTTP/stdio，便于无 GUI 场景。
- 默认绑定 `127.0.0.1`；局域网绑定需显式开启并提示风险。端口冲突时明确报错，不静默换端口。
- GUI 与 CLI 通过文件锁 + watcher 协作；MCP 不覆盖 GUI 脏缓冲区。

### 7.2 认证与授权

- 默认拒绝：MCP 关闭、无令牌即 401。
- 令牌 32 字节随机、DPAPI 用户级加密存 `mcp/tokens.bin`；字段：id、名称、scopes、允许的笔记本、按笔记的路径允许/拒绝列表（支持 glob 高级模式）、创建/过期/最后使用/吊销。
- 作用域：`read`（列表/读取/大纲/书签）、`write`（新建/更新/移动）、`search`、`delete`（默认回收站；永久删除单独 `delete:permanent`）、`export`、`admin`。
- 每次工具调用先做路径规范化 + 笔记本根前缀校验 + ACL；搜索/列表在返回摘要前过滤，避免通过搜索泄露未授权笔记。
- 审计 JSONL：时间、令牌 id/名称、工具、笔记本、路径、字节数、结果、耗时、请求 id；不记录正文；按月轮转，默认保留 90 天。
- Settings 提供令牌创建/复制一次/轮换/吊销、按笔记本勾选权限、按笔记路径允许、审计查看、客户端配置片段。

### 7.3 工具与资源

工具：`list_notebooks`、`notebook_info`、`list_notes`、`read_note`（支持 offset/limit，返回 sha256）、`write_note`（创建/更新，支持 `expected_sha256` 并发校验与 dry-run）、`append_note`、`move_note`、`delete_note`（默认回收站）、`search_notes`、`get_outline`、`list_bookmarks`、`add_bookmark`、`export_note`。

资源：`mdwork://notebook/{id}/note/{rel_path}` 只读。暂不提供 prompts。

安全：拒绝 `..`、UNC、设备路径、符号链接/重解析点逃逸；只允许通过专用工具写 `.mdwork/bookmarks.toml` 等元数据；图片写入限制在 `assets/` 并校验 MIME/大小；所有写操作走 core 的原子写与 watcher 抑制。

客户端配置示例：

```json
{
  "mcpServers": {
    "mojian": {
      "command": "C:\\Users\\<user>\\AppData\\Local\\Programs\\md-work\\md-work-cli.exe",
      "args": ["mcp", "serve", "--stdio"],
      "env": { "MDWORK_TOKEN": "<token>" }
    }
  }
}
```

HTTP：`http://127.0.0.1:11026/mcp` + `Authorization: Bearer <token>`。

### 7.4 AI 接口预留

`mdwork-core/src/ai/mod.rs` 定义 `trait AiProvider`（补全/对话/摘要的最小接口）与空注册表；首版无实现、无网络请求，后续接入不改变 MCP 与编辑器边界。

## 8. Windows 集成与安装包

- 单实例：GPUI 初始化前创建命名管道 `\\.\pipe\md-work-<用户SID哈希>-single`；后续实例解析 argv 后发送 `OPEN\t<路径>` 并退出。首实例校验路径（存在、不是目录、扩展名允许、长度上限、`file:///` 百分号解码），然后置前并打开标签。路径无效只提示，不崩溃。
- 外部打开：若文件在某已注册笔记本内，自动打开该笔记本并定位笔记；否则作为“外部文件”标签打开，可另存入笔记本。
- 文件关联（HKCU，无需管理员）：
  - `SystemFileAssociations\.md\shell\OpenWithMoJian` → “用墨笺打开”
  - `Software\Classes\MoJian.md` + `.md` 的 `OpenWithProgids`
  - 默认关联无法静默修改，设置页提供“打开 Windows 默认应用设置”按钮。
- 安装包：NSIS 3，`RequestExecutionLevel user`，`SetShellVarContext current`；安装到 `%LOCALAPPDATA%\Programs\md-work`；开始菜单“墨笺”，桌面快捷方式可选；可选右键菜单；卸载移除程序与 HKCU 键，默认保留工作区与应用数据并询问。随包附带 `THIRD-PARTY-NOTICES.md`（GPUI/gpui-component/Typst 等 Apache-2.0，内置 CJK 字体 OFL）。
- 图标：把 `D:\Downloads\我的笔记本-copy.svg` 复制为 `assets/icon.svg`；`xtask` 用 resvg 生成 16–256 多尺寸 ICO；`winresource` 嵌入图标、VERSIONINFO、DPI/长路径 manifest；窗口图标由 GPUI AssetSource 加载 SVG/PNG。
- 日志与崩溃：`tracing` 每日滚动；panic hook 写本地 crash JSON（版本、系统、回溯、最后日志），仅本地，提供“打开文件夹”。无遥测。
- 首次启动：解析“我的文档”，创建工作区与欢迎笔记本/示例笔记；提示工作区位置与右键打开方式。

## 9. 里程碑与验收

按单人全职、包含测试估算；实际排期预留 20% 缓冲。

| 阶段 | 交付 | 估算 | 验收 |
|---|---|---|---|
| **M0 技术验证** | GPUI/gpui-component 在 Win10 19045 构建；编辑器中文 IME/软换行/撤销；TextView 能力矩阵；Typst 中文 PDF、PNG、公式；rmcp stdio/HTTP 冒烟；单实例/argv；图标 | 2 周 | 500 字连续拼音输入无丢失、p95 <30ms；20 页中文 PDF <2s；公式 SVG 正常；Explorer 打开 <300ms；任一核心项不达标即评估回退 Iced |
| **M1 核心与外壳** | 工作区/笔记本格式、扫描与重复检测、原子写、编码、会话、主题/i18n、单实例、文件关联 | 2 周 | 复制笔记本文件夹进入工作区可识别/给出原因；GBK 往返；崩溃恢复；冷启动 <600ms |
| **M2 笔记本与编辑** | 虚拟化树、多笔记本、标签、编辑器接入、自动保存、外部标签、watcher/冲突、状态栏、定位当前笔记 | 3 周 | 1 万节点滚动 60fps；外部修改冲突条；重启恢复标签/光标 |
| **M3 预览/搜索/书签/回收站** | PreviewIr、代码换行、表格/任务/图片/公式、干净复制、大纲、书签、并行搜索、回收站 → **v0.1** | 4 周 | 预览全选复制 = 规范化源文（无多余空行）；5k 笔记搜索 <2s、取消 <100ms；书签跳标题 |
| **M4 导出** | Typst PDF/PNG、目录/页码/页眉页脚/封面/字体/代码换行、每笔记本目录记忆、子进程取消 | 4 周 | 50 页 PDF <3s；中文/公式/图片正确；换行开关生效；目录按笔记本隔离；取消无残留 |
| **M5 zip 与图床** | 整本 zip 导出/导入、三种冲突、暂存/校验；本地/HTTP 图床 | 2 周 | 恶意 zip 拒绝；合并/覆盖/重命名均可用；图床上传失败回退本地并提示 |
| **M6 MCP/CLI** | stdio/HTTP、令牌/DPAPI、ACL、工具/资源、审计、沙箱、Settings UI、客户端片段 | 4 周 | MCP Inspector 全工具通过；未授权 401/403；路径逃逸被拒；脏缓冲返回冲突 |
| **M7 安装包与硬化** | NSIS 用户级安装/卸载、右键打开、性能与可访问性、E2E → **v1.0** | 3 周 | 无管理员安装/卸载干净；E2E 清单全绿；性能目标达成 |

总计约 28–32 周；v0.1 约 12–14 周。优先级：P0 工作区/树/编辑/保存/编码/搜索/大纲；P1 预览/复制/公式/PDF；P2 PNG/zip/图床/MCP；P3 命令面板/关联/打磨。

**GPUI 回退 Iced 的触发条件**：中文 IME 组合输入不可用且 10 人日内无上游修复；输入延迟 p95 >30ms 或 1 小时连续输入崩溃；软换行/撤销不可用且 fork 超过 15 人日；M4 内 TextView/自研块无法达到可接受的散文预览质量；GPUI pre-1.0 变更导致一个月内 3 次以上无法通过锁版本规避的破坏。回退只重写 `apps/gui`，core/render/export/mcp 不受影响。

## 10. 测试与验证

- 单元：扫描/去重/ID/路径规范化、原子写中断模拟、UTF-8/GBK/1252/BOM/CRLF 往返、回收站恢复、zip 路径逃逸/zip bomb、ACL 矩阵、纯文本投影、Typst 转义、代码预换行列宽。
- Golden（`insta`）：Markdown → Typst 源、PreviewIr、PDF 页数/文本抽查、PNG 尺寸/像素抽样、图标尺寸。
- 集成：headless CLI 导出；zip 往返；watcher（新增/修改/重命名/删除、回环抑制）；MCP 客户端对 HTTP/stdio 全工具；GUI 脏缓冲 vs MCP 写入并发。
- MCP Inspector：每个工具的正常/拒绝/越权/路径逃逸用例；令牌轮换；审计条目。
- Windows E2E 清单：
  - 资源管理器右键/双击 `.md`（冷启动、已运行、中文路径、超长路径、文件不存在）不崩溃，单实例转发。
  - 复制整个笔记本文件夹进工作区自动加载；无效文件夹提示原因。
  - 分别打开 A/B 笔记本导出，目录各自记忆；切换后不串。
  - GBK 笔记打开/编辑/保存/重开；代码块换行在预览、PDF、PNG 三处切换。
  - 亮/暗/跟随系统；界面与笔记字体；中英文运行时切换；状态栏行列/字数。
  - 左侧树定位当前笔记；命令面板不响应双击 Shift。
  - 用户级安装/卸载，无 HKLM 写入；卸载保留数据提示。
- 性能目标：冷启动 <600ms（热 <300ms）；5 标签空闲内存 <150MB；5MB 笔记打开 <1s、输入 <16ms；5k 笔记/100MB 搜索 <2s、取消 <100ms；50 页 PDF <3s；100 页 PNG 拼接 <10s、峰值 <500MB。

## 11. 主要风险与缓解

| 风险 | 缓解 |
|---|---|
| GPUI/gpui-component pre-1.0、Windows IME | 精确锁版本 + M0 门禁 + 保留 Iced 回退；必要时 fork 编辑器组件 |
| TextView 对图片/表格/代码换行/选择复制支持不足 | 从第一天起自建 PreviewIr 与块渲染；复制走 IR 投影；M3 验收锁定 |
| Typst 中文/字体/代码高亮/长图 | 内置 OFL CJK 回退字体；系统字体优先；代码预换行 + token 着色，失败回退单色；PNG 分页拼接 + 尺寸上限 |
| MCP 越权/路径逃逸/令牌泄露 | 默认关闭 + 回环 + DPAPI + 每次调用 ACL + 规范化沙箱 + 审计 + 模糊测试 |
| watcher 回环/OneDrive | 哈希比对、保存期望集合、去抖、重命名配对、忽略缓存目录 |
| zip 冲突/数据丢失 | 暂存 + 清单校验 + 哈希合并预览 + 覆盖前进回收站 + 大小/路径限制 |
| 复制笔记本导致 id 冲突 | 确定性检测、副本换 id、名称消歧、可见错误状态，绝不静默删除 |
| Explorer 打开/单实例竞态 | GPUI 初始化前处理 argv 与管道；路径校验；主实例置前；长路径处理 |
| 范围蔓延 | v1 明确排除 Mermaid/AI/标签/同步；`AiProvider`/`ImageHost` 只留 trait |

## 12. 关键文件

- `Cargo.toml`、`rust-toolchain.toml`：工作区与 gpui-pre/gpui-component 精确对齐
- `crates/mdwork-core/src/notebook.rs`：格式、扫描、去重、id、原子写、回收站
- `crates/mdwork-core/src/doc.rs`：编码、EOL、哈希、恢复、外部修改
- `crates/mdwork-core/src/search.rs`：并行扫描与取消
- `crates/mdwork-core/src/archive.rs`：zip 清单、校验、合并/覆盖/重命名
- `crates/mdwork-core/src/permissions.rs`：令牌作用域、笔记本/笔记 ACL、路径沙箱
- `crates/mdwork-render/src/preview.rs`：AST → PreviewIr、纯文本投影
- `crates/mdwork-render/src/typst.rs`：AST → Typst、代码预换行/高亮、行号映射
- `crates/mdwork-export/src/pdf.rs`、`png.rs`、`worker.rs`：Typst World/字体、PNG 拼接、子进程协议
- `crates/mdwork-mcp/src/server.rs`、`tools.rs`、`auth.rs`：rmcp 传输、工具、认证审计
- `apps/gui/src/state/app_shell.rs`：实体图、Dock/标签、IPC、MCP 监控、会话恢复
- `apps/gui/src/views/{tree,editor,preview,status_bar,settings}.rs`
- `apps/cli/src/main.rs`：mcp serve、export、search、notebook 子命令
- `packaging/nsis/md-work.nsi`、`xtask/src/main.rs`、`assets/icon.svg`
- `tests/e2e/windows-checklist.md`、`tests/fixtures/`

## 13. 默认决策（如无异议按此执行）

1. 工作区默认 `Documents\md-work`；笔记本直接作为工作区子目录；笔记本内容根即笔记本目录，保留 `.mdwork/` 与 `assets/`。
2. 安装目录/应用数据使用 ASCII `md-work`，界面与安装包显示“墨笺”。
3. 每笔记本的导出/导入目录存应用数据（按 notebook id），不写入 zip。
4. 在线图床首版实现“通用 HTTP 上传 + PicGo 预设”，默认仍为本地 `assets/`；S3/对象存储后续再加。
5. `.md` 默认关联通过 Windows 设置页引导；安装包提供右键“用墨笺打开”，不尝试静默改写 UserChoice。
6. MCP HTTP 默认由 GUI 启动的子进程提供（GUI 运行期间可用），stdio 可独立运行；CLI 也支持独立 HTTP。
7. 预览与导出各自保存代码块换行状态；导出默认值按笔记本记忆。
8. 首版不导入 VNote 旧笔记本格式；如需要再单独立项。