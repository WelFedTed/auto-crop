# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Generates the two stand-in nets for the inference spike (ROADMAP M0.32-M0.34).

No trained weights are involved (B2/policy: nothing is downloaded or trained here). The graphs have
the SHAPE and OPERATOR MIX of the candidates, with deterministic random weights, which is what
operator coverage, latency, size and backend agreement depend on:

  quadnet.onnx  MobileNetV3-small-like backbone + FPN + 4-channel corner heatmap head, 1x3x256x256
                (stands in for DocQuadNet-256; BatchNorm folded into Conv as exporters do)
  orient.onnx   PP-LCNet-like depthwise-separable net with a 4-class (0/90/180/270) head, 1x3x224x224
                (stands in for the PP-LCNet doc-orientation model; paddle2onnx is not used, see ADR)

Also writes calibration/test inputs (npy) and, with --quantize, static QDQ int8 variants.
"""
import sys
import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

OPSET = 17


class G:
    def __init__(self, seed):
        self.rng = np.random.default_rng(seed)
        self.nodes, self.inits, self.n = [], [], 0

    def name(self, p):
        self.n += 1
        return f"{p}{self.n}"

    def w(self, arr, p="w"):
        nm = self.name(p)
        self.inits.append(numpy_helper.from_array(arr.astype(np.float32), nm))
        return nm

    def conv(self, x, cin, cout, k, s=1, groups=1, bias=True):
        fan = (cin // groups) * k * k
        wt = self.w(self.rng.normal(0, np.sqrt(2.0 / fan), (cout, cin // groups, k, k)))
        ins = [x, wt]
        if bias:
            ins.append(self.w(self.rng.normal(0, 0.01, (cout,)), "b"))
        out = self.name("c")
        self.nodes.append(helper.make_node("Conv", ins, [out], kernel_shape=[k, k], strides=[s, s], pads=[k // 2] * 4, group=groups))
        return out

    def op(self, typ, ins, **attrs):
        out = self.name(typ.lower())
        self.nodes.append(helper.make_node(typ, ins if isinstance(ins, list) else [ins], [out], **attrs))
        return out

    def act(self, x, kind):
        return self.op("HardSwish", x) if kind == "hs" else self.op("Relu", x)

    def se(self, x, c):
        r = max(8, c // 4)
        p = self.op("GlobalAveragePool", x)
        a = self.op("Relu", self.conv(p, c, r, 1))
        b = self.op("HardSigmoid", self.conv(a, r, c, 1), alpha=0.2, beta=0.5)
        return self.op("Mul", [x, b])

    def block(self, x, cin, k, exp, cout, s, se, act):
        y = x
        if exp != cin:
            y = self.act(self.conv(y, cin, exp, 1), act)
        y = self.act(self.conv(y, exp, exp, k, s, groups=exp), act)
        if se:
            y = self.se(y, exp)
        y = self.conv(y, exp, cout, 1)
        return self.op("Add", [x, y]) if (s == 1 and cin == cout) else y

    def up(self, x, factor):
        sc = self.name("scales")
        self.inits.append(numpy_helper.from_array(np.array([1, 1, factor, factor], dtype=np.float32), sc))
        out = self.name("resize")
        self.nodes.append(helper.make_node("Resize", [x, "", sc], [out], mode="nearest", nearest_mode="floor", coordinate_transformation_mode="asymmetric"))
        return out

    def save(self, path, inp, in_shape, out, out_shape):
        graph = helper.make_graph(
            self.nodes, "g",
            [helper.make_tensor_value_info(inp, TensorProto.FLOAT, in_shape)],
            [helper.make_tensor_value_info(out, TensorProto.FLOAT, out_shape)],
            self.inits,
        )
        model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", OPSET)])
        model.ir_version = 8
        onnx.checker.check_model(model)
        onnx.save(model, path)


def quadnet(path):
    g = G(1)
    x = "input"
    x = g.act(g.conv(x, 3, 16, 3, 2), "hs")  # 128
    x = g.block(x, 16, 3, 16, 16, 2, True, "relu")  # 64
    x = g.block(x, 16, 3, 72, 24, 2, False, "relu")  # 32
    c3 = g.block(x, 24, 3, 88, 24, 1, False, "relu")  # 32
    x = g.block(c3, 24, 5, 96, 40, 2, True, "hs")  # 16
    x = g.block(x, 40, 5, 240, 40, 1, True, "hs")
    x = g.block(x, 40, 5, 120, 48, 1, True, "hs")
    c4 = g.block(x, 48, 5, 144, 48, 1, True, "hs")  # 16
    x = g.block(c4, 48, 5, 288, 96, 2, True, "hs")  # 8
    c5 = g.block(x, 96, 5, 576, 96, 1, True, "hs")  # 8
    # FPN, 64 channels
    p5 = g.conv(c5, 96, 64, 1)
    p4 = g.op("Add", [g.conv(c4, 48, 64, 1), g.up(p5, 2)])
    p3 = g.op("Add", [g.conv(c3, 24, 64, 1), g.up(p4, 2)])
    p5 = g.conv(p5, 64, 64, 3)
    p4 = g.conv(p4, 64, 64, 3)
    p3 = g.conv(p3, 64, 64, 3)
    fused = g.op("Add", [g.op("Add", [g.up(p3, 2), g.up(g.up(p4, 2), 2)]), g.up(g.up(g.up(p5, 2), 2), 2)])  # stride 4: 64x64
    h = g.act(g.conv(fused, 64, 64, 3), "relu")
    out = g.op("Sigmoid", g.conv(h, 64, 4, 1))
    g.save(path, "input", [1, 3, 256, 256], out, [1, 4, 64, 64])


def orient(path):
    g = G(2)
    x = g.act(g.conv("input", 3, 16, 3, 2), "hs")  # 112
    cfg = [(16, 32, 3, 1), (32, 64, 3, 2), (64, 64, 3, 1), (64, 128, 3, 2), (128, 128, 3, 1), (128, 256, 5, 2), (256, 256, 5, 1), (256, 256, 5, 1), (256, 512, 5, 2), (512, 512, 5, 1)]
    for cin, cout, k, s in cfg:
        x = g.act(g.conv(x, cin, cin, k, s, groups=cin), "hs")
        x = g.act(g.conv(x, cin, cout, 1), "hs")
    x = g.se(x, 512)
    x = g.op("GlobalAveragePool", x)
    x = g.act(g.conv(x, 512, 1280, 1), "hs")
    x = g.conv(x, 1280, 4, 1)
    x = g.op("Flatten", x, axis=1)
    out = g.op("Softmax", x, axis=1)
    g.save(path, "input", [1, 3, 224, 224], out, [1, 4])


def quantize(src, dst, shape, n=24):
    import onnxruntime as ort
    from onnxruntime.quantization import CalibrationDataReader, QuantFormat, QuantType, quantize_static

    class R(CalibrationDataReader):
        def __init__(self):
            rng = np.random.default_rng(7)
            self.data = iter([{"input": rng.random(shape, dtype=np.float32)} for _ in range(n)])

        def get_next(self):
            return next(self.data, None)

    quantize_static(src, dst, R(), quant_format=QuantFormat.QDQ, activation_type=QuantType.QUInt8, weight_type=QuantType.QInt8)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "models"
    import os

    os.makedirs(out, exist_ok=True)
    quadnet(f"{out}/quadnet.onnx")
    orient(f"{out}/orient.onnx")
    rng = np.random.default_rng(99)
    rng.random((1, 3, 256, 256), dtype=np.float32).tofile(f"{out}/quadnet_input.f32")
    rng.random((1, 3, 224, 224), dtype=np.float32).tofile(f"{out}/orient_input.f32")
    if "--quantize" in sys.argv:
        quantize(f"{out}/quadnet.onnx", f"{out}/quadnet_int8.onnx", (1, 3, 256, 256))
        quantize(f"{out}/orient.onnx", f"{out}/orient_int8.onnx", (1, 3, 224, 224))
    print("ok")
