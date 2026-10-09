# aginxresearch

深度研究引擎 v0.1.1（独立仓 ~/Documents/aginxresearch，AginxOS 只
装配）：取题 → aginxbrowser :8089 聚合搜索/抓页取材（题面《篇名》
自动拉维基文库一手源，wikisource/wikipedia 域走 use_proxy）→ grok
综合成带引用条目 → 写回实例 SQLite → 发现新题自续。研究过程中的
图/视频/音频/pdf 一律收割保留（assets/ 落盘+入册）。

- **真身**：/var/lib/aginx/pkgfiles/aginxresearch/bin/aginxresearch；
  面 /var/bin/aginxresearch symlink（安装器自动带 .aginxmd）。
- **实例=系统工作文件**，唯一居所 `/home/research/<名>/`：四件套
  {program.md 研究意志, instance.conf, seed.txt, <名>.db} + assets/。
  实例不进任何仓、不上镜像；本包只装引擎本体，卸装不动实例。
- **CLI**：`aginxresearch run <实例目录>`（一回合；`seed <目录>` 灌
  种子题）。
- **常驻（v0.1.1 起随包）**：research-loop 单元（pkg [service]，落
  /var/lib/aginx/units/aginxresearch.toml）——树内 wrapper
  libexec/research-loop 轮转 duanju/taishigong/qujiu/baize 连磨
  （10-09 裁决：串行永不闲，rc!=0 退 300s 再战），日志
  /home/research/logs/<名>.log。旧 crond 错峰四腿已退役。
- **运行时依赖**（非 depends）：aginxbrowser（:8089 三 API）+
  /var/bin/grok（引擎 spawn 时自带 socks5h 代理 env；grok 家
  /root/.grok→/home/.grok 符号链接，loop 裸环境靠 wrapper 的
  HOME=/root 落对）。

## 验证

- `readlink /var/bin/aginxresearch` 指向 pkgfiles 真身；
  `aginxresearch run /home/research/<名>` 一回合落库（看
  research_archives 计数 +1）。
- loop 火验：`aginx-svc status research-loop` ready +
  /home/research/logs/<名>.log 持续出 `=== (loop) ===` 回合行。

## 回滚

`aginx-pkg rollback aginxresearch`；实例数据不受包装卸影响。
