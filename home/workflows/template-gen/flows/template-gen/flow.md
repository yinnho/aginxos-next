---
name: template-gen
description: 生成并登记 aginxbrowser 显示模板——用户要新模板、卡片 404 unknown_template、模板不存在打不开时用
version: 1
tools:
  - shell_exec
  - file_read
shell_allow:
  - "sh *flows/template-gen/scripts/*"
max_iterations: 6
---

# 模板生成（codex 派工）

显示模板住 `/var/lib/aginxbrowser/templates/`（registry.json 登记表 +
`<id>.html` 页）。生成一律派给 codex，**不要自己手写 HTML**。

## 触发场景

1. 用户对话要新模板（「做个 XX 模板」「想要 XX 样式的页」）
2. 卡片点不开，term 报来 404 unknown_template——消息里带模板名

## 派工（唯一动作）

```
sh /home/workflows/template-gen/flows/template-gen/scripts/gen.sh <模板id> <一句话风格/用途描述>
```

- 模板 id：小写英文（如 note / todo / poem），不是中文
- 槽位默认 `{{title}}` `{{body}}`；body 是 markdown，模板内 JS 渲染
- gen.sh **立即返回**（codex 后台跑约 5 分钟），完成后自动写一张用
  新模板渲染的卡片到 `/home/cards/`——卡片就是完成通知，点开即预览
- 派工后回复用户：已派 codex 生成模板 X，完成出新卡

## 等待期不要

- 不要轮询文件、不要反复 shell——完成后卡片自己出现
- 不要 file_write 模板 HTML 或 registry.json（原子性归脚本管）

## 失败处理

- gen.sh 说 codex 未装 → 回复用户 `aginx-pkg opt-in codex`
- 超过 10 分钟没出新卡 → 读 `/run/template-gen/<id>-*.log` 报原因
