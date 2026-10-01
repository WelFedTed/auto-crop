# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Classical quad-detection baseline on the long-receipt set (ROADMAP M0.55). Throwaway spike.

Canny + contours + polygon approximation (the cheap classical path, no ML). Reports, per
aspect bucket, the failure rate (no quad found, or IoU against ground truth < 0.9) and the
mean corner error as a fraction of the image diagonal. Also writes a contact sheet of 20
spot-check images with ground truth (green) and prediction (red) drawn.

The ML part of the spike (DocQuadNet-256 at 256x256 and an elongated 512x128 variant) is
SKIPPED: the MakeACopy weights licence has no answer and no owner approval (M0.26), and the
weights are never committed.
"""
import itertools
import json
import math
import sys

import cv2
import numpy as np

OUT = "out"


def order_clockwise(pts):
    """Order 4 points clockwise starting at the point nearest the image top-left."""
    pts = np.asarray(pts, np.float32).reshape(4, 2)
    c = pts.mean(0)
    ang = np.arctan2(pts[:, 1] - c[1], pts[:, 0] - c[0])
    pts = pts[np.argsort(ang)]  # clockwise in image coordinates (y down)
    start = int(np.argmin(pts.sum(1)))
    return np.roll(pts, -start, axis=0)


def detect(img):
    h, w = img.shape[:2]
    scale = 1024.0 / max(h, w)
    small = cv2.resize(img, (int(w * scale), int(h * scale)), interpolation=cv2.INTER_AREA)
    gray = cv2.cvtColor(small, cv2.COLOR_BGR2GRAY)
    gray = cv2.GaussianBlur(gray, (5, 5), 0)
    med = float(np.median(gray))
    edges = cv2.Canny(gray, max(0, 0.66 * med), min(255, 1.33 * med))
    edges = cv2.dilate(edges, np.ones((3, 3), np.uint8), iterations=2)
    cnts, _ = cv2.findContours(edges, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    best, best_area = None, 0.0
    frame_area = small.shape[0] * small.shape[1]
    for c in cnts:
        area = cv2.contourArea(c)
        if area < 0.01 * frame_area or area > 0.98 * frame_area:
            continue
        peri = cv2.arcLength(c, True)
        approx = cv2.approxPolyDP(c, 0.02 * peri, True)
        if len(approx) == 4 and cv2.isContourConvex(approx):
            if area > best_area:
                best, best_area = approx.reshape(4, 2), area
    if best is None and cnts:  # fall back to the minimum-area rectangle of the largest contour
        c = max(cnts, key=cv2.contourArea)
        if cv2.contourArea(c) >= 0.01 * frame_area:
            best = cv2.boxPoints(cv2.minAreaRect(c))
    if best is None:
        return None
    return order_clockwise(best / scale)


def quad_iou(a, b):
    a = np.asarray(a, np.float32).reshape(4, 2)
    b = np.asarray(b, np.float32).reshape(4, 2)
    inter, _ = cv2.intersectConvexConvex(a, b)
    ua, ub = cv2.contourArea(a), cv2.contourArea(b)
    return float(inter / (ua + ub - inter)) if (ua + ub - inter) > 0 else 0.0


def corner_error(pred, gt):
    """Mean corner distance over the best cyclic alignment."""
    gt = order_clockwise(gt)
    best = min(np.linalg.norm(np.roll(pred, k, axis=0) - gt, axis=1).mean() for k in range(4))
    return float(best)


def main():
    labels = [json.loads(l) for l in open(f"{OUT}/labels.jsonl", encoding="utf-8")]
    rows = {b: [] for b in sorted({l["bucket"] for l in labels})}
    sheets = []
    for i, lab in enumerate(labels):
        img = cv2.imread(f"{OUT}/images/{lab['file']}")
        gt = np.array(lab["quad"], np.float32)
        pred = detect(img)
        diag = math.hypot(img.shape[1], img.shape[0])
        if pred is None:
            rows[lab["bucket"]].append((False, 0.0, float("nan")))
        else:
            iou = quad_iou(pred, gt)
            rows[lab["bucket"]].append((iou >= 0.9, iou, corner_error(pred, gt) / diag))
        if i % 25 == 0 and len(sheets) < 20:  # 20 spot checks across buckets
            vis = img.copy()
            cv2.polylines(vis, [gt.astype(np.int32)], True, (0, 255, 0), 3)
            if pred is not None:
                cv2.polylines(vis, [pred.astype(np.int32)], True, (0, 0, 255), 2)
            sheets.append(cv2.resize(vis, (400, 300)))
    print("bucket |  n | failures (no quad or IoU<0.9) | mean IoU | median corner err (% of diagonal)")
    allf = 0
    for b, r in rows.items():
        n = len(r)
        fails = sum(1 for ok, _, _ in r if not ok)
        allf += fails
        ious = [x[1] for x in r]
        errs = [x[2] for x in r if not math.isnan(x[2])]
        print(f"{b:>4}:1 | {n:3d} | {fails:3d} ({100 * fails / n:5.1f}%)               | {np.mean(ious):.3f}    | {100 * np.median(errs):.2f}%")
    print(f"total failures: {allf}/{len(labels)} ({100 * allf / len(labels):.1f}%)")
    cols = 5
    rows_img = []
    for r in range(0, len(sheets), cols):
        chunk = sheets[r:r + cols]
        while len(chunk) < cols:
            chunk.append(np.zeros_like(sheets[0]))
        rows_img.append(np.hstack(chunk))
    cv2.imwrite(f"{OUT}/spotcheck.jpg", np.vstack(rows_img))
    print(f"wrote {OUT}/spotcheck.jpg ({len(sheets)} images)")


if __name__ == "__main__":
    main()
