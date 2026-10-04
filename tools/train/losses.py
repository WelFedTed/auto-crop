# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Losses: penalty-reduced focal loss for the heatmaps, BCE + Dice for the mask, and an L1 loss on
the centre-to-corner offsets that does not depend on which corner is listed first."""

from __future__ import annotations

import torch
import torch.nn.functional as F


def focal(logits: torch.Tensor, gt: torch.Tensor) -> torch.Tensor:
    """CenterNet focal loss over a (B, S, S) heatmap with peaks equal to 1."""
    p = torch.sigmoid(logits.float()).clamp(1e-4, 1 - 1e-4)
    pos = (gt >= 0.999).float()
    neg = 1.0 - pos
    pl = -torch.log(p) * (1 - p) ** 2 * pos
    nl = -torch.log(1 - p) * p**2 * (1 - gt) ** 4 * neg
    n = pos.sum().clamp(min=1.0)
    return (pl.sum() + nl.sum()) / n


def mask_loss(logits: torch.Tensor, gt: torch.Tensor) -> torch.Tensor:
    bce = F.binary_cross_entropy_with_logits(logits.float(), gt)
    p = torch.sigmoid(logits.float())
    inter = (p * gt).sum(dim=(1, 2))
    dice = 1 - (2 * inter + 1) / (p.sum(dim=(1, 2)) + gt.sum(dim=(1, 2)) + 1)
    return bce + dice.mean()


def offset_loss(pred: torch.Tensor, gt: torch.Tensor, m: torch.Tensor) -> torch.Tensor:
    """pred, gt: (B, 8, S, S) = four (dx, dy); m: (B, S, S). Minimum over the 4 cyclic shifts."""
    b, _, s, _ = pred.shape
    p = pred.float().view(b, 4, 2, s, s)
    g = gt.float().view(b, 4, 2, s, s)
    errs = torch.stack([(p - torch.roll(g, k, dims=1)).abs().sum(dim=(1, 2)) for k in range(4)], dim=0)  # (4, B, S, S)
    best = errs.min(dim=0).values
    return (best * m).sum() / m.sum().clamp(min=1.0)


def total_loss(out: torch.Tensor, heat: torch.Tensor, off: torch.Tensor, off_mask: torch.Tensor, w_off: float = 1.0) -> tuple[torch.Tensor, dict]:
    lc = focal(out[:, 0], heat[:, 0])
    lm = mask_loss(out[:, 1], heat[:, 1])
    lz = focal(out[:, 2], heat[:, 2])
    lo = offset_loss(out[:, 3:11], off, off_mask)
    loss = lc + lm + lz + w_off * lo
    return loss, {"corner": float(lc), "mask": float(lm), "centre": float(lz), "offset": float(lo)}
