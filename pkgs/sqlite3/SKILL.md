# sqlite3 — SQLite 3.54.0 CLI（amalgamation 全静态单件）

## 形态

官方 amalgamation（sqlite.org `sqlite-amalgamation-3540000.zip`，尺寸
2990479B + sha256 钉死）→ zig cc `aarch64-linux-musl -O2 -s` 全静态单件，
`/var/bin/sqlite3`。零依赖闭包、零 loader、零 wrapper——比 git 包的
apk 整树路线轻一档（静态 musl，内核直接 exec）。PATH 面 CLI（D12
agent-CLI 口味，git/codex/grok 同族），不注册 aginx 命令面。

## 用法（研究库只读查询）

```sh
sqlite3 --version
# 只读打开（WAL 库并发读者安全；引擎在写也不挡查询）：
sqlite3 'file:/home/research/qujiu/qujiu.db?mode=ro' \
  'SELECT count(*), max(created_at) FROM research_archives;'
# 题目库存（pending=待磨）：
sqlite3 'file:/home/research/qujiu/qujiu.db?mode=ro' \
  "SELECT status, count(*) FROM research_questions GROUP BY status;"
```

- 只读 URI（`?mode=ro`）是默认姿势——写操作归引擎本体（aginxresearch），
  CLI 别对活库下手（journal 文件归引擎管）。
- 交互 REPL 无 readline（`-DHAVE_READLINE=0`，静态代价）——管道/单命令
  口味为主；`.mode box` `.headers on` 照常可用。
- 非 root shell 注意 /var/bin 不在默认 PATH（登录 ssh 在；cron/引擎子进程
  用绝对路径 /var/bin/sqlite3）。

## 验证（上机）

```sh
sqlite3 --version        # 3.54.0 2026-10-09 ...
sqlite3 'file:/home/research/qujiu/qujiu.db?mode=ro' 'SELECT count(*) FROM research_archives;'
```

## 回滚

`aginx-pkg rollback sqlite3`（或重装旧 tar）。单件无侧效，删净即回。
