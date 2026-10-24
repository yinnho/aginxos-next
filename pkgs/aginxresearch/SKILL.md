# aginxresearch

深度研究引擎 v0.3.1（独立仓 ~/Documents/aginxresearch，AginxOS 只
装配）：取题 → aginxbrowser :8089 取材（聚合搜索 + 抓页，题面《篇名》
自动拉维基文库一手源；v0.6.2 起 /fetch 同趟带回 media 线索，附件收割
不再走 format:html 二跳）→ synth CLI 综合成带引用条目 → 写回实例
SQLite → 发现新题自续。图/视频/音频/pdf 收割保留（/download 带
dir=research/<实例名> 子目录 → rename 进 assets/ 入册）。

- **真身**：/var/lib/aginx/pkgfiles/aginxresearch/bin/aginxresearch；
  面 /var/bin/aginxresearch symlink（安装器自动带 .aginxmd）。
- **实例=系统工作文件**，唯一居所 `/home/research/<名>/`：四件套
  {program.md 研究意志, instance.conf, seed.txt, <名>.db} + assets/
  + harnesses/ + experience/（进化面）。实例不进任何仓、不上镜像；
  本包只装引擎本体，卸装不动实例。
- **CLI 八面**：`run`（--rounds N 引擎真多轮，v0.3.1 起优先于 conf）/
  `seed` / `status`（只读健康面）/ `golden`（金标题引导）/ `verify`
  （引用回验）/ `eval`（金卷测量，烧 synth 配额）/ `evolve` /
  `rollback`。
- **channel 三角色**（instance.conf）：`synth_cli` 研究 /
  `judge_cli` 评审 / `meta_cli` 进化提案，grok/claude/codex 三选；
  **judge≠synth 是宪法**（eval/evolve 入口校验）。grok 腿 spawn 自带
  HOME=/home 与 socks5 代理 env（10-10 裁决：装时就对，无软链桥）；
  无代理链的机器（X2）synth_cli 配 codex。
- **常驻**：research-loop 单元（pkg [service]，wrapper libexec/
  research-loop 轮转实例连磨，rc!=0 退 300s；日志 /home/research/
  logs/<名>.log）。试点实例不在 wrapper 名单内时不会自动跑——手动
  run 或改 wrapper 后 `aginx-svc restart`。
- **运行时依赖**（非 depends）：aginxbrowser :8089（search/fetch/
  download）+ 对应 channel CLI（/var/bin/ 下）。
- **进化回路（v0.3.x）**：evolve 一班=金标题基线→meta 提案（五文件
  白名单：program.md/instance.conf/terms_filter.txt/junk_rules.txt/
  requirements.md，预算键禁碰）→probe 同卷重测→硬门零回归+软面
  增益才采纳（限速 2/日，快照+harnesses/current 链）；正负结果同入
  experience/。`evolve --dry-run` 先看不动。

## 验证

- `readlink /var/bin/aginxresearch` 指向 pkgfiles 真身；
  `aginxresearch run /home/research/<名> --rounds 1` 落库（看
  research_archives 计数 +1；media 有货时 assets/ 落件+入册）。
- loop 火验：`aginx-svc status research-loop` ready +
  /home/research/logs/<名>.log 持续出回合行。

## 回滚

`aginx-pkg rollback aginxresearch`；实例数据不受包装卸影响。

## 噪声命中（v0.1.6+）

抓页前丢弃明显 junk（扫描页/微软帮助误召/验证标题），规则外置
junk_rules.txt 可进化。不替代 aginxbrowser 反爬。
