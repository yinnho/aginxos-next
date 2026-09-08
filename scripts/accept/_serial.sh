# _serial.sh — accept 套件的 serial 单一来源（被 source，不直接执行）。
#
# 优先级：ADB_SERIAL（env，最高）> devices/${ACCEPT_DEVICE}/device.toml
# 的 [adb] serial（D14：机型事实住档案——套件里不再硬编 serial 字面量，
# 第二台机上工作台时同一套件换 ACCEPT_DEVICE 即指向新机）。
# 套件在 source 前设 ACCEPT_DEVICE=<codename>；两级都空即死——钉死
# 纪律不放松（多机在台时防 adb 打错目标）。
#
# 取值 sed 用 [^"]*：贪婪 .* 会吞 TOML 行内注释（BSD sed 实测）。
ACCEPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[ -n "${ACCEPT_DEVICE:-}" ] || { echo "accept: ACCEPT_DEVICE 未设（source _serial.sh 前）" >&2; exit 1; }
PROFILE="${ACCEPT_ROOT}/devices/${ACCEPT_DEVICE}/device.toml"
PROFILE_SERIAL="$(sed -n 's/^serial *= *"\([^"]*\)".*/\1/p' "${PROFILE}" | head -1)"
[ -n "${PROFILE_SERIAL}" ] || { echo "accept: ${PROFILE} 无 [adb] serial" >&2; exit 1; }
SERIAL="${ADB_SERIAL:-${PROFILE_SERIAL}}"
