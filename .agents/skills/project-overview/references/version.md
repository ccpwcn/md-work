# 版本号规则（墨笺 / md-work）

## 权威来源

- **唯一权威**：仓库根目录 `Cargo.toml` → `[workspace.package].version`
- 格式：严格 `X.Y.Z`（semver 数字三段，无 `v` 前缀、无预发布后缀）
- Git 发版标签：`v` + 版本号，例如 `v1.1.1`；CI 会校验标签与 Cargo 版本一致

## 升版方式（必用脚本）

用户说「升版本号 / bump version / 升到 x.y.z」时：

```powershell
.\scripts\bump-version.ps1 1.1.2      # 指定目标
.\scripts\bump-version.ps1 -Patch     # 1.1.1 → 1.1.2
.\scripts\bump-version.ps1 -Minor     # 1.1.1 → 1.2.0
.\scripts\bump-version.ps1 -Major     # 1.1.1 → 2.0.0
.\scripts\bump-version.ps1 -Patch -DryRun
```

- **不要**手工逐个文件改版本号；一律跑上述脚本。
- 脚本**不会** git commit / tag / push；提交与打标签由用户明确要求后再做。
- 升版 ≠ 打安装包；安装包仍须用户明确要求后才执行 `.\scripts\build-installer.ps1` 或 xtask。

## 脚本会同步改写的文件

| 文件 | 改什么 |
| --- | --- |
| `Cargo.toml` | `[workspace.package].version` |
| `packaging/nsis/md-work.nsi` | 注释中的示例路径、`!ifndef VERSION` 下的回退 `!define VERSION` |
| `README.md` | 「当前版本：…」与示例产物路径中的版本段 |
| `tests/e2e/windows-checklist.md` | 标题与清单内硬编码版本 |
| `Cargo.lock` | 经 `cargo check --workspace` 刷新工作区成员版本 |

若以后新增「需要写死版本号」的文档或脚本，必须同时改 `scripts/bump-version.ps1` 的同步列表，并更新本文件。

## 运行时自动对齐、禁止手改的引用

这些从 `version.workspace = true` / `env!("CARGO_PKG_VERSION")` / xtask 注入派生，**不要**在源码里再写一份字面量版本：

| 用途 | 位置 |
| --- | --- |
| 各 crate 包版本 | 各 `Cargo.toml` 的 `version.workspace = true` |
| `mdwork_core::version()` / MCP serverInfo | `crates/mdwork-core` |
| CLI `--version` | clap / `CARGO_PKG_VERSION` |
| 设置页「墨笺 x.y.z · MIT」 | `apps/gui` |
| Windows VERSIONINFO | `apps/gui/build.rs` + winresource |
| 便携包/安装包文件名 | `xtask`（`md-work-<ver>-win64`、`md-work-<ver>-setup.exe`） |
| NSIS 正式构建 | xtask 传 `/DVERSION=...`（覆盖 nsi 回退值） |
| Release 标签校验 | `.github/workflows/release.yml` 等 |

## Agent 执行约定

1. 用户要求升版 → 只运行 `scripts/bump-version.ps1`（可先 `-DryRun`）。
2. 升版后按 `md-work-dev-workflow`：`cargo build --workspace` 与 `cargo test --workspace`（脚本已做 `cargo check`；完整 test 仍建议跑）。
3. 不要顺手打 NSIS 包、不要推 tag，除非用户明确说。
4. 不要在业务代码或新文档里再复制一份硬编码版本；能读 Cargo / env 就读 Cargo / env。
