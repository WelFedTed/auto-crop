# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Decode the network output into a page quad plus confidence features.

The centre peak gives one quad hypothesis (centre + regressed corner offsets). Each regressed
corner is then snapped to the nearest corner-heatmap peak (3x3 weighted centroid, sub-pixel) when
there is one close enough; a corner with no peak keeps its regressed position (an occluded or
out-of-frame corner) and is counted as unmatched, which lowers the confidence.
"""

from __future__ import annotations

import math

import cv2
import numpy as np

from common import canonical_quad, is_convex, poly_area

OFFSET_UNIT = 32.0


def _sig(x):
    return 1.0 / (1.0 + np.exp(-x))


def _peaks(heat: np.ndarray, thr: float = 0.05, k: int = 24):
    h, w = heat.shape
    pad = np.pad(heat, 1, constant_values=0)
    mx = np.max([pad[dy : dy + h, dx : dx + w] for dy in range(3) for dx in range(3)], axis=0)
    ys, xs = np.nonzero((heat >= mx) & (heat > thr))
    order = np.argsort(-heat[ys, xs])[:k]
    out = []
    for i in order:
        y, x = int(ys[i]), int(xs[i])
        y0, y1, x0, x1 = max(0, y - 1), min(h, y + 2), max(0, x - 1), min(w, x + 2)
        win = heat[y0:y1, x0:x1].astype(np.float64)
        gy, gx = np.mgrid[y0:y1, x0:x1]
        sw = win.sum()
        out.append((float((gx * win).sum() / sw), float((gy * win).sum() / sw), float(heat[y, x])))
    return out


def decode(out: np.ndarray, stride: int = 4, match_radius_cells: float = 3.0) -> dict | None:
    """``out``: (11, S, S) raw network output. Returns canvas-pixel quad and features, or None."""
    heat, mask, ctr = _sig(out[0]), _sig(out[1]), _sig(out[2])
    s = heat.shape[0]
    cy, cx = np.unravel_index(int(np.argmax(ctr)), ctr.shape)
    pc = float(ctr[cy, cx])
    cell = np.array([(cx + 0.5) * stride, (cy + 0.5) * stride])
    reg = cell + out[3:11, cy, cx].reshape(4, 2) * OFFSET_UNIT
    peaks = _peaks(heat)
    diag = float(np.linalg.norm(reg.max(axis=0) - reg.min(axis=0))) + 1e-6
    radius = max(match_radius_cells * stride, 0.12 * diag)
    used: set[int] = set()
    quad = reg.copy()
    peak_scores = []
    for k in range(4):
        best, bi = None, -1
        for i, (px, py, sc) in enumerate(peaks):
            if i in used:
                continue
            pos = np.array([(px + 0.5) * stride, (py + 0.5) * stride])
            d = float(np.linalg.norm(pos - reg[k]))
            if d <= radius and sc >= 0.1:
                cost = d / radius - 0.5 * sc
                if best is None or cost < best[0]:
                    best, bi = (cost, pos, sc, d), i
        if best is not None:
            used.add(bi)
            quad[k] = best[1]
            peak_scores.append(best[2])
        else:
            peak_scores.append(0.0)
    peak_scores = np.array(peak_scores)
    matched = peak_scores > 0
    if not is_convex(quad) or abs(poly_area(quad)) < 4.0:
        if is_convex(reg) and abs(poly_area(reg)) >= 4.0:
            quad, matched, peak_scores = reg.copy(), np.zeros(4, bool), np.zeros(4)
        else:
            return None
    quad = canonical_quad(quad)
    # agreement between the regressed hypothesis and the snapped corners (as a share of the diagonal)
    dist = float(np.mean(np.linalg.norm(reg - np.array([quad[int(np.argmin(np.linalg.norm(quad - r, axis=1)))] for r in reg]), axis=1))) / diag
    # mask agreement
    big = np.zeros((s * 4, s * 4), np.uint8)
    cv2.fillPoly(big, [np.round(quad / stride * 4).astype(np.int32)], 1)
    poly_m = cv2.resize(big.astype(np.float32), (s, s), interpolation=cv2.INTER_AREA)
    mk = (mask > 0.5).astype(np.float32)
    inter = float((poly_m * mk).sum())
    union = float(poly_m.sum() + mk.sum() - inter)
    mask_iou = inter / union if union > 0 else 0.0
    feats = {
        "min_peak": float(peak_scores.min()),
        "mean_peak": float(peak_scores.mean()),
        "n_matched": int(matched.sum()),
        "ctr_peak": pc,
        "mask_iou": mask_iou,
        "agree": dist,
        "n_peaks": len(peaks),
    }
    return {"quad": quad, "feats": feats}


FEATURE_NAMES = ["min_peak", "mean_peak", "n_matched", "ctr_peak", "mask_iou", "agree"]


def feature_vector(f: dict) -> np.ndarray:
    return np.array(
        [f["min_peak"], f["mean_peak"], f["n_matched"] / 4.0, f["ctr_peak"], f["mask_iou"], math.exp(-8.0 * f["agree"])],
        dtype=np.float64,
    )
