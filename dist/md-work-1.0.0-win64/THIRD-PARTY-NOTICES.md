# 第三方软件声明 / Third-Party Notices

md-work（墨笺）以 MIT 许可证发布。程序在构建时静态链接了下列第三方开源组件。
本文件按组件的主要用途分组列出；各组件版权归其作者所有，完整许可证文本见组件源码仓库或随附的 crate 元数据。

md-work is distributed under the MIT license. The application statically links the
following third-party open-source components. Each component remains the property of
its respective authors and is used under the license noted below.

## 界面 / UI

| 组件 | 版本 | 许可证 |
| --- | --- | --- |
| gpui-kit（含 gpui、gpui-component 派生代码） | 0.8.0 | Apache-2.0 |
| tree-sitter / tree-sitter-markdown / tree-sitter-rust / tree-sitter-python / tree-sitter-javascript | 随 gpui-kit | MIT |
| syntect | 随 gpui-kit | MIT |
| rust-i18n | 4.x | MIT |

## 文档处理 / Document processing

| 组件 | 版本 | 许可证 |
| --- | --- | --- |
| comrak | 0.59 | BSD-2-Clause |
| encoding_rs | 0.8 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| chardetng | 0.1 | (Apache-2.0 OR MIT) AND BSD-3-Clause |

## 导出 / Export

| 组件 | 版本 | 许可证 |
| --- | --- | --- |
| typst, typst-layout, typst-library, typst-pdf, typst-render, typst-svg, typst-kit | 0.15.1 | Apache-2.0 |
| mitex | 0.2.4 | Apache-2.0 |
| image | 0.25 | MIT OR Apache-2.0 |

## MCP 服务 / MCP server

| 组件 | 版本 | 许可证 |
| --- | --- | --- |
| rmcp | 3.7.0 | MIT |
| tokio | 1.x | MIT |
| axum | 0.8 | MIT |
| schemars | 1.x | MIT |

## 通用基础设施 / Common infrastructure

| 组件 | 版本 | 许可证 |
| --- | --- | --- |
| serde, serde_json | 1.x | MIT OR Apache-2.0 |
| thiserror, anyhow | 2.x / 1.x | MIT OR Apache-2.0 |
| tracing, tracing-subscriber, tracing-appender | 0.1 / 0.3 / 0.2 | MIT |
| uuid | 1.x | Apache-2.0 OR MIT |
| sha2 | 0.10 | MIT OR Apache-2.0 |
| clap | 4.x | MIT OR Apache-2.0 |
| known-folders | 1.x | MIT OR Apache-2.0 |
| walkdir | 2.x | MIT OR Unlicense |
| regex | 1.x | MIT OR Apache-2.0 |
| notify | 8.x | CC0-1.0 |
| ureq | 2.9.1 | MIT OR Apache-2.0 |
| zip | 9.x | MIT |
| windows (windows-rs) | 0.61.3 | MIT OR Apache-2.0 |
| winresource（仅构建期） | 0.1.34 | MIT |

完整依赖树及精确版本见 `Cargo.lock`。如需某组件的完整许可证原文，请访问其源码仓库。
