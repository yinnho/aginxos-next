---
name: morning-report
description: 每日晨报——定时任务触发或用户要晨报/早报/今日新闻摘要时用
version: 1
tools:
  - web_search
  - web_fetch
  - file_write
max_iterations: 8
---

# 每日晨报

## 触发

- 定时任务触发（上下文有「定时任务触发」段）——job 已存在，不要反问
  要不要设定时任务、不要重复创建
- 用户消息要晨报/早报/今日新闻

## 取材

web_search 今日新闻（`fetch_top` 拿正文一步搜读），按需分域补搜：
时政/财经/科技/体育。今天是几号、星期几以设备日期为准。

## 成稿

- 首行 `# 晨报 · YYYY年M月D日 星期X`，农历有把握才带
- 分栏 `## 头条` `## 财经` `## 体育`（有大事才立栏）+ 收尾
  `## 今日看点`
- 要点用列表，一条一句事实，共 12~25 条；不编造、没核实的标注存疑

## 归档（到此为止）

全文存 `/home/workflows/morning-report/output/晨报-YYYY年M月D日.src.md`
（file_write 的 path 一字不差用这个绝对路径——写相对 `output/` 会被
发送者域收编到别处）。
**归档=markdown 落盘即完成。禁止导 docx/pptx/pdf——设备没有
pandoc，document_generate 在设备上不可用，试了必失败还污染回复。**

## 交付（回复即成品）

- **回复正文=完整晨报全文**，首行就是 `# 晨报 · …`——定时任务的
  delivery 会把回复原文包进 morning 卡片上屏，用户问询时回复也
  直接是正文
- 禁止回复「以下是您的晨报」包裹语、道歉语、给用户的操作建议；
  归档失败之类的过程话不上回复
- 正文里不出现任务 ID、技术细节
