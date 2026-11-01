# 发版说明（changelog）

每个正式版本在打 `v*` 标签前，必须在本目录放一份发版说明。GitHub / Gitee
发布流水线会校验该文件存在且非空，并把正文用作 Release 说明（不再自动生成）。

## 命名

```text
doc/changelog/vX.Y.Z.md
```

- 与 Git 标签一致（含 `v` 前缀），例如标签 `v1.2.0` → `doc/changelog/v1.2.0.md`
- 正文用简体中文；第一行建议是标题（如 `# 墨笺 1.2.0`）

## 发版前检查清单

1. `.\scripts\bump-version.ps1 X.Y.Z` 并提交
2. 写好（或核对）`doc/changelog/vX.Y.Z.md`，与功能变更一并提交
3. `git tag vX.Y.Z` 后推送代码与标签

缺文件或版本与标签不一致时，发布任务会直接失败，不会创建空 Release。
