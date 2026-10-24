# claude — Claude Code 引擎 CLI（2026-10-24 装机）

上游：官方 npm 平台子包 `@anthropic-ai/claude-code-linux-arm64-musl`
（Bun 编译原生单文件，musl arm64，自带 JS runtime 不需要 Node）。

## 用法

    claude --version
    claude --print "一句话问题"     # 非交互单发
    claude                          # 交互 REPL（终端）

## brain 腿（关键配置）

走自家 brain 网关的 Anthropic 格式面，**不是 Anthropic 官方**：

    ANTHROPIC_BASE_URL=https://brain.aginx.net/anthropic
    ANTHROPIC_API_KEY=<brain key，/etc/aginx/env>

正式接线=agents 档案 claude 条目 `[command.env]`（codex/grok 同款
env 腿+HOME 家目录），wrapper 不写死任何环境。

## 同族

codex（OpenAI 格式 brain 腿）· grok（选装）。引擎=商品劳力，随用随换。
