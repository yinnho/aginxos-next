# aginxresearch

深度研究引擎 v0.1.0（独立仓 ~/Documents/aginxresearch，AginxOS 只
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
- **常驻**：crond 错峰腿（每 3h 各一题，duanju :07 / taishigong :22 /
  qujiu :37 / baize :52），包装脚本 /usr/libexec/aginx/research-round.sh
  （钉 HOME=/root+PATH、pidof 防撞、日志 /home/research/logs/）。
  **改 crontabs 后必须 `touch /etc/crontabs`**（busybox crond 只认
  目录 mtime，改文件不重扫=新条目永不火）。
- **运行时依赖**（非 depends）：aginxbrowser（:8089 三 API）+
  /var/bin/grok（引擎 spawn 时自带 socks5h 代理 env；grok 家
  /root/.grok→/home/.grok 符号链接，cron 裸环境靠 wrapper 的
  HOME=/root 落对）。

## 验证

- `readlink /var/bin/aginxresearch` 指向 pkgfiles 真身；
  `aginxresearch run /home/research/<名>` 一回合落库（看
  research_archives 计数 +1）。
- cron 火验：/home/research/logs/<名>.log 出现对应整点 `===` 回合行。

## 回滚

`aginx-pkg rollback aginxresearch`；实例数据不受包装卸影响。
