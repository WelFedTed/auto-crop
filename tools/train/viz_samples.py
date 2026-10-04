# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Writes a sheet of augmented training samples with their targets (corner heat in red, mask in blue)."""
import sys
from pathlib import Path

import cv2
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from data import TrainSet  # noqa: E402


def main(manifest: str, out: str, n: int = 24) -> None:
    ds = TrainSet([manifest], 4, 256, seed=3)
    tiles = []
    for i in range(n):
        x, heat, off, offm = ds[i * 7 % len(ds)]
        img = ((x.numpy().transpose(1, 2, 0) * 0.5 + 0.5) * 255).astype(np.uint8)[..., ::-1].copy()
        up = lambda a: cv2.resize(a, (256, 256), interpolation=cv2.INTER_NEAREST)  # noqa: E731
        ov = img.astype(np.float32)
        ov[..., 2] = np.clip(ov[..., 2] + 255 * up(heat[0].numpy()), 0, 255)
        ov[..., 0] = np.clip(ov[..., 0] + 120 * up(heat[1].numpy()), 0, 255)
        ov[..., 1] = np.clip(ov[..., 1] + 255 * up(heat[2].numpy()), 0, 255)
        tiles.append(ov.astype(np.uint8))
    rows = [np.hstack(tiles[i : i + 6]) for i in range(0, len(tiles), 6)]
    cv2.imwrite(out, np.vstack(rows))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 24)
