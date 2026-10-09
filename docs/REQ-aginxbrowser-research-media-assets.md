# REQ：aginxbrowser 研究取材的媒体资产支持

日期：2026-10-08　提出方：aginxresearch（研究引擎，redfin 在役）
性质：需求反馈，非改动请求——本仓不碰 aginxbrowser 源码。

## 背景

aginxresearch 循环 = 取题 → aginxbrowser /search + /fetch 取材 → grok
综合 → SQLite 落条目。新要求：**研究过程中的图片/视频/音频/pdf 等
有价值文件必须保留**。实测中发现两条与该要求相关的缺口。

## 缺口 1（主要）：/fetch 正文把图片全剥掉了

`POST /fetch` 默认格式（Markdown/Text）返回 readability 抽取正文，
图片 URL 在源头就被剥离——研究引擎拿不到任何媒体线索。

实测（redfin，2026-10-08）：

```
POST /fetch {"url":"https://www.thepaper.cn"}            → 正文 0 个图片 URL
POST /fetch {"url":"https://www.thepaper.cn","format":"html"} → 58KB，含 36 个图片 URL
```

引擎目前的补法：对 top 页面**再发一次** `format:"html"` 请求，扫 DOM
属性里的媒体 URL。代价 = 每页两跳（搜索延迟 ~12s ×2），且 HTML 响应
体积远大于正文，白白占用取材预算。

**诉求**：/fetch 响应增加一个媒体清单字段（如 `media` 或 `images`）：
页面渲染后实际解析到的媒体资源 URL 列表——`img` 当前 src 现值（含
data-src 懒加载落位后的）、`video/audio` source、og:image，去重即可。
这样一跳同时拿到正文+媒体线索。备选：Markdown 格式保留 `![](url)`
行（正文内嵌图自然带出）。

## 缺口 2（次要，可选）：/download 落点只能由 daemon env 决定

`POST /download` 的文件落在 `AGINXBROWSER_DOWNLOAD_DIR`（daemon 全局），
请求里的 `filename` 只是文件名，调用方不能指定子目录。引擎现在下载后
再 rename 搬进实例目录——同文件系统没问题，跨 fs 会炸。

**诉求（可选）**：请求允许 `dir` 字段指定下载根下的相对子目录（内部
做路径清洗，不允许逃逸），省掉调用方搬运。

## 已在引擎侧消化（不阻塞，仅供知悉）

- 图标/头像降权（logo/icon/avatar/sprite/favicon 关键词 URL 排后）。
- 去重靠响应自带 sha256；超 asset_max_mb 的文件下载后删除。
- /download 对被墙域自动代理（should_auto_proxy）很好用，wikipedia
  类外站图片现在也能收——这条是表扬，不是诉求。

## 回执（2026-10-08，aginxbrowser #232 / commit 0c1c8f1）

两件都接了，本地已提交待发版：

1. **media 清单（主诉求）**：`/fetch` 响应新增 `media` 字段（URL 数组，
   去重、剔 `data:`、帽 500、空时不出现——老调用方无感）。HTTP 层静态
   扫 HTML 属性，浏览器层渲染后探针（img currentSrc 现值含懒加载落位
   + data-src/data-original/srcset 首选 + og:image + video/audio
   source/poster）。真机：thepaper.cn markdown 一跳 **19 条媒体 URL**
   （原 0），省掉 format:html 二跳。备选方案（markdown 保留 ![](url)）
   未采纳——会改所有现有消费方的正文形状。
2. **download dir（次诉求）**：请求可选 `dir` 字段=下载根下相对子目录，
   按需创建。绝对路径 / `..` / NUL 拒绝，不可逃逸根。不用再 rename 搬运。

发版后即可用；在那之前引擎侧可以先把二跳逻辑留着兼容旧版本。
