# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Offline oracle generator for crates/imgproc (ROADMAP M1.23, M1.25).

Writes small, deterministic JSON fixtures into crates/imgproc/tests/fixtures/. The Rust tests read
the fixtures; this script never runs in CI (a dev-only oracle, like the OpenCV note in ADR-0002).

    pip install -r tools/imgproc-oracles/requirements.txt
    python tools/imgproc-oracles/gen_oracles.py

Fixtures (all seeded; re-running reproduces them byte for byte for the same numpy/OpenCV):

* homography_cv2.json     cv2.getPerspectiveTransform on float32-exact quads (M1.23 oracle).
* warp_numpy_lanczos3.json  a NumPy float64 Lanczos3 reference warp (exact weights, no lookup
                            table): sub-pixel shifts, a perspective map, Gray8 / RGB8 / RGB16
                            (M1.25 "within 1 LSB").
* warp_cv2_lanczos4.json  cv2.warpPerspective(INTER_LANCZOS4 | WARP_INVERSE_MAP) outputs for the
                            PSNR check (OpenCV has no Lanczos3, so the kernel differs; geometry and
                            pixel-centre convention must agree).
* zone_plate_reference.json is not generated: the zone plate is built in Rust.

Convention (same as the Rust kernel): pixel (i, j) has its centre at (i, j); the matrix maps output
pixel centres to source pixel centres; a sample is "outside" when x < -0.5, x >= w - 0.5 (same for
y), and outside pixels are zero; inside, taps beyond the edge clamp to the edge pixel.
"""
from __future__ import annotations

import json
import math
import pathlib

import cv2
import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "imgproc" / "tests" / "fixtures"


def hexs(a: np.ndarray) -> str:
    """Little-endian bytes of the array as lowercase hex."""
    return np.ascontiguousarray(a).astype(a.dtype.newbyteorder("<")).tobytes().hex()


def lanczos3(x: np.ndarray) -> np.ndarray:
    x = np.asarray(x, dtype=np.float64)
    out = np.zeros_like(x)
    nz = np.abs(x) < 3
    px = math.pi * x[nz]
    with np.errstate(divide="ignore", invalid="ignore"):
        v = 3.0 * np.sin(px) * np.sin(px / 3.0) / (px * px)
    v[x[nz] == 0] = 1.0
    out[nz] = v
    # whole pixels other than 0 are exactly zero
    out[(x == np.round(x)) & (x != 0)] = 0.0
    return out


def warp_lanczos3(src: np.ndarray, m: np.ndarray, out_w: int, out_h: int, maxval: int) -> np.ndarray:
    """Float64 reference warp. src is (h, w, c); returns (out_h, out_w, c) of src's dtype."""
    h, w, c = src.shape
    vv, uu = np.meshgrid(np.arange(out_h, dtype=np.float64), np.arange(out_w, dtype=np.float64), indexing="ij")
    d = m[2, 0] * uu + m[2, 1] * vv + m[2, 2]
    x = (m[0, 0] * uu + m[0, 1] * vv + m[0, 2]) / d
    y = (m[1, 0] * uu + m[1, 1] * vv + m[1, 2]) / d
    inside = (x >= -0.5) & (x < w - 0.5) & (y >= -0.5) & (y < h - 0.5)
    x0 = np.floor(x).astype(np.int64)
    y0 = np.floor(y).astype(np.int64)
    fx, fy = x - x0, y - y0
    wx = np.stack([lanczos3(fx - (k - 2)) for k in range(6)], axis=-1)
    wy = np.stack([lanczos3(fy - (k - 2)) for k in range(6)], axis=-1)
    wx /= wx.sum(axis=-1, keepdims=True)
    wy /= wy.sum(axis=-1, keepdims=True)
    acc = np.zeros((out_h, out_w, c), dtype=np.float64)
    srcf = src.astype(np.float64)
    for ky in range(6):
        yi = np.clip(y0 + ky - 2, 0, h - 1)
        for kx in range(6):
            xi = np.clip(x0 + kx - 2, 0, w - 1)
            acc += (wy[..., ky] * wx[..., kx])[..., None] * srcf[yi, xi]
    res = np.clip(np.floor(acc + 0.5), 0, maxval)
    res[~inside] = 0
    return res.astype(src.dtype)


def blur(img: np.ndarray, sigma: float) -> np.ndarray:
    """Separable Gaussian blur with edge replication (stand-in for lens blur on a real photo)."""
    r = int(math.ceil(3 * sigma))
    k = np.exp(-0.5 * (np.arange(-r, r + 1) / sigma) ** 2)
    k /= k.sum()
    out = img
    for axis in (0, 1):
        pad = [(0, 0)] * img.ndim
        pad[axis] = (r, r)
        padded = np.pad(out, pad, mode="edge")
        out = sum(k[i] * np.take(padded, range(i, i + img.shape[axis]), axis=axis) for i in range(2 * r + 1))
    return out


def texture(rng: np.random.Generator, w: int, h: int, c: int, maxval: int, sigma: float = 0.0) -> np.ndarray:
    """A document-like test image: soft gradient, dark text-like bars, mild noise. `sigma` > 0 blurs
    the bars the way optics blur a real photo (hard 1 px edges are the worst case for any resampler)."""
    yy, xx = np.mgrid[0:h, 0:w]
    base = 0.62 + 0.25 * (xx / w) + 0.08 * (yy / h)
    img = np.repeat(base[..., None], c, axis=-1)
    for _ in range(max(6, w * h // 90)):
        bw, bh = int(rng.integers(3, 18)), int(rng.integers(2, 6))
        x0, y0 = int(rng.integers(0, max(1, w - bw))), int(rng.integers(0, max(1, h - bh)))
        img[y0 : y0 + bh, x0 : x0 + bw, :] = rng.uniform(0.08, 0.35)
    if sigma > 0:
        img = blur(img, sigma)
    img += rng.normal(0, 0.012, img.shape)
    return np.clip(np.round(img * maxval), 0, maxval).astype(np.uint8 if maxval == 255 else np.uint16)


def homography_fixtures() -> dict:
    rng = np.random.default_rng(20261001)
    cases = []
    for i in range(48):
        # Source: a jittered quadrilateral in a 4000 x 3000 frame (clockwise from top-left).
        # Coordinates are multiples of 1/4, exactly representable in float32 (cv2 takes float32).
        cx, cy = rng.uniform(1200, 2800), rng.uniform(900, 2100)
        r = rng.uniform(500, 1100)
        aspect = rng.choice([1.0, 1.414, 0.7, 3.0, 8.0]) if i % 3 == 0 else rng.uniform(0.6, 1.8)
        ang = rng.uniform(-0.5, 0.5)
        base = np.array([[-1, -1], [1, -1], [1, 1], [-1, 1]], dtype=np.float64) * [r * aspect ** 0.5, r / aspect ** 0.5]
        rot = np.array([[math.cos(ang), -math.sin(ang)], [math.sin(ang), math.cos(ang)]])
        src = base @ rot.T + [cx, cy] + rng.normal(0, 0.06 * r, (4, 2))
        src = np.round(src * 4) / 4
        ow, oh = float(np.round(rng.uniform(300, 3000))), float(np.round(rng.uniform(300, 3000)))
        dst = np.array([[0, 0], [ow, 0], [ow, oh], [0, oh]], dtype=np.float64)
        if i % 4 == 3:  # a general quad as the target too
            dst = np.round((dst + rng.normal(0, 30, (4, 2))) * 4) / 4
        h = cv2.getPerspectiveTransform(src.astype(np.float32), dst.astype(np.float32))
        h = h / h[2, 2]
        cases.append({"src": src.reshape(-1).tolist(), "dst": dst.reshape(-1).tolist(), "h": h.reshape(-1).tolist()})
    return {"oracle": f"cv2 {cv2.__version__} getPerspectiveTransform", "cases": cases}


def case(name: str, src: np.ndarray, m: np.ndarray, out_w: int, out_h: int, maxval: int, expected: np.ndarray) -> dict:
    h, w, c = src.shape
    return {
        "name": name,
        "w": w,
        "h": h,
        "channels": c,
        "bits": 8 if maxval == 255 else 16,
        "src": hexs(src),
        "matrix": m.reshape(-1).tolist(),
        "out_w": out_w,
        "out_h": out_h,
        "expected": hexs(expected),
    }


def numpy_fixtures() -> dict:
    rng = np.random.default_rng(7)
    cases = []
    rgb8 = texture(rng, 40, 30, 3, 255)
    gray8 = texture(rng, 36, 28, 1, 255)
    rgb16 = texture(rng, 32, 24, 3, 65535)
    shifts = [(0.25, 0.0), (0.5, 0.5), (0.123, -0.371), (-0.8, 0.4), (1.0 / 3.0, 2.0 / 3.0), (0.999, 0.001)]
    for src, tag, mv in ((rgb8, "rgb8", 255), (gray8, "gray8", 255), (rgb16, "rgb16", 65535)):
        h, w, _ = src.shape
        for dx, dy in shifts:
            m = np.array([[1, 0, dx], [0, 1, dy], [0, 0, 1.0]])
            exp = warp_lanczos3(src, m, w, h, mv)
            cases.append(case(f"shift_{tag}_{dx:+.3f}_{dy:+.3f}", src, m, w, h, mv, exp))
        # A perspective map (tilted quad -> upright rectangle), output smaller than the source.
        quad = np.array([[3.2, 2.1], [w - 4.7, 4.4], [w - 2.5, h - 3.3], [5.1, h - 1.9]], dtype=np.float32)
        ow, oh = int(w * 0.8), int(h * 0.9)
        dst = np.array([[0, 0], [ow - 1, 0], [ow - 1, oh - 1], [0, oh - 1]], dtype=np.float32)
        m = cv2.getPerspectiveTransform(dst, quad)  # dst -> src
        exp = warp_lanczos3(src, m, ow, oh, mv)
        cases.append(case(f"perspective_{tag}", src, m, ow, oh, mv, exp))
        # Scale by 0.8 (mild minification) and a 3-degree rotation about the centre.
        s = 0.8
        m = np.array([[s, 0, 0.5 * (1 - s) * (w - 1)], [0, s, 0.5 * (1 - s) * (h - 1)], [0, 0, 1.0]])
        cases.append(case(f"scale08_{tag}", src, m, w, h, mv, warp_lanczos3(src, m, w, h, mv)))
        a = math.radians(3.0)
        cx, cy = (w - 1) / 2, (h - 1) / 2
        m = np.array([[math.cos(a), -math.sin(a), cx - math.cos(a) * cx + math.sin(a) * cy],
                      [math.sin(a), math.cos(a), cy - math.sin(a) * cx - math.cos(a) * cy],
                      [0, 0, 1.0]])
        cases.append(case(f"rot3deg_{tag}", src, m, w, h, mv, warp_lanczos3(src, m, w, h, mv)))
    # Self-check: identity is exact.
    ident = warp_lanczos3(rgb8, np.eye(3), 40, 30, 255)
    assert np.array_equal(ident, rgb8), "numpy reference must reproduce the source at integer positions"
    return {"oracle": f"numpy {np.__version__} float64 Lanczos3 (exact weights)", "cases": cases}


def cv2_fixtures() -> dict:
    rng = np.random.default_rng(11)
    cases = []
    # (width, height, output width, output height, blur sigma in pixels). Sigma 0 is the worst case
    # (every bar edge is one pixel sharp); sigma 1 is closer to a real photo of a document.
    for w, h, ow, oh, sigma in [(160, 120, 140, 100, 0.0), (192, 144, 170, 128, 1.0), (176, 132, 150, 118, 1.5)]:
        src = texture(rng, w, h, 3, 255, sigma)
        quad = np.array([[0.06 * w, 0.10 * h], [0.93 * w, 0.05 * h], [0.97 * w, 0.92 * h], [0.03 * w, 0.95 * h]], dtype=np.float32)
        dst = np.array([[0, 0], [ow - 1, 0], [ow - 1, oh - 1], [0, oh - 1]], dtype=np.float32)
        m = cv2.getPerspectiveTransform(dst, quad)  # dst -> src
        exp = cv2.warpPerspective(
            src, m, (ow, oh), flags=cv2.INTER_LANCZOS4 | cv2.WARP_INVERSE_MAP, borderMode=cv2.BORDER_CONSTANT, borderValue=0
        )
        cases.append(case(f"cv2_lanczos4_blur{sigma:g}_{w}x{h}_to_{ow}x{oh}", src, m, ow, oh, 255, exp))
    return {"oracle": f"cv2 {cv2.__version__} warpPerspective INTER_LANCZOS4 | WARP_INVERSE_MAP", "cases": cases}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, data in (
        ("homography_cv2.json", homography_fixtures()),
        ("warp_numpy_lanczos3.json", numpy_fixtures()),
        ("warp_cv2_lanczos4.json", cv2_fixtures()),
    ):
        path = OUT / name
        path.write_text(json.dumps(data, separators=(",", ":")) + "\n", encoding="utf-8")
        print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes, {len(data['cases'])} cases)")


if __name__ == "__main__":
    main()
