#!/usr/bin/env python3
"""从 icon-source.png 生成各平台图标，不依赖 `tauri icon`。

产出（写入 src-tauri/icons/）：
  32x32.png, 128x128.png, 128x128@2x.png, icon.png (512), StoreLogo.png
  Windows: icon.ico (16/24/32/48/64/128/256)
  macOS:   icon.icns (16..1024)

用法： python3 scripts/gen_icons.py
"""
from __future__ import annotations

import os
import struct
import sys

from PIL import Image

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
ICONS = os.path.join(ROOT, "src-tauri", "icons")
SRC = os.path.join(ICONS, "icon-source.png")

PNG_SIZES = [32, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
# ICNS 类型码 -> 像素尺寸
ICNS_TYPES = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
]


def load_source() -> Image.Image:
    if not os.path.exists(SRC):
        print(f"缺少源图 {SRC}，请先运行 scripts/make_icon.py", file=sys.stderr)
        raise SystemExit(1)
    img = Image.open(SRC).convert("RGBA")
    if img.size != (1024, 1024):
        img = img.resize((1024, 1024), Image.LANCZOS)
    return img


def scaled(img: Image.Image, size: int) -> Image.Image:
    return img.resize((size, size), Image.LANCZOS)


def write_png(img: Image.Image, path: str) -> None:
    img.save(path, "PNG", optimize=True)


def build_ico(img: Image.Image, path: str) -> None:
    frames = [scaled(img, s) for s in ICO_SIZES]
    blobs = []
    for f in frames:
        import io

        buf = io.BytesIO()
        f.save(buf, "PNG", optimize=True)
        blobs.append(buf.getvalue())

    header = struct.pack("<HHH", 0, 1, len(frames))
    offset = len(header) + 16 * len(frames)
    entries = b""
    for f, blob in zip(frames, blobs):
        w = 0 if f.size[0] >= 256 else f.size[0]
        h = 0 if f.size[1] >= 256 else f.size[1]
        entries += struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    with open(path, "wb") as fh:
        fh.write(header + entries + b"".join(blobs))


def build_icns(img: Image.Image, path: str) -> None:
    import io

    body = b""
    for code, size in ICNS_TYPES:
        buf = io.BytesIO()
        scaled(img, size).save(buf, "PNG", optimize=True)
        blob = buf.getvalue()
        body += code + struct.pack(">I", len(blob) + 8) + blob
    with open(path, "wb") as fh:
        fh.write(b"icns" + struct.pack(">I", len(body) + 8) + body)


def main() -> int:
    os.makedirs(ICONS, exist_ok=True)
    img = load_source()

    for size in PNG_SIZES:
        name = f"{size}x{size}.png"
        write_png(scaled(img, size), os.path.join(ICONS, name))
    write_png(scaled(img, 256), os.path.join(ICONS, "128x128@2x.png"))
    write_png(scaled(img, 512), os.path.join(ICONS, "icon.png"))
    write_png(scaled(img, 128), os.path.join(ICONS, "StoreLogo.png"))
    # Windows Store 图标集（tauri 会引用其中部分）
    for size in (30, 44, 71, 89, 107, 142, 150, 284, 310):
        write_png(scaled(img, size), os.path.join(ICONS, f"Square{size}x{size}Logo.png"))

    build_ico(img, os.path.join(ICONS, "icon.ico"))
    build_icns(img, os.path.join(ICONS, "icon.icns"))

    for f in sorted(os.listdir(ICONS)):
        p = os.path.join(ICONS, f)
        print(f"  {f:28s} {os.path.getsize(p):>9,d} B")
    return 0


if __name__ == "__main__":
    sys.exit(main())
