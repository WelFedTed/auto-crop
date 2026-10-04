# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Page-corner network: a MobileNetV3-small-style backbone (own code, random initialisation) with a
small FPN and one head that predicts, at stride 4 (or 2):

  ch 0       corner heatmap, class-agnostic (all four corners in one channel; rotation-equivariant)
  ch 1       page mask
  ch 2       page-centre heatmap
  ch 3..10   offsets centre -> the four corners (dx, dy per corner, clockwise, in units of 32 px of
             the network input), read at the centre peak; trained with a loss that does not care
             which corner is listed first

No pretrained weights anywhere (ROADMAP M4.05: from scratch); no torchvision, no downloads.
The network returns raw logits for ch 0..2 (apply sigmoid) and raw offsets.
"""

from __future__ import annotations

import torch
from torch import nn

OFFSET_UNIT = 32.0  # offsets are stored as pixels / 32 of the network input


def _act(kind: str) -> nn.Module:
    return nn.Hardswish(inplace=True) if kind == "hs" else nn.ReLU(inplace=True)


class ConvBN(nn.Sequential):
    def __init__(self, cin, cout, k=1, s=1, groups=1, act="hs"):
        layers = [nn.Conv2d(cin, cout, k, s, k // 2, groups=groups, bias=False), nn.BatchNorm2d(cout)]
        if act:
            layers.append(_act(act))
        super().__init__(*layers)


class SE(nn.Module):
    def __init__(self, c: int):
        super().__init__()
        r = max(8, c // 4)
        self.pool = nn.AdaptiveAvgPool2d(1)
        self.fc1 = nn.Conv2d(c, r, 1)
        self.fc2 = nn.Conv2d(r, c, 1)
        self.relu = nn.ReLU(inplace=True)
        self.gate = nn.Hardsigmoid()

    def forward(self, x):
        return x * self.gate(self.fc2(self.relu(self.fc1(self.pool(x)))))


class Block(nn.Module):
    def __init__(self, cin, k, exp, cout, s, se, act):
        super().__init__()
        layers = []
        if exp != cin:
            layers.append(ConvBN(cin, exp, 1, act=act))
        layers.append(ConvBN(exp, exp, k, s, groups=exp, act=act))
        if se:
            layers.append(SE(exp))
        layers.append(ConvBN(exp, cout, 1, act=None))
        self.body = nn.Sequential(*layers)
        self.skip = s == 1 and cin == cout

    def forward(self, x):
        y = self.body(x)
        return x + y if self.skip else y


# (kernel, expansion, out, stride, SE, activation): MobileNetV3-small layout.
CFG = [
    (3, 16, 16, 2, True, "relu"),   # stride 4
    (3, 72, 24, 2, False, "relu"),  # stride 8
    (3, 88, 24, 1, False, "relu"),
    (5, 96, 40, 2, True, "hs"),     # stride 16
    (5, 240, 40, 1, True, "hs"),
    (5, 240, 40, 1, True, "hs"),
    (5, 120, 48, 1, True, "hs"),
    (5, 144, 48, 1, True, "hs"),
    (5, 288, 96, 2, True, "hs"),    # stride 32
    (5, 576, 96, 1, True, "hs"),
    (5, 576, 96, 1, True, "hs"),
]


class CornerNet(nn.Module):
    def __init__(self, width: float = 1.0, fpn: int = 64, out_stride: int = 4, head: int = 64):
        super().__init__()
        assert out_stride in (2, 4)
        self.out_stride = out_stride

        def w(c):
            return max(8, int(round(c * width / 8)) * 8)

        self.stem = ConvBN(3, w(16), 3, 2, act="hs")  # stride 2
        blocks, cin = [], w(16)
        for k, exp, cout, s, se, act in CFG:
            blocks.append(Block(cin, k, w(exp), w(cout), s, se, act))
            cin = w(cout)
        self.blocks = nn.ModuleList(blocks)
        # taps: stride 2 (stem), 4 (block 0), 8 (block 2), 16 (block 7), 32 (block 10)
        ch = [w(16), w(16), w(24), w(48), w(96)]
        levels = [4, 3, 2, 1] + ([0] if out_stride == 2 else [])
        self.lat = nn.ModuleDict({f"l{i}": nn.Conv2d(ch[i], fpn, 1) for i in levels})
        self.up = nn.Upsample(scale_factor=2, mode="nearest")
        self.smooth = nn.ModuleDict({f"l{i}": ConvBN(fpn, fpn, 3, groups=fpn, act="hs") for i in levels if i < 4})
        self.mix = nn.ModuleDict({f"l{i}": ConvBN(fpn, fpn, 1, act="hs") for i in levels if i < 4})
        self.ctx = nn.Sequential(ConvBN(fpn, fpn, 3, groups=fpn, act="hs"), ConvBN(fpn, fpn, 1, act="hs"))
        hc = head
        self.head = nn.Sequential(ConvBN(fpn, hc, 3, act="hs"), ConvBN(hc, hc, 3, groups=hc, act="hs"), ConvBN(hc, hc, 1, act="hs"))
        self.out = nn.Conv2d(hc, 11, 1)
        # Prior for the focal losses: start with low heatmap probabilities.
        with torch.no_grad():
            self.out.bias.zero_()
            self.out.bias[:3] = -2.19
        self._init()

    def _init(self):
        for m in self.modules():
            if isinstance(m, nn.Conv2d):
                nn.init.kaiming_normal_(m.weight, mode="fan_out")
                if m.bias is not None and m is not self.out:
                    nn.init.zeros_(m.bias)
            elif isinstance(m, nn.BatchNorm2d):
                nn.init.ones_(m.weight)
                nn.init.zeros_(m.bias)

    def forward(self, x):
        s2 = self.stem(x)
        feats = {}
        y = s2
        for i, b in enumerate(self.blocks):
            y = b(y)
            if i in (0, 2, 7, 10):
                feats[i] = y
        c2, c4, c8, c16, c32 = s2, feats[0], feats[2], feats[7], feats[10]
        p = self.ctx(self.lat["l4"](c32))
        taps = [(3, c16), (2, c8), (1, c4)] + ([(0, c2)] if self.out_stride == 2 else [])
        for lvl, c in taps:
            k = f"l{lvl}"
            p = self.mix[k](self.smooth[k](self.lat[k](c) + self.up(p)))
        o = self.out(self.head(p))
        return o  # (B, 11, H/stride, W/stride)


def count_params(m: nn.Module) -> int:
    return sum(p.numel() for p in m.parameters())


if __name__ == "__main__":
    net = CornerNet()
    print("params", count_params(net))
    print(net(torch.zeros(1, 3, 256, 256)).shape)
    print("params stride2", count_params(CornerNet(out_stride=2)))
