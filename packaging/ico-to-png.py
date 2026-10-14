#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Extracts the largest PNG image of an .ico file (the repository has only crates/shell/icons/icon.ico,
whose single image is a 32x32 placeholder) so the macOS and Linux packages can build their own icon
formats from it. Standard library only. Usage: ico-to-png.py <icon.ico> <out.png>"""
import struct
import sys

data = open(sys.argv[1], "rb").read()
count = struct.unpack("<H", data[4:6])[0]
best = None
for i in range(count):
    w, _h, _c, _r, _p, _b, size, off = struct.unpack("<BBBBHHII", data[6 + 16 * i : 22 + 16 * i])
    png = data[off : off + size]
    if png[:8] == b"\x89PNG\r\n\x1a\n" and (best is None or (w or 256) > best[0]):
        best = (w or 256, png)
if best is None:
    sys.exit("no PNG image inside the .ico")
open(sys.argv[2], "wb").write(best[1])
print(f"wrote a {best[0]}x{best[0]} PNG to {sys.argv[2]}")
