#!/usr/bin/env python3
"""gen-fixtures.py — host-only OCR fixture 生成器（S3 双栏页）。

page-synthetic/cam-screen 等首批 fixture 是老仓手工产物，原样收编；
此脚本只生成合成新形状。macOS 系统字体，PIL 仅 host 工具链依赖。

用法: python3 tools/ocr/gen-fixtures.py  （幂等，覆盖写）
"""

import sys

# macOSSystem CJK 字体候选（PingFang 主，Hiragino 兜底）
FONTS = [
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
]


def load_font(size):
    from PIL import ImageFont

    for p in FONTS:
        try:
            return ImageFont.truetype(p, size)
        except OSError:
            continue
    sys.exit(f"no CJK font found in {FONTS}")


def two_columns(out):
    """双栏页：通栏标题 + 左「甲」右「乙」各三行 + 中间空沟。

    读序期望（order_columns 切开后）：标题 → 甲一/二/三 → 乙一/二/三。
    未切时行分组只认 y：甲一·乙一同行交错，甲/乙 交替——正是要修的错。
    """
    from PIL import Image, ImageDraw

    W, H = 1000, 700
    img = Image.new("RGB", (W, H), "white")
    d = ImageDraw.Draw(img)
    title_f = load_font(56)
    body_f = load_font(40)

    d.text((W // 2, 50), "双栏标题页", font=title_f, fill="black", anchor="ma")

    left_lines = ["甲栏第一行", "甲栏第二行", "甲栏第三行"]
    right_lines = ["乙栏第一行", "乙栏第二行", "乙栏第三行"]
    y0, dy = 180, 120
    for i, s in enumerate(left_lines):
        d.text((90, y0 + i * dy), s, font=body_f, fill="black")
    for i, s in enumerate(right_lines):
        d.text((590, y0 + i * dy), s, font=body_f, fill="black")
    # 沟 = x≈400..590（左栏行宽 ~240，右栏起 590）

    img.save(out, "JPEG", quality=92)
    print(f"{out}: {W}x{H}")


if __name__ == "__main__":
    here = __import__("pathlib").Path(__file__).parent
    two_columns(here / "fixtures" / "page-twocol.jpg")
