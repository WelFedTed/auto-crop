# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Predict page quads for a manifest and write them as JSON lines in the harness format.

    python predict.py --ckpt target/train/run1/best.pt --manifest target/synth/smoke/manifest.jsonl \
        --out target/preds/smoke.jsonl [--conf target/train/run1/conf.json]
    python predict.py --ckpt ... --fit-conf target/synth/val/manifest.jsonl   # writes conf.json next to the checkpoint

then score with the repo's own harness:

    cargo xtask eval run --manifest MANIFEST --predictor jsonl:preds.jsonl --out result.json

The confidence is a logistic combination of five decode features fitted on the SYNTHETIC validation
set (``--fit-conf``); it is not calibrated on real photographs. ``state`` is ``good`` at or above
the threshold chosen there (the lowest one with at most 1% silent failures on validation), else
``check``; no quad is ``failed``. Aggregates only for private files.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import clamp_quad_to_frame  # noqa: E402
from decode import feature_vector  # noqa: E402
from infer import run_manifest, summarise, torch_forward  # noqa: E402
from model import CornerNet  # noqa: E402


def load_model(ckpt: str, device="cuda"):
    c = torch.load(ckpt, map_location="cpu", weights_only=False)
    a = c["args"]
    m = CornerNet(width=a.get("width", 1.0), out_stride=a.get("stride", 4))
    m.load_state_dict(c["ema"])
    return m.to(device).to(memory_format=torch.channels_last).eval(), a


def ort_forward(path: str):
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    sess = ort.InferenceSession(path, so, providers=["CPUExecutionProvider"])
    name = sess.get_inputs()[0].name

    def f(x: torch.Tensor) -> np.ndarray:
        return sess.run(None, {name: (x.float() / 127.5 - 1.0).numpy()})[0]

    return f


def fit_logistic(x: np.ndarray, y: np.ndarray, iters: int = 400) -> tuple[np.ndarray, float]:
    xt = torch.tensor(x, dtype=torch.float64)
    yt = torch.tensor(y, dtype=torch.float64)
    w = torch.zeros(x.shape[1], dtype=torch.float64, requires_grad=True)
    b = torch.zeros(1, dtype=torch.float64, requires_grad=True)
    opt = torch.optim.LBFGS([w, b], lr=0.5, max_iter=iters)

    def closure():
        opt.zero_grad()
        loss = torch.nn.functional.binary_cross_entropy_with_logits(xt @ w + b, yt) + 1e-3 * (w**2).sum()
        loss.backward()
        return loss

    opt.step(closure)
    return w.detach().numpy(), float(b.detach())


def confidences(rows: list[dict], conf: dict | None) -> list[float]:
    out = []
    for r in rows:
        if r["quad"] is None or conf is None:
            out.append(0.0 if r["quad"] is None else float(r["feats"]["min_peak"]) if r["feats"] else 0.0)
            continue
        z = float(feature_vector(r["feats"]) @ np.array(conf["w"]) + conf["b"])
        out.append(1.0 / (1.0 + np.exp(-z)))
    return out


def fit_conf(rows: list[dict], max_silent: float = 0.01, min_accept: int = 150) -> dict:
    ok = [r for r in rows if r["quad"] is not None]
    x = np.array([feature_vector(r["feats"]) for r in ok])
    y = np.array([1.0 if r["iou"] >= 0.9 else 0.0 for r in ok])
    w, b = fit_logistic(x, y)
    conf = {"w": w.tolist(), "b": b, "features": "min_peak, mean_peak, n_matched/4, ctr_peak, mask_iou, exp(-8*agree)", "fitted_on": "synthetic validation"}
    p = confidences(rows, conf)
    order = np.argsort(-np.array(p))
    ious = np.array([r["iou"] for r in rows])[order]
    ps = np.array(p)[order]
    thr = 0.99
    # lowest threshold whose accepted set has <= max_silent failures (and a minimum size)
    silent = np.cumsum(ious < 0.9)
    for k in range(len(ps) - 1, min_accept, -1):
        if silent[k] / (k + 1) <= max_silent:
            thr = float(ps[k])
            break
    conf["threshold_good"] = thr
    return conf


def write_preds(rows: list[dict], conf: dict | None, out: str, thr: float | None = None, clamp: bool = False) -> None:
    ps = confidences(rows, conf)
    t = thr if thr is not None else (conf or {}).get("threshold_good", 0.9)
    with open(out, "w", encoding="utf8", newline="\n") as f:
        for r, p in zip(rows, ps):
            if r["quad"] is None:
                f.write(json.dumps({"id": r["id"], "quad": None, "state": "failed"}) + "\n")
                continue
            quad = r["quad"]
            if clamp:  # answer the visible part of a page the frame cuts (the convention of the owner's labels)
                quad = clamp_quad_to_frame(quad) or quad
            f.write(json.dumps({"id": r["id"], "quad": [[round(a, 6), round(b, 6)] for a, b in quad], "confidence": round(float(p), 5), "state": "good" if p >= t else "check", **{"feats": r["feats"]}}) + "\n")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt")
    ap.add_argument("--onnx", help="use this ONNX file (ONNX Runtime CPU) instead of the checkpoint")
    ap.add_argument("--manifest")
    ap.add_argument("--out")
    ap.add_argument("--conf")
    ap.add_argument("--fit-conf", help="validation manifest to fit the confidence on; writes conf.json next to the checkpoint")
    ap.add_argument("--threshold", type=float)
    ap.add_argument("--clamp", action="store_true", help="clip the quad to the frame (visible part of a cut page)")
    ap.add_argument("--limit", type=int)
    a = ap.parse_args(argv)
    model, margs = load_model(a.ckpt) if a.ckpt else (None, {"stride": 4, "size": 256})
    stride, size = margs.get("stride", 4), margs.get("size", 256)
    fwd = ort_forward(a.onnx) if a.onnx else torch_forward(model)
    if a.fit_conf:
        rows = run_manifest(fwd, a.fit_conf, size, stride, limit=a.limit)
        conf = fit_conf(rows)
        path = Path(a.ckpt).with_name("conf.json")
        path.write_text(json.dumps(conf, indent=1), encoding="utf8")
        print("fit-conf on", summarise(rows), "threshold", conf["threshold_good"], "->", path)
        return 0
    conf = json.loads(Path(a.conf).read_text(encoding="utf8")) if a.conf else None
    rows = run_manifest(fwd, a.manifest, size, stride, limit=a.limit)
    write_preds(rows, conf, a.out, a.threshold, a.clamp)
    if rows and "iou" in rows[0]:
        print(json.dumps(summarise(rows)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
