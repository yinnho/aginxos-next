---
name: jq
description: jq JSON 处理器——信封解码/结构化读取一刀（PATH 面 CLI）
---

jq v1.8.1（Alpine v3.22 整树）。常用姿势：

```sh
jq -r '.data.value' <in.json>          # 解值（relay 信封=sqlite3 之外的第二条解码路）
jq -r '.content' envelope.json | head  # 信封 content 原文
jq '.files[] | .path' manifest.json    # manifest 清单
```

机器上常与 sqlite3 配对：sqlite3 管 DB，jq 管 JSON。
