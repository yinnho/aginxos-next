#!/usr/bin/env bash
# panther-x2 fleet 复制线（路线 C·满配快照，2026-10-07 用户裁决）。
#
#   fleet.sh snapshot [--host ip]     从在役机抽满配快照 → .local/（无密可存可传）
#   fleet.sh kit [--host ip]          收身份三件套（env/auth/relay-secret）
#                                     → .local/…/kit/（0600，gitignored）
#   fleet.sh init --host ip --id 名 [--env-file F] [--auth-file F]
#                [--relay-secret-file F] [--pubkey F] [--tar F] [--reboot]
#                                     新板铺树+注身份+切 init=（可回滚；
#                                     身份三件不传参时自动取 kit/）
#   fleet.sh verify [--id] 名         在役验收：agc relay 两名册往返
#
# 「复制」的宪法分解：系统段（L0+包+名册+人格件）进快照；身份段
# （relay id/jwt/relay_secret、brain env、codex auth、authorized_keys）
# 永远逐台注入——零私数据烤入的 fleet 推论。快照因此可明文放 .local、
# 可 scp 过网。
#
# 前置（每台新板一次性，手动）：Mac 与板同网；ssh-copy-id root@<ip>
# （卖家系统口令见 #448 案卷，只内网不外传）——这同时保活回滚腿
# （卖家系统 ssh 永远可达）。
# 回滚：init 切换后随时可逆 = 恢复 armbianEnv 备份（板侧 /root/ 与
# Mac 侧 .local/…/backups/ 各一份）再 reboot，落回卖家系统。
set -euo pipefail

DEV_DIR="$(cd "$(dirname "$0")" && pwd)"
LOCAL="${DEV_DIR}/../../.local/device/panther-x2/fleet"
TPL="$DEV_DIR/fleet-config.tpl"
SSH_OPTS=(-o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new)

die() { echo "FATAL: $*" >&2; exit 1; }

# 排除表（设备侧 tar 时剥除）。挂载类 dir+dir/* 双写；「留壳剥肉」类
# （secret/spool/sessions…）只写 dir/*——目录本体留在快照里，新机树形
# 完整，init 不必补 mkdir。密钥身份类由 init 从文件逐台注入；运行态
# （spool/会话/sqlite/日志）新机自起自生。
EXCLUDES=(
  ./proc ./proc/* ./sys ./sys/* ./dev ./dev/* ./run ./run/* ./tmp ./tmp/*
  ./var/log/*
  ./etc/aginx/env
  ./etc/aginx/config.toml
  ./var/lib/aginx/secret/*
  ./var/lib/aginx/gateway/sessions.json
  ./var/lib/aginx/gateway/binding.json
  ./var/lib/aginx/gateway/spool/*
  ./var/lib/aginx/rtc-offset
  ./root/.ssh/*
  ./root/.codex.bak-*
  ./home/.codex/auth.json
  ./home/.codex/*.sqlite*
  ./home/.codex/sessions/*
  ./home/.codex/shell_snapshots/*
  ./home/.codex/tmp/* ./home/.codex/.tmp/*
  ./home/.codex/installation_id ./home/.codex/.sandbox_migration
  ./home/.codex/thread-writer-locks/*
)

# ---------------------------------------------------------------- snapshot
do_snapshot() {
  local host=192.168.2.2
  [ "${1:-}" = --host ] && host="${2:?}"
  mkdir -p "$LOCAL"
  local out
  out="$LOCAL/aginxos-full-$(date +%Y%m%d-%H%M).tar"
  local ex="" e
  for e in "${EXCLUDES[@]}"; do ex="$ex --exclude=$e"; done

  echo ">> tar from root@${host}（骑乘世界：根即树）"
  ssh "${SSH_OPTS[@]}" "root@$host" "cd / && tar cf -$ex ." > "$out"
  local size sum
  size=$(stat -f%z "$out")
  sum=$(shasum -a 256 "$out" | cut -d' ' -f1)
  [ "$size" -lt 500000000 ] || die "快照 $((size/1024/1024))MB 异常大——疑似排除失效，已废"
  echo ">> 快照 $out  $((size / 1024 / 1024))MB  sha256=$sum"

  # 验收：解包核结构 + 无密证明。密值经管道进 shell 变量，全程不回显。
  local tmp; tmp=$(mktemp -d)
  tar xf "$out" -C "$tmp"
  local p
  for p in ./init ./etc/aginx/device.toml ./etc/aginx/groups.desc \
           ./var/bin/codex ./var/bin/aginx \
           ./var/lib/aginx/units/aginx.toml \
           ./var/lib/aginx/pkgfiles/aginx \
           ./var/lib/aginx/gateway/agents/system \
           ./var/lib/aginx/skills/_system \
           ./home/.codex/config.toml ./home/.codex/skills \
           ./home/workflows ./home/AGENTS.md ./root/.codex \
           ./var/lib/aginx/secret ./var/lib/aginx/gateway/spool; do
    # -L 兜符号链接：/root/.codex 等指向设备绝对路径，Mac 解包必悬空。
    [ -e "$tmp/$p" ] || [ -L "$tmp/$p" ] || die "结构缺件: $p"
  done
  for p in ./proc ./sys ./dev ./run ./etc/aginx/env ./etc/aginx/config.toml \
           ./home/.codex/auth.json ./var/lib/aginx/gateway/sessions.json \
           ./var/lib/aginx/gateway/binding.json ./var/lib/aginx/rtc-offset; do
    [ ! -e "$tmp/$p" ] || die "排除失效（不该在快照里）: $p"
  done
  [ -z "$(ls -A "$tmp/root/.ssh" 2>/dev/null || true)" ] || die "root/.ssh 非空"
  [ -z "$(ls -A "$tmp/var/lib/aginx/secret" 2>/dev/null || true)" ] || die "secret 库非空"
  [ -z "$(ls -A "$tmp/var/lib/aginx/gateway/spool" 2>/dev/null || true)" ] || die "spool 非空"

  # 无密证明：拉在役三处密值（config 双 secret/env/auth.json），拆词后
  # grep 全树零命中才算过。拆词吃掉 JSON 引号与 env 的 KEY= 前缀，保
  # 证裸值也能命中；≥20 字符才当密值——真密（hex64/sk-…）都远长于此，
  # 键名（AGINXBRAIN_API_KEY 等 14–18 字）落在阈值下不误报（它们合法
  # 出现在 secret.policy 与 codex 技能文档里）。
  local v leak=0
  while IFS= read -r v; do
    [ "${#v}" -ge 20 ] || continue
    if grep -rFq -- "$v" "$tmp" 2>/dev/null; then
      echo "泄漏命中（值不回显）" >&2; leak=1
    fi
  done < <(ssh "${SSH_OPTS[@]}" "root@$host" \
    "sed -n 's/^relay_secret = //p;s/^jwt_secret = //p' /etc/aginx/config.toml | tr -d '\"'; \
     cat /etc/aginx/env /home/.codex/auth.json 2>/dev/null" \
    | tr ',{}"=' '\n' | tr -d '\r"')
  [ "$leak" = 0 ] || die "快照含密，已废（文件在 ${out}，人工核对后删）"
  rm -rf "$tmp"
  echo "PASS: 结构齐 + 无密证明过"

  {
    echo "date   = $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "source = root@$host (在役机)"
    echo "repo   = $(git -C "$DEV_DIR/../.." rev-parse --short HEAD)"
    echo "size   = $size"
    echo "sha256 = $sum"
    echo "excludes = ${EXCLUDES[*]}"
  } > "$out.manifest"
  echo ">> 清单 $out.manifest"
}

# ---------------------------------------------------------------- kit
do_kit() {
  local host=192.168.2.2
  [ "${1:-}" = --host ] && host="${2:?}"
  local kit="$LOCAL/kit"
  mkdir -p "$kit"
  # 三件全经 ssh 管道直落文件，不经过屏。relay-secret 只取值本身。
  ssh "${SSH_OPTS[@]}" "root@$host" 'cat /etc/aginx/env' > "$kit/env"
  ssh "${SSH_OPTS[@]}" "root@$host" 'cat /home/.codex/auth.json' > "$kit/auth.json"
  ssh "${SSH_OPTS[@]}" "root@$host" \
    "sed -n 's/^relay_secret = //p' /etc/aginx/config.toml | tr -d '\"'" > "$kit/relay-secret"
  chmod 600 "$kit/env" "$kit/auth.json" "$kit/relay-secret"
  local f n
  for f in env auth.json relay-secret; do
    n=$(stat -f%z "$kit/$f")
    [ "$n" -gt 0 ] || die "kit/$f 拉下来是空的"
    echo "kit/$f  ${n}B  (内容不回显)"
  done
}

# ---------------------------------------------------------------- init
do_init() {
  local host="" id="" envf="" authf="" secretf="" pubf="${HOME}/.ssh/id_ed25519.pub" tarf="" reboot=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --host) host="${2:?}"; shift 2 ;;
      --id) id="${2:?}"; shift 2 ;;
      --env-file) envf="${2:?}"; shift 2 ;;
      --auth-file) authf="${2:?}"; shift 2 ;;
      --relay-secret-file) secretf="${2:?}"; shift 2 ;;
      --pubkey) pubf="${2:?}"; shift 2 ;;
      --tar) tarf="${2:?}"; shift 2 ;;
      --reboot) reboot=1; shift ;;
      *) die "未知参数 $1" ;;
    esac
  done
  : "${host:?--host 必填}" "${id:?--id 必填}"
  # 身份三件缺参时自动取 kit/（fleet.sh kit 预收）。
  envf="${envf:-$LOCAL/kit/env}"
  authf="${authf:-$LOCAL/kit/auth.json}"
  secretf="${secretf:-$LOCAL/kit/relay-secret}"
  # relay 法：id 只许小写字母数字（同 crates/pair sanitize；连字符被拒）。
  [[ "$id" =~ ^[a-z0-9]+$ ]] || die "relay id 只许小写字母数字: $id"
  local f
  for f in "$envf" "$authf" "$secretf" "$pubf"; do
    [ -f "$f" ] || die "输入文件缺: $f（身份三件可先跑 fleet.sh kit 收）"
  done
  if [ -z "$tarf" ]; then
    tarf=$(ls -t "$LOCAL"/aginxos-full-*.tar 2>/dev/null | head -1)
  fi
  [ -n "$tarf" ] && [ -f "$tarf" ] || die "找不到快照（先跑 snapshot 或 --tar 指名）"
  local sum; sum=$(shasum -a 256 "$tarf" | cut -d' ' -f1)
  grep -q "sha256 = $sum" "$tarf.manifest" 2>/dev/null || die "快照 sha256 对不上 manifest"
  local S="ssh ${SSH_OPTS[*]} root@$host"

  echo ">> 预检 root@$host"
  $S 'cat /proc/device-tree/model 2>/dev/null' | grep -qi panther || die "机型不对（要 Panther）"
  if $S 'test -e /aginxos'; then die "板上已有 /aginxos——拒绝覆盖在役/半途机"; fi
  if $S "grep -q 'init=/aginxos/init' /boot/armbianEnv.txt 2>/dev/null"; then die "armbianEnv 已切 init=——拒绝重复 init"; fi
  $S 'test -f /boot/armbianEnv.txt' || die "形状意外：无 /boot/armbianEnv.txt"
  echo "   armbianEnv 现行: $($S 'grep extraargs /boot/armbianEnv.txt || true')"

  echo ">> 铺树（${tarf}）"
  scp -q "${SSH_OPTS[@]}" "$tarf" "root@$host:/root/fleet-snap.tar"
  $S 'tar xf /root/fleet-snap.tar -C /aginxos 2>/dev/null || { mkdir -p /aginxos && tar xf /root/fleet-snap.tar -C /aginxos; } && rm /root/fleet-snap.tar'
  $S 'test -x /aginxos/init && test -f /aginxos/home/.codex/config.toml' \
    || die "铺树后结构不对（init/config.toml 缺）"

  echo ">> 身份注入（id=${id}；密件不回显）"
  local cfg; cfg=$(mktemp)
  sed -e "s/__ID__/$id/g" \
      -e "s/__RELAY_SECRET__/$(tr -d '\n\r' < "$secretf")/g" \
      -e "s/__JWT_SECRET__/$(openssl rand -hex 32)/g" "$TPL" > "$cfg"
  chmod 600 "$cfg"
  scp -q "${SSH_OPTS[@]}" "$cfg" "root@$host:/aginxos/etc/aginx/config.toml"
  scp -q "${SSH_OPTS[@]}" "$envf" "root@$host:/aginxos/etc/aginx/env"
  scp -q "${SSH_OPTS[@]}" "$authf" "root@$host:/aginxos/home/.codex/auth.json"
  $S 'mkdir -p /aginxos/root/.ssh && chmod 700 /aginxos/root/.ssh'
  scp -q "${SSH_OPTS[@]}" "$pubf" "root@$host:/aginxos/root/.ssh/authorized_keys"
  $S 'chmod 600 /aginxos/etc/aginx/config.toml /aginxos/etc/aginx/env /aginxos/home/.codex/auth.json /aginxos/root/.ssh/authorized_keys'
  rm -f "$cfg"

  echo ">> 切 init=（备份：板侧 + Mac 侧）"
  local stamp; stamp=$(date +%Y%m%d%H%M)
  $S "cp /boot/armbianEnv.txt /root/armbianEnv.pre-fleet.$stamp"
  mkdir -p "$LOCAL/backups"
  scp -q "${SSH_OPTS[@]}" "root@$host:/root/armbianEnv.pre-fleet.$stamp" \
    "$LOCAL/backups/$host.armbianEnv.pre.$stamp"
  if $S 'grep -q "^extraargs=" /boot/armbianEnv.txt'; then
    $S "sed -i 's/^extraargs=\(.*\)\$/extraargs=\1 init=\/aginxos\/init/' /boot/armbianEnv.txt"
  else
    $S "echo 'extraargs=init=/aginxos/init' >> /boot/armbianEnv.txt"
  fi
  echo "   切后: $($S 'grep extraargs /boot/armbianEnv.txt')"

  if [ "$reboot" = 1 ]; then
    echo ">> reboot，等 90s 起机"
    $S 'reboot' || true
    sleep 90
    do_verify "$id"
  else
    echo ">> 就绪。下一步："
    echo "   ssh root@$host reboot   # 起机后验收："
    echo "   $0 verify --id $id"
    echo "   回滚：ssh root@$host 'cp /root/armbianEnv.pre-fleet.$stamp /boot/armbianEnv.txt && reboot'"
  fi
}

# ---------------------------------------------------------------- verify
do_verify() {
  local id=
  if [ "${1:-}" = --id ]; then id="${2:?}"; else id="${1:-}"; fi
  [ -n "$id" ] || die "verify 需 id（--id 名）"
  local i face out
  for face in system codex; do
    for i in 1 2 3; do
      if out=$(agc "agent://$id.relay.aginx.net/$face" '只用中文回答两个字：收到' 2>&1); then
        echo "PASS $face: $(echo "$out" | head -1)"
        break
      fi
      if [ "$i" = 3 ]; then echo "FAIL $face: $out" >&2; exit 1; fi
      echo "   $face 未应（$i/3），60s 后重试…"; sleep 60
    done
  done
  echo "全绿：$id 两名册 relay 往返过"
}

case "${1:-}" in
  snapshot) shift; do_snapshot "$@" ;;
  kit) shift; do_kit "$@" ;;
  init) shift; do_init "$@" ;;
  verify) shift; do_verify "$@" ;;
  *) die "用法：fleet.sh snapshot|kit|init|verify（头注释有全流程）" ;;
esac
