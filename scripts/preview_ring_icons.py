#!/usr/bin/env python3
"""生成 macOS Template 单色圆环在浅色/深色菜单栏上的预览。"""
from pathlib import Path
import math
import struct
import zlib

S = 36
SCALE = 5
GAP = 18
STATES = [
    ("offline", None, False),
    ("10", 10, False),
    ("65", 65, False),
    ("100", 100, False),
    ("65 charge", 65, True),
]
IW = S * SCALE
W = len(STATES) * IW + (len(STATES) + 1) * GAP
H = 2 * IW + 3 * GAP + 44
canvas = bytearray(W * H * 4)


def point_poly(x, y, points):
    inside = False
    j = len(points) - 1
    for i, (xi, yi) in enumerate(points):
        xj, yj = points[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def icon(percent, charging):
    out = bytearray(S * S * 4)
    connected = percent is not None
    level = (percent or 0) / 100
    tau = math.tau
    radius = 14
    progress_angle = tau * level
    end_x = 18 + radius * math.sin(progress_angle)
    end_y = 18 - radius * math.cos(progress_angle)
    bolt_points = [
        (18.5, 9.8),
        (13.8, 18.2),
        (17.2, 18.2),
        (15.7, 26.1),
        (22.4, 16.1),
        (18.8, 16.1),
    ]

    for y in range(S):
        for x in range(S):
            alpha_sum = 0
            for sy in range(4):
                for sx in range(4):
                    px = x + (sx + 0.5) / 4
                    py = y + (sy + 0.5) / 4
                    dx = px - 18
                    dy = py - 18
                    distance = math.hypot(dx, dy)
                    angle = (math.atan2(dy, dx) + math.pi / 2) % tau
                    track = abs(distance - radius) < 0.75
                    arc = (
                        connected
                        and level > 0
                        and abs(distance - radius) < 2.25
                        and (
                            angle <= progress_angle
                            or math.hypot(dx, dy + radius) < 2.25
                            or math.hypot(px - end_x, py - end_y) < 2.25
                        )
                    )
                    slash = not connected and abs(px - py) < 1.25 and distance < radius - 2.2
                    bolt = charging and point_poly(px, py, bolt_points)
                    alpha_sum += 255 if arc or slash or bolt else 68 if track else 0
            out[(y * S + x) * 4 + 3] = min(255, alpha_sum // 16)
    return out


def fill_rect(y0, y1, color):
    for y in range(y0, y1):
        for x in range(W):
            q = (y * W + x) * 4
            canvas[q : q + 4] = bytes((*color, 255))


def composite(template, background, foreground, ox, oy):
    for y in range(S):
        for x in range(S):
            alpha = template[(y * S + x) * 4 + 3] / 255
            color = tuple(
                int(background[channel] * (1 - alpha) + foreground[channel] * alpha)
                for channel in range(3)
            )
            for yy in range(SCALE):
                for xx in range(SCALE):
                    q = ((oy + y * SCALE + yy) * W + ox + x * SCALE + xx) * 4
                    canvas[q : q + 4] = bytes((*color, 255))


light_bg = (235, 235, 237)
dark_bg = (38, 38, 41)
fill_rect(0, H, light_bg)
dark_y = IW + 2 * GAP + 22
fill_rect(dark_y, H, dark_bg)

for row, (background, foreground) in enumerate(
    [(light_bg, (35, 35, 35)), (dark_bg, (245, 245, 245))]
):
    oy = GAP + row * (IW + GAP + 22)
    for index, (_, percent, charging) in enumerate(STATES):
        ox = GAP + index * (IW + GAP)
        composite(icon(percent, charging), background, foreground, ox, oy)


def chunk(kind, data):
    return (
        struct.pack(">I", len(data))
        + kind
        + data
        + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    )


raw = b"".join(b"\0" + canvas[y * W * 4 : (y + 1) * W * 4] for y in range(H))
png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(raw, 9))
    + chunk(b"IEND", b"")
)
out = Path("/tmp/g502hub-template-ring-preview.png")
out.write_bytes(png)
print(out)
