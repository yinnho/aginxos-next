#!/bin/sh
# run.sh — 后台腿：锁 → codex 生成 → 验 → 烟测上屏 → 出卡（完成通知即预览）。
# 由 gen.sh setsid 甩出，stdout/stderr 全在 /run/template-gen/<id>-<ts>.log。
# busybox 铁律：无 awk/netstat/curl/wget；nc 必须 stdin 保持（尾部 sleep）。
set -u

ID="${1:-}"
DESC="${2:-}"
TDIR=/var/lib/aginxbrowser/templates
CARDS=/home/cards

say() { echo "[$(date +%H:%M:%S)] $*"; }

# 同 id 防重入：mkdir 原子锁（并发第二次直接退，registry tmp 不打架）
LOCK="/run/template-gen/$ID.lock"
mkdir "$LOCK" 2>/dev/null || { say "模板 $ID 已有生成任务在跑，退出"; exit 0; }
trap 'rmdir "$LOCK" 2>/dev/null' EXIT

# 卡片写出（heredoc 不解释转义，\\n 字面落 JSON 正好是换行转义）
card() {
  CS=$(date +%Y%m%d-%H%M%S)
  cat > "$CARDS/.${CS}-template-$ID.json" <<EOF
{
  "title": "$1",
  "template": "$2",
  "data": { "title": "$3", "body": "$4" },
  "source": "template-gen"
}
EOF
  mv "$CARDS/.${CS}-template-$ID.json" "$CARDS/${CS}-template-$ID.json"
  say "出卡：$CARDS/${CS}-template-$ID.json"
}

PROMPT="任务：给 aginxbrowser 生成新模板 $ID。目录 $TDIR。先读 reply.html、note.html、todo.html 学纪律：磷光终端配色变量（--bg #060a07 --accent #3dfd8f 一族）、等宽字体栈、body min-height 2620px、离线无外部资源、markdown 正文用模板内 JS 渲染。槽位 {{title}} {{body}}。风格要求：$DESC。registry.json 用同目录临时副本逐字节校验后 mv 原子更新，加入 $ID 条目（matches 放中文同义词和 $ID）。完成后报告两个文件的路径和字节数。"

say "codex 开跑：$ID —— $DESC"
cd /tmp || exit 1
CODEX_HOME=/home/.codex /var/bin/codex exec --skip-git-repo-check --sandbox danger-full-access "$PROMPT"
RC=$?
say "codex 退出码 $RC"
if [ "$RC" -ne 0 ]; then
  card "模板 $ID 生成失败" reply "生成失败" "## codex 退出码 $RC\\n\\n日志在 /run/template-gen/ 下，让母体看一眼。"
  exit 1
fi

# 验工件：页 + 登记
if [ ! -s "$TDIR/$ID.html" ]; then
  card "模板 $ID 生成失败" reply "生成失败" "## $TDIR/$ID.html 没落盘\\n\\ncodex 报成功但文件不在，日志见 /run/template-gen/。"
  exit 1
fi
if ! grep -q "\"$ID\"" "$TDIR/registry.json"; then
  card "模板 $ID 生成失败" reply "生成失败" "## registry.json 没登记 $ID\\n\\n页在登记缺，日志见 /run/template-gen/。"
  exit 1
fi
say "工件齐：$TDIR/$ID.html + registry 已登记"

# 烟测上屏（nc stdin 保持铁律；200/ok=true 过，unknown_template=登记坏了）
printf '{"template":"%s","data":{"title":"新模板 %s","body":"## 已就绪\\n\\ncodex 生成完毕，本页即预览。"}}' "$ID" "$ID" > /tmp/tg-smoke.json
SZ=$(wc -c < /tmp/tg-smoke.json)
{ printf 'POST /open HTTP/1.0\r\nContent-Type: application/json\r\nContent-Length: %s\r\nConnection: close\r\n\r\n' "$SZ"
  cat /tmp/tg-smoke.json
  sleep 3
} | timeout 8 busybox nc 127.0.0.1 8089 > /tmp/tg-smoke.resp 2>&1
if grep -q 'unknown_template' /tmp/tg-smoke.resp 2>/dev/null; then
  card "模板 $ID 生成失败" reply "生成失败" "## 烟测 404 unknown_template\\n\\nregistry 登记没生效，日志见 /run/template-gen/。"
  exit 1
fi
grep -q '"ok":true' /tmp/tg-smoke.resp 2>/dev/null && say "烟测 200，已上屏" || say "烟测响应异常（文件已验，放行）"

card "新模板「$ID」就绪" "$ID" "模板 $ID" "## codex 已生成\\n\\n- 点开即预览\\n- registry 已登记\\n- 风格：$DESC"
say "全部完成"
