# 墨笺受控 HTML 子集

墨笺允许在 Markdown 笔记中写**极小的 HTML 白名单子集**，用来做Markdown 本身不擅长的事：控制图片尺寸、给一段文字上色、加粗、做块级提示框。它由 `mdwork-render` 解析进 `PreviewIr`，**GPUI 预览**与 **Typst 导出（PDF / PNG）**共用同一套语义，不启动浏览器内核，也不会执行脚本。

标准 Markdown 写法（例如 `![alt](url)`、`**粗体**`）完全不受影响；只有当你需要尺寸或样式时，才使用本规范里的标签。

> 一句话记忆：**只有 `img` / `div` / `span` / `style` 四个标签；只有下表中列出的属性与 CSS 声明会生效。**

## 1. 支持的标签与属性

| 标签 | 允许属性 | 说明 |
| --- | --- | --- |
| `img` | `src`、`alt`、`title`、`width`、`height`、`id`、`class`、`style` | 图片；尺寸与样式规则见下文 |
| `div` | `id`、`name`、`class`、`style`、`title` | 块级文本容器 |
| `span` | `id`、`name`、`class`、`style`、`title` | 行内文本容器 |
| `style` | 无 | 只用于写 CSS 规则，**自身不会显示出来** |

其它标签（如 `<script>`、`<iframe>`、`<b>`、`<a>`、`<table>`）一律丢弃，标签内的文字会保留。`on*` 事件属性、`javascript:` 伪协议、`url(...)` 等不生效。

### `img`

- `width` / `height` 可写 `300`、`300px` 或 `50%`。百分比相对于预览 / 导出的内容区宽度。
- 只写 `width`：按图片原始宽高比自动算高；只写 `height`：按比例反推宽度。
- `alt` 与 `title` 保持通常语义；图片路径仍走墨笺既有的本地图片解析规则。

```markdown
<img src="../assets/cover.jpg" alt="封面" title="封面图" width="60%" />

<img src="../assets/icon.png" alt="图标" width="64px" height="64px" />
```

### `div` / `span`

- `div` 块级，`span` 行内。
- `class` 用空格分隔多个类名。
- **内容视为纯文本**：标签内部不解析 Markdown，也不解析嵌套 HTML，嵌套标签会按字面文本显示。例如 `<div>**这不是粗体**</div>` 会原样显示星号。
- 想让文字带样式，请用 `style` 属性或 `<style>` 规则里的 `font-weight`；Markdown 的 `**粗体**` 请写在标签外面。

```markdown
这一段是 <span style="color:#0a84ff">蓝色</span> 和普通文字混排。

<div id="tip" class="note" style="font-size:14px">这是一段块级提示文字。</div>
```

## 2. `style` 规则

```markdown
<style>
.note {
  color: #444;
  background-color: #fff4d6;
  font-size: 14px;
  font-weight: bold;
}
#warn { color: #c00; font-weight: 700; }
.cover { width: 50%; }
</style>
```

- 选择器**只支持** `#id` 和 `.class`，且必须是单个标识符（字母、数字、`-`、`_`）。不支持标签选择器、后代选择器、逗号组合、`@` 规则。
- 规则只作用于本笔记里的 `img` / `div` / `span`。
- `<style>` 块本身在预览和 PDF / PNG 导出中都不显示。

### 允许的 CSS 声明

| 声明 | 取值 | 效果 |
| --- | --- | --- |
| `font-size` | `14px` / `120%` | 文字大小；百分比相对于正文字号 |
| `font-weight` | `normal`、`bold` 或 `100`–`900` 的整数 | 字重；`normal` = 400，`bold` = 700 |
| `color` | `#rgb` / `#rrggbb` | 文字颜色 |
| `background-color` | `#rgb` / `#rrggbb` | 文字背景色 |
| `width` | `300px` / `40%` | 元素宽度；百分比相对于内容区宽度 |
| `height` | `200px` / `30%` | 元素高度；百分比同样相对于内容区宽度 |

补充说明：

- 属性名大小写不敏感，连字符可省略，因此 `font-size`、`fontSize`、`FONTSIZE` 等价。
- 同时兼容历史笔误 `backgroudColor` / `backgroud-color`。
- `lighter` / `bolder` 依赖父元素继承值，受控子集**不支持**，会被忽略；请直接写具体数值。
- 颜色只接受十六进制字面量；`red`、`rgb(...)`、`hsl(...)` 等会被忽略，防止引入外部资源。
- 导出时字重会映射到 Typst 最接近的档位（例如 `700` → `bold`，`300` → `light`），预览与导出保持一致。

## 3. 样式优先级（低 → 高）

1. 默认外观
2. `<style>` 中的 `.class` 规则（同属性以后出现的规则为准；元素有多个 class 时按书写顺序叠加）
3. `<style>` 中的 `#id` 规则
4. 元素上的 `style="..."` 内联声明
5. `img` 的 `width` / `height` **属性**（覆盖样式表和内联声明里的同名尺寸）

也就是说，越贴近元素、越具体的写法优先级越高；`img width="200px"` 永远比 `.cover { width: 50% }` 更强。

## 4. 安全边界

- 标签、属性、CSS 声明三重白名单；未知内容一律忽略，不会导致渲染崩溃。
- `on*` 事件属性、`javascript:`、`url(` 以及 `@import` 等规则全部忽略。
- 不加载外部字体、不执行脚本、不发起网络请求；图片仍走墨笺现有的本地路径解析。
- 所有内容都来自你自己的笔记文件，离线可用。

## 5. 示例

### 示例一：给文字上色与加粗

```markdown
普通文字里嵌入 <span style="color:#0a84ff;font-weight:bold">蓝色加粗短语</span>，
以及 <span style="color:#c00">红色强调</span>。

<span style="font-weight:300">细体</span> ·
<span style="font-weight:500">中等</span> ·
<span style="font-weight:700">粗体</span>
```

### 示例二：用 class 做统一的提示框

```markdown
<style>
.note { color: #3a3a3a; background-color: #fff4d6; font-size: 14px; padding-ignored: 8px; }
.note b { color: red; }
.tip-title { font-weight: bold; color: #8a5a00; }
</style>

<div class="note" id="tip">
<span class="tip-title">提示：</span>这是一段**不会被解析成粗体**的说明文字。
</div>
```

说明：`.note b` 是后代选择器，会被忽略；`padding-ignored` 不在白名单里，也会被忽略。这正是受控子集的预期行为——写错不会破坏渲染，只是不生效。

### 示例三：图片尺寸与 id 覆盖

```markdown
<style>
.cover { width: 70%; }
#hero { width: 320px; }
</style>

<img class="cover" src="../assets/cover.jpg" alt="封面" />

<img id="hero" class="cover" src="../assets/hero.jpg" alt="主图" />

<!-- 属性优先级最高：即使 .thumb 写了 width，实际宽度仍是 96px -->
<style>.thumb { width: 50%; }</style>
<img class="thumb" src="../assets/avatar.png" alt="头像" width="96px" />
```

### 示例四：完整笔记

把下面这段保存为 `.md` 文件，用墨笺打开并 `Ctrl+T` 切换预览，再用「导出 PDF」检查，两处效果一致：

```markdown
# 旅行清单

<style>
.tag { color: #fff; background-color: #1296db; font-size: 12px; font-weight: bold; }
.desc { color: #555; font-size: 14px; }
#must { color: #c00; font-weight: 700; }
</style>

<img src="../assets/bag.jpg" alt="背包" width="240px" />

<div class="desc" id="must">必须带：证件、钥匙、充电宝。</div>

选 <span class="tag">轻量</span> 而不是 <span class="tag" style="background-color:#888">全副武装</span>。

这行中的 **Markdown 粗体** 仍然正常工作，因为它写在受控标签之外。
```

## 6. 常见问题

- **为什么我的 `<b>` / `<font color="red">` 没效果？** 它们不在白名单里。请用 `<span style="font-weight:bold;color:#c00">`。
- **为什么 `<div>` 里的 `**粗体**` 变成了星号？** 受控标签的内容按纯文本处理，避免嵌套 HTML 破坏安全边界。
- **为什么 PDF 里的字重和预览略有差别？** 两者使用同一份字重数值，但字体文件实际提供的字重档位不同，导出时会就近映射。
- **写错了会报错吗？** 不会。未知标签、未知属性、非法颜色与数值都会被静默忽略，方便你先写内容、后调样式。
