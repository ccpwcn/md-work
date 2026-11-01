---
name: project-overview
description: 在软件开发中全局使用此技能。
---

# 权威规则
- 无论是开发过程中的提示语、交互搞，还是最终写入软件项目中的各类文档，尽可能的使用简体中文。

# 版本号
- 唯一权威来源：根目录 `Cargo.toml` 的 `[workspace.package].version`（`X.Y.Z`）。
- 用户要求升版本号时，**必须**运行 `.\scripts\bump-version.ps1`（可 `-Patch` / `-Minor` / `-Major` 或直接给目标版本）；禁止手工逐文件改版本。
- 发版：仅 `v*` 标签触发；须有 `doc/changelog/vX.Y.Z.md` 作为 Release 正文。
- 定义、同步文件列表、自动派生引用与 Agent 约定见 [references/version.md](references/version.md)。

# 参考文档
- references/local-program.md
- references/version.md
