# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Backend parity of an exported net on real inputs (M4.16 bar; the ADR 0010 check used only 5 random inputs).

    python parity.py prepare --ckpt best.pt --manifest MANIFEST --n 64 --onnx net.onnx --inputs in.f32 --ref ref.f32
    # Rust: cargo run --release -p auto-crop-infer --features ort,rten --example net_parity -- net.onnx in.f32 64 SIZE out
    python parity.py compare --ref ref.f32 --got out.ort.f32 [--got out.rten.f32 ...] --n 64 --size SIZE --stride 4

``prepare`` exports the checkpoint (opset 17, fp32, static 1x3xSxS), letterboxes ``--n`` images of
the manifest exactly as the evaluation does, writes them as little-endian f32 in the net's input range
and the PyTorch fp32 CPU outputs. ``compare`` reports, per backend file, the largest absolute
difference of the raw outputs and, after the real decoder (``decode.py``), the largest difference of
the decoded corners in heatmap cells and in canvas pixels, plus how many images decode differently
(a corner snapped to another peak). Heatmap resolution is ``size / stride`` cells; the bar is
corners agreeing to < 0.1 cell.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from data import EvalSet  # noqa: E402
from decode import decode  # noqa: E402
from predict import load_model  # noqa: E402


def prepare(a) -> int:
    model, margs = load_model(a.ckpt, device="cpu")
    size, stride = margs.get("size", 256), margs.get("stride", 4)
    model = model.float().to(memory_format=torch.contiguous_format).eval()
    x0 = torch.rand(1, 3, size, size) * 2 - 1
    Path(a.onnx).parent.mkdir(parents=True, exist_ok=True)
    try:
        torch.onnx.export(model, x0, a.onnx, opset_version=17, input_names=["input"], output_names=["output"], dynamo=False)
    except TypeError:
        torch.onnx.export(model, x0, a.onnx, opset_version=17, input_names=["input"], output_names=["output"])
    import onnx

    m = onnx.load(a.onnx)
    onnx.checker.check_model(m)
    ops = sorted({n.op_type for n in m.graph.node})
    ds = EvalSet(a.manifest, size)
    idx = np.linspace(0, len(ds) - 1, min(a.n, len(ds))).round().astype(int)
    xs = torch.stack([ds[int(i)][0] for i in idx]).float() / 127.5 - 1.0
    with torch.no_grad():
        ref = torch.cat([model(xs[i : i + 1]) for i in range(len(xs))]).numpy()
    xs.numpy().astype("<f4").tofile(a.inputs)
    ref.astype("<f4").tofile(a.ref)
    print(f"exported {a.onnx} ({Path(a.onnx).stat().st_size / 1e6:.2f} MB, opset 17, operators {ops}); {len(xs)} inputs of {size}x{size} -> {a.inputs}; torch fp32 CPU outputs {tuple(ref.shape)} -> {a.ref}")
    return 0


def _corners(out: np.ndarray, stride: int):
    d = decode(out, stride)
    return None if d is None else d["quad"]


def compare(a) -> int:
    ch = 11
    s = a.size // a.stride
    ref = np.fromfile(a.ref, dtype="<f4").reshape(a.n, ch, s, s)
    rc = [_corners(ref[i], a.stride) for i in range(a.n)]
    rc = [None if q is None else np.asarray(q) for q in rc]
    ok = True
    for path in a.got:
        got = np.fromfile(path, dtype="<f4").reshape(a.n, ch, s, s)
        raw = float(np.abs(got - ref).max())
        raw_heat = float(np.abs(got[:, :3] - ref[:, :3]).max())
        worst, differ, mean_sum, mean_n = 0.0, 0, 0.0, 0
        for i in range(a.n):
            q = _corners(got[i], a.stride)
            if q is None or rc[i] is None:
                differ += int((q is None) != (rc[i] is None))
                continue
            # corners are in canvas pixels, canonical order: compare in heatmap cells
            d = np.linalg.norm(np.asarray(q) - rc[i], axis=1) / a.stride
            if d.max() > 0.5:
                differ += 1  # decoded to a different peak: not a rounding difference
                continue
            worst = max(worst, float(d.max()))
            mean_sum += float(d.sum())
            mean_n += 4
        passed = differ == 0 and worst < 0.1
        ok &= passed
        print(f"{Path(path).name}: n={a.n}  max |raw output diff| {raw:.2e} (heatmaps and mask logits {raw_heat:.2e})  decoded corners: max {worst:.2e} cells ({worst * a.stride:.2e} px at {a.size}), mean {mean_sum / max(mean_n, 1):.2e} cells, images decoding differently {differ}  -> {'PASS' if passed else 'FAIL'} (bar < 0.1 cell)")
    return 0 if ok else 1


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--ckpt", required=True)
    p.add_argument("--manifest", required=True)
    p.add_argument("--n", type=int, default=64)
    p.add_argument("--onnx", required=True)
    p.add_argument("--inputs", required=True)
    p.add_argument("--ref", required=True)
    c = sub.add_parser("compare")
    c.add_argument("--ref", required=True)
    c.add_argument("--got", action="append", required=True)
    c.add_argument("--n", type=int, required=True)
    c.add_argument("--size", type=int, default=256)
    c.add_argument("--stride", type=int, default=4)
    a = ap.parse_args(argv)
    return prepare(a) if a.cmd == "prepare" else compare(a)


if __name__ == "__main__":
    sys.exit(main())
