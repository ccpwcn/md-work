# 墨笺 MCP 服务与工具使用说明

> 适用版本：v1.2.4；验证日期：2026-11-08；平台：Windows 10/11 x64。
> 本文说明 `md-work-cli.exe` 提供的 MCP 服务（stdio / Streamable HTTP）、AI Agent 如何发现并使用它的能力，以及配置与排障时的注意事项。
> 相关源码：`apps/cli/src/main.rs`、`crates/mdwork-mcp/`、`crates/mdwork-core/src/{permissions,mcp_permissions}.rs`、`apps/gui/src/app/mcp_settings.rs`。

---

## 一、结论速览：AI Agent 能否获知能力

**能，且能力发现链路完整。** MCP 客户端按标准握手即可知道这个服务能做什么，不需要额外文档或硬编码：

| 发现途径 | 服务返回的内容 | 实测结果 |
| --- | --- | --- |
| `initialize` | `capabilities.tools`、`capabilities.resources`、`instructions`（服务用途、写入需带 sha256、删除默认进回收站等约定）、`protocolVersion` | 正常，`2025-06-18` |
| `tools/list` | 14 个工具，每个都有 `description`、`inputSchema`、`outputSchema`、`annotations` | 正常，14 个 |
| `resources/templates/list` | 笔记资源 URI 模板 `mdwork://notebook/{notebook_id}/note/{note_path}` | 正常，1 个 |
| `resources/list` / `resources/read` | 当前令牌可见的笔记列表与 Markdown 原文 | 正常 |
| `tools/call` → `server_status` | 当前令牌的 `scopes`、`workspace_root`、版本、笔记本数量 | 正常，可用于自检 |

stdio 实测完成 `initialize` → `tools/list` → `tools/call`（`server_status`、`list_notebooks`、`write_note`、`read_note`）→ `resources/list` → `resources/read`；Streamable HTTP 实测完成 `initialize` → `tools/list` → `tools/call`（`server_status`）→ `resources/list`，均返回预期结果。`cargo test -p mdwork-mcp -p md-work-cli` 全部通过。

**已知边界（不影响能力发现，但需了解）**：

- `initialize` 的 `serverInfo` 是框架默认值 `{"name":"rmcp","version":"3.7.0"}`，客户端里显示的服务名不是「墨笺」；识别服务请用 `instructions` 或 `server_status` 返回的 `name: 墨笺`。
- 只声明了 `tools` + `resources` 能力，未提供 `prompts`、采样（sampling）、资源订阅等 MCP 可选能力。
- `tools/list` 返回的是**静态全集**，不会按当前令牌的 scopes 裁剪；权限在 `tools/call` 时才判定。Agent 应先用 `server_status` 查询自身权限，越权工具调用会返回 `-32003 FORBIDDEN`。
- 工具描述与 `instructions` 均为英文；其余面向用户的界面文案有中文。

---

## 二、组件与启动方式

MCP 逻辑只实现一次，放在 `mdwork-mcp` crate 中，由独立 CLI 可执行文件对外提供：

```text
md-work.exe            GUI 主程序（设置页、令牌与权限编辑、拉起 HTTP 子进程）
md-work-cli.exe        对外 CLI：mcp serve / export / search / notebook ...
```

### 1. 命令

```powershell
md-work-cli.exe mcp serve --stdio                  # stdio 传输（不传任何参数时也是 stdio）
md-work-cli.exe mcp serve --http                  # Streamable HTTP，默认端口 11026
md-work-cli.exe mcp serve --http --port 12000     # 指定端口
md-work-cli.exe --version                         # md-work-cli 1.2.4
```

- `--stdio` 与 `--http` 互斥；`--port` 仅对 HTTP 生效，默认 `11026`。
- HTTP 只绑定 `127.0.0.1`，端点为 `http://127.0.0.1:<port>/mcp`；端口被占用时明确报错，不会静默换端口。启动时会在 stderr 打印监听地址；**stdout 专用于 stdio 协议**，日志一律走 stderr 与文件（`%LOCALAPPDATA%\md-work\logs`）。
- 独立运行的 HTTP 服务用 `Ctrl+C` 结束；由 GUI 拉起时，GUI 还传入 `MDWORK_PARENT_PID`，GUI 退出会自动结束子进程。

### 2. 两种服务模型

| | HTTP（GUI 托管） | stdio（客户端拉起） |
| --- | --- | --- |
| 启动者 | 勾选「设置 → MCP → 服务」开关后由 GUI 启动子进程 | AI 客户端直接拉起 CLI |
| 生命周期 | 仅在 GUI 运行且开关打开期间可用 | 随客户端会话启停，不受 GUI 开关控制 |
| 工作区 | GUI 显式传入 `MDWORK_WORKSPACE`，避免读到刚切换前的旧配置 | 由环境变量/GUI 配置决定 |
| 鉴权 | 每个请求带 `Authorization: Bearer <token>` | 启动时通过 `env.MDWORK_TOKEN` 传入 |

GUI 默认不启动 HTTP 服务（`mcp_http = false`），端口默认 `11026`。stdio 与 CLI 自带的 HTTP 都可以在无 GUI 场景下独立运行。

### 3. 工作区解析顺序

1. 环境变量 `MDWORK_WORKSPACE`；
2. `%LOCALAPPDATA%\md-work\config\core.toml` 中 GUI 保存的 `workspace_root`；
3. `%USERPROFILE%\Documents\md-work`。

独立运行 stdio（尤其是 Agent 在别的目录里拉起 CLI）时，建议显式设置 `MDWORK_WORKSPACE`，否则可能服务到 GUI 上次打开的工作区。

---

## 三、准备工作（推荐用 GUI 完成）

CLI 本身**不提供生成令牌的子命令**，令牌必须在 GUI 中创建。

1. **新建令牌**：设置 → MCP → 令牌 → 新建令牌。
   - 名称自定义；scopes 默认只勾 `read`、`search`；
   - 「可访问的笔记本」必须至少选一个（默认选中当前打开的笔记本），空列表等于什么都访问不到；
   - 笔记路径白名单/黑名单支持 `*` 与 `**` glob，多个用逗号分隔，**黑名单优先**，白名单留空表示允许全部笔记，例如 `工作/**`、`**/私密*`。
   - 令牌形如 `mdt_<令牌ID>_<密钥>`，**只显示一次**，关闭窗口后无法再查看；之后可「轮换密钥」「吊销」「删除」。
2. **授权笔记**：设置 → MCP → 权限。四级权限：`隐藏 < 只读 < 修改 < 删除`；支持「父子联动 / 单独授权」。
   - **默认全部隐藏**；新建笔记本、文件夹、笔记同样默认隐藏，需要重新授权；
   - 变更即时保存，但**只对新建的 MCP 会话生效**，已连接的客户端要重连；
   - 有效权限取自身与全部祖先的最小值，隐藏的父节点会隐藏整棵子树。
3. **启用 HTTP 服务**（仅 HTTP 方式需要）：设置 → MCP → 服务，打开开关、确认端口、点「应用」。状态栏显示 `MCP :<端口>`。
4. **复制客户端配置**：服务页下方有「HTTP / stdio」两个标签与复制按钮，直接把生成的 `mcpServers` JSON 粘到 AI 客户端配置里。刚创建或轮换令牌时片段里是真实令牌，否则显示占位符 `mdt_<令牌ID>_<创建时显示的密钥>`。
5. **审计**：设置 → MCP → 审计，查看最近的工具调用（不记录笔记正文）。

> 权限是**两层取交集**：令牌的 scopes / 笔记本白名单 / 路径 glob，再加上权限页的四级权限树。少配一层，Agent 就会看到空列表或收到 `FORBIDDEN`。

---

## 四、客户端配置

### HTTP（推荐，GUI 运行期间可用）

```json
{
  "mcpServers": {
    "md-work": {
      "type": "http",
      "url": "http://127.0.0.1:11026/mcp",
      "headers": { "Authorization": "Bearer mdt_<令牌ID>_<密钥>" }
    }
  }
}
```

### stdio（不依赖 GUI 是否打开）

```json
{
  "mcpServers": {
    "md-work": {
      "command": "C:\\Users\\<user>\\AppData\\Local\\Programs\\md-work\\md-work-cli.exe",
      "args": ["mcp", "serve", "--stdio"],
      "env": { "MDWORK_TOKEN": "mdt_<令牌ID>_<密钥>" }
    }
  }
}
```

要点：

- `command` 建议写绝对路径；安装版默认在 `%LOCALAPPDATA%\Programs\md-work\md-work-cli.exe`。
- stdio 通过 `env.MDWORK_TOKEN` 传令牌；缺少令牌或令牌无效时 CLI 启动即退出，错误写入日志/stderr。
- HTTP 方式**每个请求**都要带 `Authorization` 头；无令牌或错令牌返回 HTTP `401` 并带 `WWW-Authenticate: Bearer realm="md-work"`。
- 含令牌的配置等同于笔记本读写权限，不要提交到仓库或共享；令牌库用 DPAPI 加密，绑定当前 Windows 用户，换用户/换机器后原令牌不可用。
- 客户端、CLI、GUI 应保持同一版本（导出等功能对版本不一致有明确报错）。

---

## 五、AI Agent 如何发现并使用能力（重点）

### 1. 握手与能力声明

客户端发起 `initialize`（`protocolVersion: 2025-06-18`），服务端返回：

```json
{
  "protocolVersion": "2025-06-18",
  "capabilities": { "tools": {}, "resources": {} },
  "serverInfo": { "name": "rmcp", "version": "3.7.0" },
  "instructions": "MoJiān (墨笺) notebook server. All calls use the token's notebook and path ACL. Notes are UTF-8/GBK text; write_note expects the sha256 returned by read_note. Deletes go to the notebook trash unless delete:permanent is granted."
}
```

随后客户端发送 `notifications/initialized`，即可 `tools/list`。stdio 是每行一个 JSON-RPC 消息（JSON Lines）；HTTP 是 Streamable HTTP POST。

### 2. `tools/list` 返回什么

每个工具都带名称、英文描述、输入/输出 JSON Schema 与注解，例如（字段已节选）：

```json
{
  "name": "read_note",
  "description": "Read a note (optionally a line range) plus its encoding, EOL and sha256",
  "inputSchema": { "type": "object", "properties": { "notebook_id": {}, "path": {}, "offset": {}, "limit": {} }, "required": ["notebook_id", "path"] },
  "outputSchema": { "type": "object", "properties": { "...": "字段省略" } },
  "annotations": { "readOnlyHint": true }
}
```

注解语义：`readOnlyHint`（只读）、`destructiveHint`（破坏性，仅 `delete_note` 为 `true`）、`idempotentHint`（幂等，如 `add_bookmark`）。客户端可据此决定是否弹确认。

### 3. 工具清单（14 个，静态全集）

| 工具 | 作用 | 所需 scope | 主要参数 |
| --- | --- | --- | --- |
| `server_status` | 版本、工作区根、笔记本数、令牌名称与 scopes（**无需 scope**，建议作为连通性自检第一步） | — | 无 |
| `list_notebooks` | 列出令牌可访问且权限树未隐藏的笔记本 | `read` | 无 |
| `notebook_info` | 笔记本元数据与可见笔记/文件夹统计 | `read` | `notebook_id` |
| `list_notes` | 列出笔记与文件夹，可按 `folder` 过滤；隐藏项不返回 | `read` | `notebook_id`, `folder?` |
| `read_note` | 读取笔记（可按行范围），返回编码、EOL、BOM、`sha256` | `read` | `notebook_id`, `path`, `offset?`, `limit?`（默认 400 行，上限 5000） |
| `get_outline` | 标题大纲（层级、标题、1 起始行号） | `read` | `notebook_id`, `path` |
| `list_bookmarks` | 笔记本的书签（过滤隐藏笔记） | `read` | `notebook_id` |
| `search_notes` | 跨笔记本全文搜索（纯文本或正则） | `search` | `query`, `notebook_id?`, `regex?`, `case_sensitive?`, `max_hits?`（默认 200，上限 1000） |
| `write_note` | 新建或整体替换笔记（原子写，保留原编码/BOM/EOL） | `write` | `notebook_id`, `path`, `text`, `expected_sha256?`, `create?`, `dry_run?` |
| `append_note` | 追加文本，必要时补一个换行 | `write` | `notebook_id`, `path`, `text`, `expected_sha256?`, `create?` |
| `move_note` | 重命名/移动，书签随行 | `write` | `notebook_id`, `from`, `to` |
| `delete_note` | 删除；默认进笔记本回收站，`permanent` 需额外 scope | `delete`（永久删除还需 `delete:permanent`） | `notebook_id`, `path`, `permanent?` |
| `add_bookmark` | 为笔记或标题加书签（幂等） | `write` | `notebook_id`, `path`, `heading?` |
| `export_note` | 导出 PDF / PNG / WebP 到应用导出目录并返回文件路径 | `export` | `notebook_id`, `path`, `format?`, `output_name?`, `title?` |

`server_status`、`list_notebooks` 的入参是空对象，客户端可直接无参调用。

### 4. 资源（resources）

- 模板：`mdwork://notebook/{notebook_id}/note/{note_path}`（`note_path` 需百分号编码）。
- 实际 URI：`mdwork://notebook/<uuid 无连字符>/note/<编码后的相对路径>`，例如 `mdwork://notebook/728dc7e7da9b4c679cc84001c6fd17fc/note/probe.md`。
- `resources/list` 最多返回 500 条，且只列令牌 ACL 与权限树都允许的笔记；`resources/read` 返回 `text/markdown` 原文。
- 大工作区请用 `list_notes` / `search_notes` 定位，不要依赖 `resources/list` 遍历全部。

### 5. 建议的调用顺序

```text
读：server_status → list_notebooks → notebook_info / list_notes → read_note / get_outline / resources/read → search_notes
写：read_note（拿 sha256）→ write_note（带 expected_sha256；可先 dry_run）→ append_note / move_note / add_bookmark / delete_note / export_note
```

`tools/call` 示例：

```json
{"jsonrpc":"2.0","id":7,"method":"tools/call",
 "params":{"name":"write_note","arguments":{
   "notebook_id":"<UUID>","path":"工作/设计.md","text":"# 设计\n正文\n",
   "expected_sha256":"<read_note 返回的 sha256>"}}}
```

响应同时给出 `content[0].text`（JSON 字符串）与 `structuredContent`（对象），Agent 优先解析 `structuredContent`。`write_note` 的 `sha256` 校验用于防止覆盖 GUI 或其他 Agent 的并发修改；哈希不一致返回 `CONFLICT` 并附带 `expected_sha256` / `actual_sha256`。

---

## 六、scopes、错误码与数据位置

### 1. 令牌 scopes（7 个）

| scope | 允许的工具 |
| --- | --- |
| `read` | 列举、读取、大纲、书签、资源 |
| `write` | `write_note`、`append_note`、`move_note`、`add_bookmark` |
| `search` | `search_notes` |
| `delete` | `delete_note`（默认回收站） |
| `delete:permanent` | `delete_note` 的 `permanent: true` |
| `export` | `export_note` |
| `admin` | 覆盖以上全部 scope（仍受笔记本/路径 ACL 与权限树约束） |

### 2. 错误码

HTTP 层无令牌/错令牌直接是 `401`；工具层的越权与业务错误是 JSON-RPC error，`data.code` 为下列标签：

| code | 标签 | 含义 |
| --- | --- | --- |
| `-32001` | `UNAUTHORIZED` | 令牌缺失/无效（stdio 未设 `MDWORK_TOKEN` 会在启动阶段报错） |
| `-32002` | `NOT_FOUND` | 笔记本或笔记不存在 |
| `-32003` | `FORBIDDEN` | 缺少 scope、笔记本/路径被 ACL 或权限树拒绝 |
| `-32009` | `CONFLICT` | 目标已存在，或 `expected_sha256` 不匹配 |
| `-32010` | `DOC_DIRTY` | GUI 中该笔记有未保存修改，拒绝写入；先在 GUI 保存 |
| `-32011` | `INVALID_PATH` | 路径越界、绝对路径、保留名等 |
| `-32012` | `EXPORT_FAILED` | 导出失败 |

> 注意：ACL 越权**不是 HTTP 403**，而是工具调用返回的 `-32003`；参数不合法走标准 `-32602 INVALID_PARAMS`，内部错误为 `-32603`。

### 3. 相关文件

| 内容 | 位置 |
| --- | --- |
| 令牌库（SHA-256 哈希 + DPAPI 加密） | `%LOCALAPPDATA%\md-work\mcp\tokens.bin` |
| 权限树、签名、密钥 | `%LOCALAPPDATA%\md-work\mcp\permissions.toml`、`permissions.toml.sig`、`permissions.key` |
| 审计日志（按月 JSONL，保留 90 天，不记正文） | `%LOCALAPPDATA%\md-work\mcp\audit\YYYY-MM.jsonl` |
| GUI 脏缓冲标记 | `%LOCALAPPDATA%\md-work\dirty.toml` |
| `export_note` 输出 | `%LOCALAPPDATA%\md-work\exports\` |
| 日志 | `%LOCALAPPDATA%\md-work\logs\` |
| 配置（工作区、端口、服务开关） | `%LOCALAPPDATA%\md-work\config\{core,runtime}.toml` |

`permissions.toml` 是明文可读的，但由 HMAC 签名保护：外部程序手改文件会导致验签失败，服务按「全部隐藏」失效关闭，需要在 GUI 权限页重新授权来重建签名。

---

## 七、注意事项（务必逐条确认）

1. **三处默认拒绝**：MCP HTTP 服务默认关闭；无令牌返回 401；权限树默认全部隐藏。生成令牌、勾选笔记本、在权限页授权、打开服务开关，缺一不可。
2. **工具清单不等于可用清单**：`tools/list` 永远返回 14 个工具。Agent 应先 `server_status` 看 `scopes`，再决定调用范围；越权调用返回 `-32003`，而不是「工具不存在」。
3. **权限变更需要重连**：服务在建立会话时加载权限快照，GUI 中改权限/令牌后，已连接的客户端要断开重连才生效。
4. **新节点默认隐藏**：新建笔记本、文件夹、笔记后要重新授权；重命名/移动会迁移对应的权限记录。
5. **令牌只显示一次**：明文只在创建/轮换时出现，令牌库只存哈希且用当前用户的 DPAPI 加密；换 Windows 用户、换机器、把 `tokens.bin` 拷走都不可用。含令牌的 `mcpServers` 配置不要提交到仓库、贴到公开场合；stdio 方式下令牌会出现在子进程的 `env` 中。
6. **服务标识**：客户端可能把服务显示成 `rmcp 3.7.0`（`serverInfo` 未定制）；用 `instructions` 或 `server_status.name` 判断是不是墨笺。工具描述与 instructions 为英文。
7. **stdio 的 stdout 是协议通道**：CLI 日志走 stderr 与文件。不要给 `command` 套会往 stdout 打印内容的包装脚本，否则 JSON-RPC 帧会被污染。
8. **HTTP 是 Streamable HTTP**：需要 `Content-Type: application/json`、`Accept: application/json, text/event-stream`，`initialize` 响应头的 `Mcp-Session-Id` 要带回后续请求；**每个请求**都要带 `Authorization` 头。仅监听 `127.0.0.1`，不要做端口转发或改绑到公网地址。
9. **写保护**：GUI 有未保存修改时拒写（`DOC_DIRTY`）；写前用 `read_note` 取 `sha256` 并作为 `expected_sha256` 传入，避免并发覆盖；路径一律按笔记本内相对路径规范化，逃逸会被拒。删除默认进回收站，永久删除必须显式授予 `delete:permanent`。
10. **隐蔽过滤**：`list_notebooks`、`list_notes`、`search_notes`、`list_bookmarks`、`resources/list` 都会按 ACL 与权限树过滤，连隐藏笔记的数量/名字都不会泄露；返回空列表时先检查权限页，而不是怀疑工作区为空。
11. **端口与生命周期**：端口占用会明确报错，不自动改端口；GUI 关闭开关或退出后 HTTP 连接立即不可用，stdio 不受影响。CLI 与 GUI 版本需一致。
12. **能力边界**：无 `prompts`、采样、资源订阅等可选能力；`resources/list` 上限 500 条；导出依赖本地 Typst 组件，失败返回 `EXPORT_FAILED`。

---

## 八、手工排查

先用最简单的连通性检查（应返回 401）：

```powershell
curl.exe -i http://127.0.0.1:11026/mcp
```

手工发 MCP 消息时，等价的 HTTP 请求形态（实测可用）如下：

```http
POST /mcp HTTP/1.1
Host: 127.0.0.1:11026
Content-Type: application/json
Accept: application/json, text/event-stream
MCP-Protocol-Version: 2025-06-18
Authorization: Bearer mdt_<令牌ID>_<密钥>
Content-Length: ...

{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}
```

从 `initialize` 响应头取出 `Mcp-Session-Id`，后续请求加上该头，例如 `{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}` 与 `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"server_status","arguments":{}}}`。日常使用优先用 MCP Inspector 或客户端自带调试器，避免在 shell 里处理 JSON 引号与 SSE 响应。

| 现象 | 常见原因 | 处理 |
| --- | --- | --- |
| stdio 客户端一启动就断开 | 未设 `MDWORK_TOKEN`，或令牌无效/已吊销 | 在 GUI 重新生成令牌并写入 `env` |
| HTTP 全部 `401` | 缺 `Authorization` 头、令牌写错、令牌库不属于当前用户 | 核对片段里的令牌；必要时轮换 |
| `tools/list` 有工具，调用却 `-32003` | scope 不够、笔记本未勾选、glob 拒绝、权限树隐藏 | 用 `server_status` 看 scopes；检查令牌与权限页 |
| 笔记本列表为空 / 资源为空 | 权限树默认隐藏，或令牌未勾选该笔记本 | 在「设置 → MCP → 权限」授权后重连 |
| 写入返回 `DOC_DIRTY` | GUI 里该笔记有未保存修改 | 先在 GUI 保存，再重试 |
| 写入返回 `CONFLICT` | 文件在读取后被改动，`sha256` 过期 | 重新 `read_note` 拿新的 `sha256` |
| 启动 HTTP 提示端口被占用 | 已有 GUI 或另一个 CLI 在监听 | 停掉旧进程，或换 `--port` 并同步改客户端 URL |
| GUI 退出后 HTTP 连不上 | GUI 托管的子进程随 GUI 结束 | 改用 stdio，或独立启动 `mcp serve --http` |

---

## 九、验证记录

本次文档基于以下实测（v1.2.4，2026-11-08）：

- `cargo build -p md-work-cli`：构建成功；`md-work-cli.exe --help`、`mcp serve --help`、`--version` 输出与本文一致。
- `cargo test -p mdwork-mcp -p md-work-cli`：40 项既有测试 + 1 项临时端到端探针，全部通过；既有测试覆盖令牌/DPAPI、权限树失效关闭、ACL 矩阵、HTTP 401、写入冲突、脏缓冲保护、工具 schema 注册等（删除临时探针后复跑为 40 项）。
- 临时端到端探针：用真实 CLI 二进制分别以 stdio 与 Streamable HTTP 启动，都完成了 `initialize`、`notifications/initialized`、`tools/list`（14 个工具及输入/输出 schema）、`tools/call` 与 `resources/list`；stdio 额外验证了 `resources/templates/list`、`resources/read` 以及 `write_note` / `read_note` 写入读回。全部返回预期结果。探针为一次性验证文件，验证后已从仓库删除。
