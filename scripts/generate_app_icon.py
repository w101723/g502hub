#!/usr/bin/env python3
"""生成 g502hub AppIcon：鼠标主体 + 清晰的电池充电徽章。"""
from pathlib import Path
import math
import struct
import zlib

SIZE = 1024
OUT = Path(__file__).resolve().parents[1] / "g502hub.app/Contents/Resources/AppIcon-1024.png"


def rounded_box(x, y, cx, cy, hx, hy, radius):
    qx = abs(x - cx) - (hx - radius)
    qy = abs(y - cy) - (hy - radius)
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - radius


def capsule(x, y, x1, y1, x2, y2, radius):
    vx, vy = x2 - x1, y2 - y1
    wx, wy = x - x1, y - y1
    vv = vx * vx + vy * vy
    t = max(0.0, min(1.0, (wx * vx + wy * vy) / vv))
    return math.hypot(x - (x1 + t * vx), y - (y1 + t * vy)) <= radius


def inside_polygon(x, y, points):
    inside = False
    j = len(points) - 1
    for i, (xi, yi) in enumerate(points):
        xj, yj = points[j]
        if (yi > y) != (yj > y):
            cross = (xj - xi) * (y - yi) / (yj - yi) + xi
            if x < cross:
                inside = not inside
        j = i
    return inside


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def write_png(path, width, height, rgba):
    raw = b"".join(b"\x00" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


pixels = bytearray(SIZE * SIZE * 4)
for y in range(SIZE):
    for x in range(SIZE):
        i = (y * SIZE + x) * 4
        bg = rounded_box(x + .5, y + .5, 512, 512, 458, 458, 210)
        if bg > 3:
            continue
        alpha = int(255 * max(0.0, min(1.0, (3 - bg) / 6)))
        t = y / (SIZE - 1)
        # 蓝黑渐变，保持与 macOS 深色系统图标协调。
        pixels[i:i+4] = bytes((int(34 - 13*t), int(69 - 22*t), int(101 - 20*t), alpha))

        # 白色鼠标主体，略向左上放置，给右下角电池徽章留空间。
        body_d = rounded_box(x, y, 438, 475, 220, 318, 198)
        body = abs(body_d) < 19
        split = capsule(x, y, 246, 373, 630, 373, 11)
        wheel_outline = abs(rounded_box(x, y, 438, 285, 31, 68, 27)) < 12
        wheel_mark = capsule(x, y, 438, 260, 438, 307, 8)
        if body or split or wheel_outline or wheel_mark:
            pixels[i:i+4] = bytes((246, 250, 255, 255))

        # 电池徽章底：与鼠标轮廓分离，避免形状粘连。
        badge = rounded_box(x, y, 690, 704, 214, 145, 72) <= 0
        if badge:
            pixels[i:i+4] = bytes((20, 39, 59, 255))

        # 电池清晰结构：白色外框 + 右侧正极 + 青绿色电量填充。
        batt_outer = abs(rounded_box(x, y, 675, 704, 132, 67, 28)) < 14
        terminal = rounded_box(x, y, 823, 704, 22, 31, 10) <= 0
        fill = rounded_box(x, y, 637, 704, 76, 43, 17) <= 0
        if batt_outer or terminal:
            pixels[i:i+4] = bytes((246, 250, 255, 255))
        if fill:
            pixels[i:i+4] = bytes((62, 222, 181, 255))

        # 电池内部独立闪电；四周至少留 18px，不与外框粘连。
        bolt_points = [(672, 663), (642, 708), (669, 708), (653, 745), (711, 692), (680, 692)]
        if inside_polygon(x, y, bolt_points):
            pixels[i:i+4] = bytes((246, 250, 255, 255))

write_png(OUT, SIZE, SIZE, bytes(pixels))
print(OUT)
