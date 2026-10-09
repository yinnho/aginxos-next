# morning-report

晨报 cron 腿 v0.1.0（#472 运维件归包）：busybox crond 08:00 拉起
codex（晨报官 AGENTS.md 工作目录，三段式：机身/业务/外面），
交付=微信端直推（channels weixin，bind_agent=codex）。

- **真身**：/var/lib/aginx/pkgfiles/morning-report/libexec/
  morning-codex.sh；面 /var/bin/morning-report symlink。
- **cron 块**：随包 [cron] 落位（`# aginx:cron=morning-report
  begin/end` 标记块，重装原地换块）；日志
  /home/workflows/morning-report/logs/crond.log。
- **产物核验=见文件不听汇报**：rc=0 也要见
  output/晨报-*.src.md 在 40 分钟窗内新鲜落盘，否则退出 1。
- **重火**：brain 流超时会整回合烧掉不落 -o——wrapper 自带一次
  120s 后重火（重火即活）。

## 验证

- `grep -A2 'aginx:cron=morning-report begin' /etc/crontabs/root`
  恰一块、行内容对。
- 手火：`/var/bin/morning-report`；看 output/ 新 .src.md +
  logs/codex-*.log 的 rc 链。

## 回滚

`aginx-pkg rollback morning-report`（cron 块留位——重装换块；
卸装不删块，手���删需连标记一起删）。
