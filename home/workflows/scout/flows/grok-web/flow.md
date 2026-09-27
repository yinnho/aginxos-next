---
name: grok-web
description: 派 grok 走隧道查墙外网页/X 数据——任务进来先异步派工秒回，回头问结果时取果交原文
version: 2
tools:
  - shell_exec
  - file_read
  - file_list
shell_allow:
  - "sh *flows/grok-web/scripts/*"
max_iterations: 6
---

# grok×browser 取材（异步派工）

## 派工（第一段，秒回）

```
sh /home/workflows/scout/flows/grok-web/scripts/ask.sh '<完整任务一句话：查什么、哪些页面/账号、要哪些字段>'
```

- 立即返回。回话：已派 grok 取材，约 1–4 分钟，回来问即取
- 任务必须写全（grok 只看到这一句）

## 取果（第二段，回来问时）

- `file_list` 看 `/home/workflows/scout/output/`（**一律绝对路径**）：只有
  `.md.body` 没有最终 `.md` = grok 还在跑——直接回「还没好，一两分钟
  后再问」，别自己查、别动 web 工具
- 有 `.md` 时 `file_read` 读**时间戳最新**的那个，先看头部「退出码」行：
  0=成，原文照交（字段名保留原文，别改写）；124=超 280s 被掐，建议
  拆小重派；1=grok 本体错（详情 `/run/grok-web/last.err`，宿主侧可查）
- 交回原文数据，不加工成稿——成稿归母体
- 两段之间不轮询、不反复 shell 查文件

## 不要

- 不加 env（脚本已带隧道+NO_PROXY 铁律，漏 NO_PROXY 回环 MCP 就死）
- 不把墙外域名塞给自己的 web 工具重试——直连必死
- 不同步等 grok 跑完再回话——relay turn gate 110s 撑不下
