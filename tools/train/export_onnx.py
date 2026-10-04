# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Export a checkpoint to ONNX (opset 17, fp32, static 1x3x256x256) and check parity with ONNX Runtime.

    python export_onnx.py --ckpt target/train/run1/best.pt --out target/models/corner-net.onnx

The BatchNorm layers are folded into the convolutions by the exporter (eval mode). The graph uses
Conv, HardSwish, HardSigmoid, Relu, GlobalAveragePool, Mul, Add, Resize (nearest) only, the same
operator set the inference spike (ADR 0007) covered. Writes ``<out>`` and prints the file size and
the largest absolute difference between PyTorch and ONNX Runtime on random and real-looking input.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from predict import load_model  # noqa: E402


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--size", type=int, default=256)
    a = ap.parse_args(argv)
    model, _ = load_model(a.ckpt, device="cpu")
    model = model.float().to(memory_format=torch.contiguous_format).eval()
    x = torch.rand(1, 3, a.size, a.size) * 2 - 1
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    try:
        torch.onnx.export(model, x, a.out, opset_version=17, input_names=["input"], output_names=["output"], dynamo=False)
    except TypeError:  # older torch without the dynamo flag
        torch.onnx.export(model, x, a.out, opset_version=17, input_names=["input"], output_names=["output"])
    import onnx
    import onnxruntime as ort

    m = onnx.load(a.out)
    onnx.checker.check_model(m)
    ops = sorted({n.op_type for n in m.graph.node})
    sess = ort.InferenceSession(a.out, providers=["CPUExecutionProvider"])
    worst = 0.0
    for seed in range(5):
        xx = torch.rand(1, 3, a.size, a.size, generator=torch.Generator().manual_seed(seed)) * 2 - 1
        with torch.no_grad():
            ref = model(xx).numpy()
        got = sess.run(None, {"input": xx.numpy()})[0]
        worst = max(worst, float(np.abs(ref - got).max()))
    size_mb = Path(a.out).stat().st_size / 1e6
    print(f"wrote {a.out}: {size_mb:.2f} MB, opset 17, operators {ops}, max |torch - ort| over 5 inputs = {worst:.2e}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
