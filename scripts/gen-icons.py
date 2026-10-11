#!/usr/bin/env python3
"""生成客户端占位图标（纯标准库，不依赖 Pillow）。

真实品牌图标应由设计产出后直接覆盖 `client/src-tauri/icons/` 下的同名文件。
这个脚本存在的唯一理由：CI / 全新克隆里没有图标时，tauri-build 在 Windows 上
会因为缺 `icons/icon.ico` 直接编译失败（`bundle.icon` 为空也一样，因为 Windows
资源编译是硬需求）。所以把它留在仓库里，而不是让人手搓二进制。

用法：
    python scripts/gen-icons.py [--out client/src-tauri/icons]
"""

from __future__ import annotations

import argparse
import struct
import zlib
from pathlib import Path

# 马赛克：2×2 色块，左下角切掉一小块做"缺角"识别点。
TILES = [
    (0x2D, 0xD4, 0xBF),  # teal
    (0xFB, 0xBF, 0x24),  # amber
    (0x8B, 0x5C, 0xF6),  # violet
    (0xF4, 0x3F, 0x5E),  # rose
]
BG = (0x1E, 0x24, 0x33, 0xFF)


def pixel(x: int, y: int, size: int) -> tuple[int, int, int, int]:
    """按归一化坐标画 2×2 色块。"""
    half = size / 2
    # 外边距 12%，让图标在小尺寸下不至于顶满
    m = size * 0.12
    inner = size - 2 * m
    if x < m or y < m or x >= size - m or y >= size - m:
        return BG
    u = (x - m) / inner
    v = (y - m) / inner
    col = 0 if u < 0.5 else 1
    row = 0 if v < 0.5 else 1
    # 缺角：右下角一小块留背景色
    if col == 1 and row == 1 and u > 0.78 and v > 0.78:
        return BG
    r, g, b = TILES[row * 2 + col]
    return (r, g, b, 0xFF)


def png(size: int) -> bytes:
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # filter type 0
        for x in range(size):
            raw.extend(pixel(x, y, size))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def ico_bmp(size: int) -> bytes:
    """ICO 里的 BMP 条目：BITMAPINFOHEADER + BGRA（自下而上）+ AND 掩码。"""
    header = struct.pack(
        "<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0
    )
    body = bytearray()
    for y in range(size - 1, -1, -1):
        for x in range(size):
            r, g, b, a = pixel(x, y, size)
            body.extend((b, g, r, a))
    mask_row = ((size + 31) // 32) * 4
    body.extend(b"\x00" * (mask_row * size))
    return header + bytes(body)


def ico(entries: list[tuple[int, bytes]]) -> bytes:
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    for size, data in entries:
        w = 0 if size >= 256 else size
        out.extend(
            struct.pack("<BBBBHHII", w, w, 0, 0, 1, 32, len(data), offset)
        )
        offset += len(data)
    for _, data in entries:
        out.extend(data)
    return bytes(out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="client/src-tauri/icons")
    ns = ap.parse_args()

    out = Path(ns.out)
    out.mkdir(parents=True, exist_ok=True)

    # Windows 资源编译必需
    (out / "icon.ico").write_bytes(
        ico([(16, ico_bmp(16)), (32, ico_bmp(32)), (48, ico_bmp(48)), (256, png(256))])
    )
    # Linux / macOS 打包用
    for size in (32, 128, 256):
        (out / f"{size}x{size}.png").write_bytes(png(size))
    (out / f"{128}x{128}@2x.png").write_bytes(png(256))
    (out / "icon.png").write_bytes(png(512))

    print(f"图标已生成到 {out}")


if __name__ == "__main__":
    main()
