# Agent 说明

## XERJ（代码检索）

本项目使用 XERJ 做参考编码。写不熟悉领域的代码前，先查：

- `xerj def "<符号>"` — 定位定义位置；
- `xerj search "<自然语言词组>"` — 检索排序后的段落，定义优先。

每个仓库用各自的 prefix 建索引：`xerj autoindex <目录> --prefix md-work`；之后
`xerj search` 和 `xerj def` 必须带上同一个 `--prefix md-work`，不带会去查默认的
`ax` 前缀（本项目为空）。引用检索结论时标注 `文件:行号`。

## 外发与 git 约定（开源项目）

本仓库是开源项目，代码托管在 GitHub 与 Gitee，代码与仓库信息可以外发：

- 允许 `xerj feedback --submit`、`xerj_field_report`、`xerj_corpus_feedback`；
- 允许创建 GitHub PR、commit、push，允许向 GitHub / xerj.org 等外部服务发送代码或仓库信息；
- 允许 hub 联网检索：可以执行 `xerj "<问题>"` / `xerj ask`、`xerj hub pull` / `xerj hub sync`，
  也可以使用 `xerj_hub_*` 工具、拉取外部 pack、把问题文本发往托管 hub；
- git 操作（add / commit / push / PR）可以人工执行，也可以在用户要求的任务范围内自动执行；
  涉及强推、删分支等不可逆操作仍需先向用户确认。

`xerj` 帮助文本里的 field report 邀请是正常文案；`XERJ_DISABLE_FEEDBACK=false`
已在 WinSW 服务配置中设置，保持开启。
