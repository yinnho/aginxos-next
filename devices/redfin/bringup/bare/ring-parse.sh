#!/bin/sh
# ring-parse.sh — L0 侧只读解析裸件日志环（S0-2，NOLINUX-PLAN 规格）。
#
# 用法（L0 上，root）: sh ring-parse.sh [ring_lba]   默认 25096
#   = boot_a start(8712)+16384；裸件侧由 GPT 名查动态定位，此处默认值
#     仅供人用——严谨对账以 GPT 实查为准（busybox dd if=/dev/sda bs=4096
#     skip=1 count=1 读头，33288=boot_b start 同法查）。
#
# 逐块判魔数 "AGXLOG\0\1"（41 47 58 4c 4f 47 00 01），命中块打
# slot/seq(hex LE)/stamp(hex LE)/level + 文本（+32 起，\0 截）。
# seq 十进制在文本 "#N" 前缀里（od 按 BE 进位，读不出 LE u64——
# 2026-10-15 实测定谳）。全 256 块扫完打印统计。
# 只读：dd 无 of=，零写盘。

BASE="${1:-25096}"
DEV="${DEV:-/dev/sda}"
TMP=/run/ring-blk.$$
MAGIC="4147584c4f470001"

hits=0
maxtext=""
i=0
while [ "$i" -lt 256 ]; do
  dd if="$DEV" bs=4096 skip=$((BASE + i)) count=1 2>/dev/null > "$TMP"
  m=$(dd if="$TMP" bs=1 count=8 2>/dev/null | od -An -tx1 | tr -d ' \n')
  if [ "$m" = "$MAGIC" ]; then
    hits=$((hits + 1))
    seqh=$(dd if="$TMP" bs=1 skip=8 count=8 2>/dev/null | od -An -tx1 | tr -d ' \n')
    sth=$(dd if="$TMP" bs=1 skip=16 count=8 2>/dev/null | od -An -tx1 | tr -d ' \n')
    lvl=$(dd if="$TMP" bs=1 skip=24 count=1 2>/dev/null | od -An -tu1 | tr -d ' ')
    txt=$(dd if="$TMP" bs=1 skip=32 count=96 2>/dev/null | tr '\0' '\n' | head -1)
    echo "slot=$i seq=$seqh stamp=$sth lvl=$lvl  $txt"
  fi
  i=$((i + 1))
done
rm -f "$TMP"
echo "--- $hits/256 blocks, ring @LBA $BASE"
