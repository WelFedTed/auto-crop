# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Synthetic long-receipt generator (ROADMAP M0.54). Throwaway spike code.

Renders 500 photos of long, narrow receipts in four aspect buckets (4:1, 6:1, 8:1, 10:1,
125 each) with known ground-truth quads: random 3D pose (any in-plane rotation, tilt up to
30 degrees), cluttered or light backgrounds, noise, blur and JPEG q40-95. Seeded and
reproducible. Output: out/images/*.jpg and out/labels.jsonl (quad clockwise from the
top-left of the UPRIGHT receipt, in image pixels). This generator shares no code with the
app's own warp (decision B21 / PLAN 7.5).
"""
import json
import math
import os
import sys

import cv2
import numpy as np

CANVAS_W, CANVAS_H = 1600, 1200
RECEIPT_W = 320
BUCKETS = [4, 6, 8, 10]
PER_BUCKET = 125
OUT = "out"


def render_receipt(rng, aspect):
    h = int(round(aspect * RECEIPT_W * rng.uniform(0.96, 1.04)))
    paper = rng.integers(232, 256)
    tint = rng.integers(-6, 7, size=3)
    img = np.full((h, RECEIPT_W, 3), paper, np.uint8)
    img = np.clip(img.astype(int) + tint, 0, 255).astype(np.uint8)
    y = 24
    fonts = [cv2.FONT_HERSHEY_SIMPLEX, cv2.FONT_HERSHEY_DUPLEX, cv2.FONT_HERSHEY_PLAIN]
    ink = int(rng.integers(15, 90))  # darker = fresh print, lighter = faded thermal paper
    while y < h - 40:
        r = rng.random()
        if r < 0.08:
            cv2.line(img, (12, y), (RECEIPT_W - 12, y), (ink,) * 3, 1)
            y += 14
            continue
        if r < 0.12 and y < h - 120:  # barcode block
            x = 30
            while x < RECEIPT_W - 30:
                w = int(rng.integers(1, 4))
                cv2.rectangle(img, (x, y), (x + w, y + 46), (ink,) * 3, -1)
                x += w + int(rng.integers(1, 4))
            y += 62
            continue
        font = fonts[int(rng.integers(0, len(fonts)))]
        scale = float(rng.uniform(0.4, 0.62))
        left = "".join(rng.choice(list("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 "), size=int(rng.integers(6, 20))))
        right = "%d.%02d" % (rng.integers(0, 99), rng.integers(0, 99))
        cv2.putText(img, left, (10, y), font, scale, (ink,) * 3, 1, cv2.LINE_AA)
        (tw, _), _ = cv2.getTextSize(right, font, scale, 1)
        cv2.putText(img, right, (RECEIPT_W - 10 - tw, y), font, scale, (ink,) * 3, 1, cv2.LINE_AA)
        y += int(rng.integers(20, 28))
    return img


def make_background(rng):
    kind = rng.choice(["wood", "dark", "light", "noise", "cloth"], p=[0.25, 0.2, 0.15, 0.2, 0.2])
    base = rng.integers(20, 235, size=3).astype(float)
    if kind == "light":
        base = rng.integers(215, 250, size=3).astype(float)
    if kind == "dark":
        base = rng.integers(10, 70, size=3).astype(float)
    yy, xx = np.mgrid[0:CANVAS_H, 0:CANVAS_W].astype(np.float32)
    bg = np.zeros((CANVAS_H, CANVAS_W, 3), np.float32)
    bg[:] = base
    if kind == "wood":
        ang = rng.uniform(0, math.pi)
        stripes = np.sin((xx * math.cos(ang) + yy * math.sin(ang)) / rng.uniform(5, 14) + rng.uniform(0, 6)) * 14
        bg += stripes[..., None]
    elif kind == "cloth":
        bg += (np.sin(xx / 4.0) * np.sin(yy / 4.0) * 10)[..., None]
    noise = rng.normal(0, rng.uniform(3, 12), size=(CANVAS_H, CANVAS_W, 3)).astype(np.float32)
    bg += cv2.GaussianBlur(noise, (0, 0), rng.uniform(0.5, 3))
    # lighting gradient
    g = (xx / CANVAS_W * rng.uniform(-30, 30) + yy / CANVAS_H * rng.uniform(-30, 30))[..., None]
    return np.clip(bg + g, 0, 255)


def pose_quad(rng, aspect):
    """Project the receipt rectangle with a random 3D pose; fit to the canvas."""
    w, h = RECEIPT_W, aspect * RECEIPT_W
    corners = np.array([[-w / 2, -h / 2], [w / 2, -h / 2], [w / 2, h / 2], [-w / 2, h / 2]], dtype=np.float64)
    theta = rng.uniform(0, 2 * math.pi)
    pitch = math.radians(rng.uniform(-30, 30))
    yaw = math.radians(rng.uniform(-30, 30))
    rz = np.array([[math.cos(theta), -math.sin(theta), 0], [math.sin(theta), math.cos(theta), 0], [0, 0, 1]])
    rx = np.array([[1, 0, 0], [0, math.cos(pitch), -math.sin(pitch)], [0, math.sin(pitch), math.cos(pitch)]])
    ry = np.array([[math.cos(yaw), 0, math.sin(yaw)], [0, 1, 0], [-math.sin(yaw), 0, math.cos(yaw)]])
    r = rz @ rx @ ry
    pts = np.c_[corners, np.zeros(4)] @ r.T
    f, d = 1800.0, 2600.0
    proj = np.c_[f * pts[:, 0] / (pts[:, 2] + d), f * pts[:, 1] / (pts[:, 2] + d)]
    # fit to canvas with margin, then random shift keeping everything inside
    ext = proj.max(0) - proj.min(0)
    scale = min(0.92 * CANVAS_W / ext[0], 0.92 * CANVAS_H / ext[1]) * rng.uniform(0.55, 1.0)
    proj = (proj - proj.mean(0)) * scale
    lo, hi = proj.min(0), proj.max(0)
    max_dx, max_dy = CANVAS_W / 2 - 4 - hi[0], CANVAS_H / 2 - 4 - hi[1]
    min_dx, min_dy = -(CANVAS_W / 2 - 4) - lo[0], -(CANVAS_H / 2 - 4) - lo[1]
    shift = np.array([rng.uniform(min_dx, max_dx), rng.uniform(min_dy, max_dy)])
    return (proj + shift + np.array([CANVAS_W / 2, CANVAS_H / 2])).astype(np.float32)


def make_image(seed, aspect):
    rng = np.random.default_rng(seed)
    receipt = render_receipt(rng, aspect)
    bg = make_background(rng)
    quad = pose_quad(rng, receipt.shape[0] / RECEIPT_W)
    h, w = receipt.shape[:2]
    src = np.array([[0, 0], [w - 1, 0], [w - 1, h - 1], [0, h - 1]], np.float32)
    m = cv2.getPerspectiveTransform(src, quad)
    warped = cv2.warpPerspective(receipt, m, (CANVAS_W, CANVAS_H), flags=cv2.INTER_AREA)
    mask = cv2.warpPerspective(np.full((h, w), 255, np.uint8), m, (CANVAS_W, CANVAS_H), flags=cv2.INTER_LINEAR)
    # soft contact shadow
    shadow = cv2.GaussianBlur(mask, (0, 0), 8).astype(np.float32) / 255.0
    shadow = np.roll(shadow, (10, 10), axis=(0, 1))
    bg = bg * (1 - 0.25 * shadow[..., None])
    a = (mask.astype(np.float32) / 255.0)[..., None]
    img = warped.astype(np.float32) * a + bg * (1 - a)
    sigma = rng.uniform(0, 1.8)
    if sigma > 0.2:
        img = cv2.GaussianBlur(img, (0, 0), sigma)
    img += rng.normal(0, rng.uniform(1, 6), size=img.shape).astype(np.float32)
    img = np.clip(img, 0, 255).astype(np.uint8)
    q = int(rng.integers(40, 96))
    ok, buf = cv2.imencode(".jpg", img, [cv2.IMWRITE_JPEG_QUALITY, q])
    assert ok
    return buf.tobytes(), quad, q


def main():
    os.makedirs(f"{OUT}/images", exist_ok=True)
    n = 0
    with open(f"{OUT}/labels.jsonl", "w", encoding="utf-8", newline="\n") as lab:
        for bi, aspect in enumerate(BUCKETS):
            for k in range(PER_BUCKET):
                seed = 1_000_000 * (bi + 1) + k
                data, quad, q = make_image(seed, aspect)
                name = f"strip_{aspect}to1_{k:03d}.jpg"
                with open(f"{OUT}/images/{name}", "wb") as f:
                    f.write(data)
                lab.write(json.dumps({"file": name, "bucket": aspect, "seed": seed, "jpeg_q": q, "quad": quad.round(2).tolist()}) + "\n")
                n += 1
    print(f"wrote {n} images to {OUT}/images and {OUT}/labels.jsonl")


if __name__ == "__main__":
    main()
