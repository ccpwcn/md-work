# 持续集成与发布

md-work 是 Windows 专用的 Rust/GPUI 应用，构建依赖 MSVC、Windows SDK 与
DirectX，因此所有 CI 都必须跑在 Windows 上。

| 平台 | 测试（push / PR） | 发布（`v*` 标签） | 配置文件 |
| --- | --- | --- | --- |
| GitHub | `.github/workflows/ci.yml` | `.github/workflows/release.yml` | 已就绪，使用 `windows-latest` |
| Gitee | `.workflow/ci.yml` | `.workflow/release.yml` | 需要自建 Windows 主机组 |

GitHub 侧无需额外配置：推送分支或向 `main` 提 PR 会构建并运行
`cargo test --workspace`；推送 `v*` 标签会构建便携 zip 与 NSIS 安装包，并附到
GitHub Release。发布流水线会先校验标签与 `Cargo.toml` 的版本号一致（避免打出
错误版本），构建产物同时作为工作流 artifact 保留 90 天。两条流水线都支持在
Actions 页面手动触发（`workflow_dispatch`）。

## Gitee 远端

Gitee 仓库为 <https://gitee.com/ccpwcn/md-work.git>，以镜像方式与 GitHub 同步：

```powershell
git remote add gitee https://gitee.com/ccpwcn/md-work.git   # 只需执行一次
git push origin main
git push gitee main
git push gitee --tags
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

之后的行为与 GitHub 一致：

- 任意分支 push / PR：执行 `cargo build --workspace` 与 `cargo test --workspace`。
- 推送 `v*` 标签：运行测试，执行 `cargo run -p xtask -- package` 与
  `cargo run -p xtask -- installer`，并把
  `dist/md-work-<版本>-setup.exe`、`dist/md-work-<版本>-win64.zip`
  附加到同名 Gitee Release。

## 发布流程（两个平台相同）

```powershell
# 1. 确认功能已冻结、测试全部通过
cargo test --workspace

# 2. 更新版本号（Cargo.toml 的 workspace.package.version）并提交
# 3. 打标签并同时推送两个远端
git tag v1.0.0
git push origin main
git push gitee main
git push origin v1.0.0
git push gitee v1.0.0
```

标签推送后，GitHub Actions 与 Gitee Go 各自构建安装包并创建 Release。安装包保
留 `/S` 静默安装参数，供升级脚本使用。

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
