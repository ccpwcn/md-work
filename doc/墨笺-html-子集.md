# 墨笺受控 HTML 子集

Markdown 笔记中允许的极小白名单 HTML。由 `mdwork-render` 解析进 `PreviewIr`，GPUI 预览与 Typst（PDF/PNG）导出共用同一语义。不上浏览器内核。

标准 `![alt](url)` 不变；需要尺寸或块/行样式时写本规范标签。

## 1. `img`

允许属性：`src`、`alt`、`title`、`width`、`height`。

- `width` / `height`：`100`、`100px`（像素）或 `50%`（相对预览/导出内容区宽度）
- 只设 `width`：按原图宽高比自动算高
- 只设 `height`：按比例推算宽度
- 其它属性忽略

## 2. `div` / `span`

允许属性：`id`、`name`、`style`、`class`、`title`。

- `class`：空格分隔，样式表中不存在的类名忽略
- **内容视为纯文本**：不解析内部 Markdown/HTML，嵌套标签按字面文本处理
- `div` 块级，`span` 行内

## 3. `style`

- 源码中保留；**预览与 PDF/PNG 均不展示该块**
- 选择器仅 `#id`、`.class`（无后代、无逗号组合、无标签选择器）
- 声明仅：
  - `fontSize` / `font-size`
  - `width` / `height`（`px` 或 `%`）
  - `color` / `backgroundColor` / `background-color`（`#rgb` / `#rrggbb`）
- 亦兼容笔误别名 `backgroudColor` / `backgroud-color`
- `@` 规则、`url(`、其它属性一律忽略
- 规则只作用于本笔记中的 `img` / `div` / `span`

## 4. 样式优先级（低 → 高）

1. 默认外观
2. `<style>` 中 `.class`（同属性以后出现的规则为准；元素多个 class 按书写顺序叠加）
3. `<style>` 中 `#id`
4. 元素 `style="..."` 内联（仅上表属性）
5. `img` 的 `width` / `height` 属性（覆盖样式表与内联的同名尺寸）

## 5. 安全

- 标签 / 属性 / CSS 属性三重白名单
- 忽略 `on*`、`javascript:`、`url(`
- 未知标签默认丢弃
- 图片 `src` 仍走应用既有本地路径解析

## 6. 示例

```markdown
<style>
.narrow { width: 40%; }
#warn { color: #c00; background-color: #fee; font-size: 14px; }
</style>

<img src="../assets/demo.jpg" alt="示意" width="300px" />

<div id="warn" class="narrow" title="提示">**这里不会变成粗体**</div>

正文中有 <span style="color:#06c">蓝色字</span>。
```
