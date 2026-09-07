#!/usr/bin/env bash
# #228 boot-wedge defense: neuter the QMI path in cam_sensor_vsync_dev.ko.
#
# Why: cam_vsync_qmi_work hit a load-time race (list_add corruption →
# kernel BUG → cpu5 spin → radio insmod starved → boot stuck at modem,
# 2026-09-06, HARDWARE.md). We cannot just drop the two cam_sensor_vsync_*
# modules: cam_isp imports cam_notify_vsync_qmi from vsync_dev (verified
# 2026-09-07: cam_isp.ko depends= + sole referencing module), so they must
# stay loaded. Instead we ship a copy of the stock vendor ko with the two
# dangerous entries patched to immediate returns:
#
#   cam_notify_vsync_qmi  (+0x1ac in .text)  mov w0, #0 ; ret
#     - the ONLY queue_work_on site in the module lives here, so the work
#       (and its BUG) can never be queued again. cam_isp's call gets
#       return 0 = success, no CAM_ERR spam.
#   cam_vsync_qmi_work    (+0x2b8 in .text)  ret
#     - belt and braces: even a queued work item is a no-op.
#
# Symbol table, modversions CRCs and CFI metadata are untouched (function
# bodies only), so cam_isp's insmod dependency still resolves.
#
# Source ko is md5-pinned to the stock vendor_dlkm build (old repo
# boot/out/vendor-modules, matches /vendor_a on the device). Offsets are
# symbol-derived from that exact file — the asserts below refuse to patch
# anything else.
#
# Output: .local/modules.aginx/cam_sensor_vsync_dev.ko (gitignored blob;
# build-rootfs.sh copies it to /lib/modules.aginx in the image).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OLD="${OLD:-$HOME/Documents/aginxos}"
SRC="${OLD}/boot/out/vendor-modules/cam_sensor_vsync_dev.ko"
OUT="${ROOT}/.local/modules.aginx/cam_sensor_vsync_dev.ko"
STOCK_MD5="9f9a60fc26f47d9e1805c3713e49f82a"

test -f "$SRC" || { echo "missing $SRC (old repo vendor_dlkm unpack)" >&2; exit 1; }
got=$(md5 -q "$SRC")
if [ "$got" != "$STOCK_MD5" ]; then
  echo "stock ko md5 drift: got $got want $STOCK_MD5 — offsets are symbol-derived, refusing" >&2
  exit 1
fi

mkdir -p "${ROOT}/.local/modules.aginx"

python3 - "$SRC" "$OUT" <<'EOF'
import struct, sys
src, out = sys.argv[1], sys.argv[2]
d = bytearray(open(src, 'rb').read())
# .text section file offset (ELF section header parse, .text @ 0x1000 for
# this build — re-derived here so the assert below catches any drift).
e_shoff = struct.unpack_from('<Q', d, 0x28)[0]
esz = struct.unpack_from('<H', d, 0x3a)[0]
num = struct.unpack_from('<H', d, 0x3c)[0]
strndx = struct.unpack_from('<H', d, 0x3e)[0]
def sh(i):
    o = e_shoff + i * esz
    nm, _t, _f, _a, off, size = struct.unpack_from('<IIQQQQ', d, o)
    return nm, off, size
_, so, _ = sh(strndx)
text_off = None
for i in range(num):
    nm, off, size = sh(i)
    name = d[so + nm:d.index(b'\0', so + nm)].decode()
    if name == '.text':
        text_off = off
        break
assert text_off is not None, '.text not found'
MOV_W0_RET = struct.pack('<II', 0x52800000, 0xD65F03C0)  # mov w0,#0 ; ret
RET = struct.pack('<I', 0xD65F03C0)
notify = text_off + 0x1ac   # cam_notify_vsync_qmi
work = text_off + 0x2b8     # cam_vsync_qmi_work (local, BUG site)
# original prologues: SCS push (str x30,[x18],#8) / frame alloc (sub sp,#0x70)
assert d[notify:notify+4] == struct.pack('<I', 0xF800865E), 'notify prologue drift'
assert d[work:work+4] == struct.pack('<I', 0xD101C3FF), 'work prologue drift'
d[notify:notify+8] = MOV_W0_RET
d[work:work+4] = RET
open(out, 'wb').write(d)
print(f'patched: notify@{notify:#x} -> mov w0,#0; ret | work@{work:#x} -> ret')
EOF

echo "wrote $OUT"
echo "stock md5   : $STOCK_MD5"
echo "patched md5 : $(md5 -q "$OUT")"
