# 持续集成与发布

md-work 是 Windows 专用的 Rust/GPUI 应用，构建依赖 MSVC、Windows SDK 与
DirectX，因此所有 CI 都必须跑在 Windows 上。

| 平台 | 测试（push / PR） | 发布 | 配置文件 |
| --- | --- | --- | --- |
| GitHub | `.github/workflows/ci.yml` | `.github/workflows/release.yml`，仅 `main` 分支且标签已进入 `main` 历史 | 已就绪，使用 `windows-latest` |
| Gitee | `.workflow/ci.yml` | `.workflow/release.yml`，`v*` 标签 | 需要自建 Windows 主机组 |

## 分支与发版约定（GitHub）

- 功能开发一律在 `dev` 上完成，向 `main` 提 PR（或直接合并）前跑通测试。
- `dev`（以及其他非 `main` 分支）的推送**只做构建与测试**：无论是否推送 `v*`
  标签，都不会创建或发布 Release。
- 在 `dev` 上升版本、写好发版说明后推送 `v*` 标签，只表示「准备发版」。
- 只有推送 `main` 才会进入发布流水线，并且要求与 `Cargo.toml` 版本一致的
  `vX.Y.Z` 标签**已进入 `main` 历史**：
  - `dev` 合并进 `main` 后推送 `main` → 校验通过后起草并发布 Release；
  - 单独推送一个尚未进入 `main` 的标签（例如在 `dev` 上推标签）→ 跳过，不发版；
  - 标签在 `main` 合并之后才推送时，推标签本身也会触发同一条流水线，条件相同。
- squash / rebase 合并会改写提交，标签不会自动进入 `main` 历史；这种情况下需
  在 `main` 上重新打同名标签后再推送。

GitHub 侧无需额外配置：任意分支（含 `dev`）、向 `main` 提 PR、以及推送
`v*` 标签都会构建并运行 `cargo test --workspace`。发布流水线会：

1. 读取 `Cargo.toml` 的 `[workspace.package].version`，要求同名 `vX.Y.Z`
   标签已存在且已进入 `main` 历史（否则跳过，不发布）；
2. 该版本若已发布则跳过；若已有 draft Release，则继续起草并发布；
3. 校验存在非空的 `doc/changelog/vX.Y.Z.md`（与标签同名，须已随
   `dev -> main` 合并进入 `main`），并将其作为 GitHub Release 正文
   （不再自动生成说明）；
4. 跑 `cargo test --workspace`，再打便携 zip 与 NSIS 安装包，先以 `main` 为
   目标创建/更新 draft Release 并附到其上，随后在同一条流水线中发布。

发布运行时的构建产物同时作为工作流 artifact 保留 90 天。`workflow_dispatch`
必须选 `main` 分支、且标签已进入 `main` 历史才会实际发版；在 `dev` 上手动触发
不会发版。

## Gitee 远端

Gitee 仓库为 <https://gitee.com/ccpwcn/md-work.git>，以镜像方式与 GitHub 同步：

```powershell
git remote add gitee https://gitee.com/ccpwcn/md-work.git   # 只需执行一次
git push origin dev
git push origin v1.2.0
git push origin main          # dev 合并进 main 后才在 GitHub 发版
git push gitee dev
git push gitee main
git push gitee v1.2.0        # Gitee Go 仍由标签触发，请在 main 合并后再推
```

## Gitee Go 流水线

Gitee Go 的云端执行器是 Linux，无法编译 GPUI/DirectX 部分，因此流水线必须绑定
**自建 Windows 主机组**：

1. 在 Gitee 仓库页打开「流水线」（Gitee Go），首次使用时按提示启用。
2. 新建主机组，类型选择 Windows，按页面提示在该机器上安装并启动 Go Agent；
   这台机器需要 Rust 1.97.1（`rust-toolchain.toml` 会自动安装）与 VS 2022 C++
   生成工具。
3. 记下主机组 ID，替换 `.workflow/ci.yml` 与 `.workflow/release.yml` 中的
   `WINDOWS_HOST_GROUP_ID`。
4. 在 Gitee Go 界面把两条流水线关联到本仓库（YAML 模式）；`release@gitee`
   步骤若首次使用，可能需要在界面上确认一次发布权限。
5. 主机组需要预装 NSIS 3（`makensis`），发布流水线里也会尝试
   `choco install nsis`。

Gitee Go 仍按标签触发，与 GitHub 的分支流程不同：

- 任意分支 push / PR：执行 `cargo build --workspace` 与 `cargo test --workspace`。
- 推送 `v*` 标签：先校验版本与 `doc/changelog/vX.Y.Z.md`，再测试、打包，并把
  `dist/md-work-<版本>-setup.exe`、`dist/md-work-<版本>-win64.zip`
  附加到同名 Gitee Release。

因此同步 Gitee 标签时需注意：推标签就会在 Gitee 侧发版；若希望两侧时机接近，
请在 `dev` 合并进 `main` 之后再向 Gitee 推送标签。

## GitHub 发布流程

```powershell
# 1. 在 dev 上完成功能开发，确认功能冻结、测试全部通过
git checkout dev
cargo test --workspace

# 2. 升版本号（同步 Cargo.toml / NSIS / README / 验收清单 / Cargo.lock）并提交
.\scripts\bump-version.ps1 1.2.0   # 或 -Patch / -Minor / -Major
git add -A
git commit -m "发布 1.2.0"

# 3. 写好发版说明（文件名必须与标签一致，含 v 前缀）并提交
#    例：doc/changelog/v1.2.0.md
#    约定见 doc/changelog/README.md

# 4. 在 dev 上打标签并推送（只表示准备发版；dev 上的推送不会发版）
git tag v1.2.0
git push origin dev
git push origin v1.2.0

# 5. 把 dev 合并进 main 并推送；标签已进入 main 历史，流水线起草并发布 Release
git checkout main
git merge --no-ff dev
git push origin main

# 6. 需要镜像到 Gitee 时，在合并完成后再推分支与标签
git push gitee dev
git push gitee main
git push gitee v1.2.0        # Gitee Go 仍由标签触发发布
```

标签须与 Cargo 版本一致，且须已进入 `main` 历史；同名发版说明须已随合并进入
`main`；缺 changelog 或正文为空时发布任务会失败，不会创建空 Release；已发布的
版本不会重复发版。`dev` 上的推送（包括 `v*` 标签）只跑测试，不发版。使用
squash / rebase 合并时标签不会进入 `main` 历史，请在 `main` 上重新打标签后再
推送。安装包保留 `/S` 静默安装参数，供升级脚本使用。

## 本地打包（只有明确要求时才执行）

```powershell
# 推荐：一键（默认 NSIS = E:\apps\nsis）
.\scripts\build-installer.ps1 -SkipTests

# 或分步
cargo run -p xtask -- package     # 便携 zip
cargo run -p xtask -- installer   # NSIS 安装包（需要 makensis）
```

开发阶段不要执行上述命令、不要运行生成的安装包；由维护者在功能冻结后统一打包
并亲自测试。安装程序会提供「选择工作区」页面（默认 `我的文档\md-work`），首次
安装时把所选文件夹写入 `HKCU\Software\md-work\WorkspaceRoot`（注册表是 Unicode，
中文路径不会乱码）；应用仅在 `config\runtime.toml`（及旧版 `config.json`）还没有工作区时读取该提示，
随后把完整配置写回 `config\` 目录，所以升级不会覆盖已有设置。
