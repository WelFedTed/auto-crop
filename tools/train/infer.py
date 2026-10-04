# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Run a model over a manifest (PyTorch or ONNX Runtime) and decode quads in image coordinates."""

from __future__ import annotations

import numpy as np
import torch
from torch.utils.data import DataLoader

from common import IN_SIZE, apply_h, canonical_iou, letterbox_matrix
from data import EvalSet, prep
from decode import decode


def run_manifest(forward, manifest: str, size: int = IN_SIZE, stride: int = 4, limit: int | None = None, batch: int = 64, workers: int = 4):
    """``forward(x: torch.Tensor) -> np.ndarray (B, 11, S, S)``. Yields per-row dicts:
    ``id``, ``quad`` (normalised, canonical start, or None), ``feats`` and ``iou`` when the row has a ground truth."""
    ds = EvalSet(manifest, size, limit)
    dl = DataLoader(ds, batch_size=batch, shuffle=False, num_workers=workers, persistent_workers=False)
    results = []
    for x, idx in dl:
        out = forward(x)
        for b, i in enumerate(idx.tolist()):
            r = ds.rows[i]
            w, h = r["width"], r["height"]
            m = letterbox_matrix(w, h, size)
            dec = decode(out[b], stride)
            row = {"id": r["id"], "row": r}
            if dec is None:
                row.update(quad=None, feats=None, iou=0.0)
            else:
                px = apply_h(np.linalg.inv(m), dec["quad"])
                row.update(quad=(px / [w, h]).tolist(), feats=dec["feats"], px=px)
                if "quad" in r:
                    row["iou"] = canonical_iou(np.asarray(r["quad"]) * [w, h], px)
            results.append(row)
    return results


def torch_forward(model, device="cuda", amp=True):
    model.eval()

    @torch.no_grad()
    def f(x: torch.Tensor) -> np.ndarray:
        with torch.autocast(device_type="cuda", dtype=torch.bfloat16, enabled=amp and device == "cuda"):
            o = model(prep(x.to(device, non_blocking=True)).contiguous(memory_format=torch.channels_last))
        return o.float().cpu().numpy()

    return f


def summarise(rows: list[dict]) -> dict:
    ious = np.array([r["iou"] for r in rows])
    return {
        "n": len(rows),
        "mean_iou": float(ious.mean()) if len(ious) else 0.0,
        "fail_rate": float((ious < 0.9).mean()) if len(ious) else 0.0,
        "no_quad": int(sum(r["quad"] is None for r in rows)),
    }
