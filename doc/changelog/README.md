# 发版说明（changelog）

每个正式版本在打 `v*` 标签前，必须在本目录放一份发版说明。GitHub 只在 `main`
上发版：`dev` 的推送（无论是否带标签）只跑测试；标签随 `dev` 合并进入 `main`
历史后，推送 `main` 才会校验该文件存在且非空，并把正文用作 Release 说明
（不再自动生成）；Gitee Go 仍由标签触发，同样校验该文件。

## 命名

```text
doc/changelog/vX.Y.Z.md
```

- 与 Git 标签一致（含 `v` 前缀），例如标签 `v1.2.0` → `doc/changelog/v1.2.0.md`
- 正文用简体中文；第一行建议是标题（如 `# 墨笺 1.2.0`）

## 发版前检查清单

1. 在 `dev` 上 `.\scripts\bump-version.ps1 X.Y.Z` 并提交
2. 写好（或核对）`doc/changelog/vX.Y.Z.md`，与功能变更一并提交
3. `git tag vX.Y.Z` 并推送 `dev` 与标签：`dev` 只跑测试，**不发版**
4. `dev` 合并进 `main` 并推送（标签须已进入 `main` 历史），由 GitHub Actions
   在 `main` 上起草并发布 Release
   （Gitee 侧的分支与标签同步见 `doc/ci.md`）

缺文件、正文为空或版本与标签不一致时，发布任务会直接失败，不会创建空 Release。
