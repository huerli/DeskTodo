#!/usr/bin/env python3
"""生成 DeskTodo 应用图标（无需外部素材，纯程序化绘制）。

产出：src-tauri/icons/icon-source.png (1024x1024)，随后由
`pnpm tauri icon src-tauri/icons/icon-source.png` 派生各平台所需格式。
"""
from __future__ import annotations

import math
import os
import sys

from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
SS = 2  # 超采样倍数，先大后缩，边缘更干净


def rounded_mask(size: int, radius: int) -> Image.Image:
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return m


def vgradient(size: int, top: tuple[int, int, int], bottom: tuple[int, int, int]) -> Image.Image:
    g = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / max(size - 1, 1)
        g.putpixel(
            (0, y),
            (
                round(top[0] + (bottom[0] - top[0]) * t),
                round(top[1] + (bottom[1] - top[1]) * t),
                round(top[2] + (bottom[2] - top[2]) * t),
            ),
        )
    return g.resize((size, size), Image.NEAREST)


def main() -> int:
    w = SIZE * SS
    canvas = Image.new("RGBA", (w, w), (0, 0, 0, 0))

    # 1) 圆角底：深蓝 -> 靛蓝渐变
    bg = vgradient(w, (58, 92, 214), (28, 42, 110)).convert("RGBA")
    bg.putalpha(rounded_mask(w, int(w * 0.225)))
    canvas.alpha_composite(bg)

    # 2) 顶部高光，制造玻璃质感
    gloss = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    gd = ImageDraw.Draw(gloss)
    gd.ellipse((-w * 0.35, -w * 0.85, w * 1.35, w * 0.42), fill=(255, 255, 255, 46))
    gloss = gloss.filter(ImageFilter.GaussianBlur(w * 0.03))
    gloss.putalpha(Image.composite(gloss.getchannel("A"), Image.new("L", (w, w), 0), rounded_mask(w, int(w * 0.225))))
    canvas.alpha_composite(gloss)

    d = ImageDraw.Draw(canvas)

    # 3) 三条待办条目（左侧圆点 + 横线），最上面一条为“已完成”
    rows = [(0.300, True), (0.475, False), (0.650, False)]
    line_x0, line_x1 = w * 0.40, w * 0.735
    dot_x = w * 0.275
    dot_r = w * 0.031
    line_h = w * 0.042

    for cy_ratio, done in rows:
        cy = w * cy_ratio
        if done:
            d.ellipse((dot_x - dot_r, cy - dot_r, dot_x + dot_r, cy + dot_r), fill=(255, 255, 255, 255))
            # 勾
            d.line(
                [(dot_x - dot_r * 0.42, cy + dot_r * 0.02), (dot_x - dot_r * 0.08, cy + dot_r * 0.42), (dot_x + dot_r * 0.5, cy - dot_r * 0.45)],
                fill=(58, 92, 214, 255),
                width=max(2, int(w * 0.011)),
                joint="curve",
            )
            d.rounded_rectangle(
                (line_x0, cy - line_h / 2, line_x1, cy + line_h / 2),
                radius=line_h / 2,
                fill=(255, 255, 255, 150),
            )
        else:
            d.ellipse(
                (dot_x - dot_r, cy - dot_r, dot_x + dot_r, cy + dot_r),
                outline=(255, 255, 255, 235),
                width=max(2, int(w * 0.0105)),
            )
            d.rounded_rectangle(
                (line_x0, cy - line_h / 2, line_x1, cy + line_h / 2),
                radius=line_h / 2,
                fill=(255, 255, 255, 240),
            )

    # 4) 右下角提醒点（琥珀色），呼应“到期提醒”
    ax, ay, ar = w * 0.825, w * 0.825, w * 0.085
    d.ellipse((ax - ar, ay - ar, ax + ar, ay + ar), fill=(255, 255, 255, 255))
    ar2 = ar * 0.74
    d.ellipse((ax - ar2, ay - ar2, ax + ar2, ay + ar2), fill=(255, 178, 44, 255))
    # 时钟指针
    d.line([(ax, ay), (ax, ay - ar2 * 0.52)], fill=(58, 40, 10, 255), width=max(2, int(w * 0.0125)))
    d.line([(ax, ay), (ax + ar2 * 0.42, ay)], fill=(58, 40, 10, 255), width=max(2, int(w * 0.0125)))

    # 5) 缩回目标尺寸
    icon = canvas.resize((SIZE, SIZE), Image.LANCZOS)

    out_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src-tauri", "icons")
    out_dir = os.path.normpath(out_dir)
    os.makedirs(out_dir, exist_ok=True)
    out = os.path.join(out_dir, "icon-source.png")
    icon.save(out, "PNG")
    print(f"written: {out} ({icon.size[0]}x{icon.size[1]})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
