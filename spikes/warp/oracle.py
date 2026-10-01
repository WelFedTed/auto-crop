# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Dev-only OpenCV oracle for the warp spike (ROADMAP M0.36).

Reads out/src.raw, out/H.txt and the Rust outputs, runs cv2.warpPerspective with
Lanczos4 (OpenCV has no Lanczos3) and reports PSNR against each Rust result. This
only validates the geometry/coordinate convention; the kernel difference (a=4 vs
a=3) limits the PSNR. OpenCV is never a Rust dependency (cargo tree -i opencv is empty).
"""
import sys
import numpy as np
import cv2

SW, SH, DW, DH = 4000, 3000, 4000, 3000


def psnr(a, b):
    mse = np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2)
    return float("inf") if mse == 0 else 10 * np.log10(255.0 * 255.0 / mse)


src = np.fromfile("out/src.raw", dtype=np.uint8).reshape(SH, SW, 3)
h = np.array([float(v) for v in open("out/H.txt").read().split()]).reshape(3, 3)
res = {}
for name in ("ref", "own", "kornia"):
    res[name] = np.fromfile(f"out/{name}.raw", dtype=np.uint8).reshape(DH, DW, 3)

for label, flag in (("LANCZOS4", cv2.INTER_LANCZOS4), ("CUBIC", cv2.INTER_CUBIC), ("LINEAR", cv2.INTER_LINEAR)):
    cvout = cv2.warpPerspective(src, h, (DW, DH), flags=flag | cv2.WARP_INVERSE_MAP, borderMode=cv2.BORDER_REPLICATE)
    print(f"cv2 {label:9s} vs ref {psnr(cvout, res['ref']):6.2f} dB | vs own {psnr(cvout, res['own']):6.2f} dB | vs kornia {psnr(cvout, res['kornia']):6.2f} dB")
print("cv2", cv2.__version__)
