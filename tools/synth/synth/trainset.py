# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""TRAINING mode of the generator (ROADMAP M4.12, feasibility study ADR 0010).

``python -m synth --train --seed S --count N --out DIR`` writes a suite meant for TRAINING a learned
page-corner detector, not for evaluating one: the tag quotas lean towards the failures of the
classical detector (long and strip receipts, textured desks, partial frames, curl), four extra desk
textures appear (``backgrounds.TRAIN_KINDS``), and a share of the pictures show a hand holding the
page (``occluders``). Files are JPEG, PNG or WebP, EXIF orientation 1 and sRGB only, so that a
plain image reader gives the upright picture.

It changes nothing about the evaluation suites: it is a separate preset reached only through
``--train``, the extra random numbers come from streams of their own, and a test regenerates the
smoke suite and compares every byte.

Seed discipline (M4.13): training seeds must never equal an evaluation seed. ``EVAL_SEEDS`` lists the
seeds of every evaluation suite named in the docs; ``check_seed`` refuses them. Scene ids are
``<name>-sNNNNN`` and derive from the seed, so disjoint seeds give disjoint scenes.
"""

from __future__ import annotations

import cv2
import numpy as np

from . import backgrounds

# Seeds used by evaluation suites (smoke, full, smoke-b, smoke-c, mid-d, the one-factor sweep, the
# Rust stand-in suites, the multi-item suites). A training or validation run may not use them.
EVAL_SEEDS = frozenset(
    {
        0x5A0C_0001,  # smoke
        0x5A0C_0002,  # full
        1234567,  # smoke-b
        987654321,  # smoke-c
        424242,  # mid-d
        777,  # one-factor sweeps
        31415926,
        20261002,
        0x5EED_A070_C20B_0001,
        0x5A0C_0C01,  # curved-smoke
        0x5A0C_0C02,  # curved-full
    }
)

# Quotas biased towards what the classical detector gets wrong. Image axes not listed keep the
# default quotas, except exif and colour space, which are pinned.
OVERRIDES: dict[str, dict[str, float]] = {
    "aspect": {"document": 0.20, "receipt": 0.28, "long": 0.32, "strip": 0.20},
    "background": {
        "wood": 0.10, "fabric": 0.08, "stone": 0.08, "plain": 0.06, "tile": 0.08, "dark-mat": 0.06, "white-desk": 0.10,
        "planks": 0.12, "marble": 0.10, "terrazzo": 0.12, "carpet": 0.10,
    },
    "framing": {"full": 0.70, "partial": 0.30},
    "curl": {"flat": 0.84, "curled": 0.16},
    "format": {"jpeg": 0.70, "png": 0.10, "webp": 0.20},
    "exif": {"1": 1.0},
    "colorspace": {"srgb": 1.0},
    "rotation": {"upright": 0.30, "tilted": 0.30, "any": 0.40},
}
DEFAULT_HAND_SHARE = 0.5
DEFAULT_MAX_EDGE = 448
# Real failure modes added for the second training cycle (ADR 0010 update). Each is a probability
# per picture (``tight``, ``wow``) or per document scene (``mat``) and is drawn from a stream of its
# own, so the evaluation suites are untouched and the older training streams do not move.
DEFAULT_TIGHT_SHARE = 0.25  # the page fills the frame: corners within a few percent of the border
DEFAULT_MAT_SHARE = 0.15  # a photo print: coloured picture inside a white mat (truth = the whole print)
DEFAULT_WOW_SHARE = 0.2  # white (or paper-coloured) on white: the desk is within a few grey levels of the page


def check_seed(seed: int) -> None:
    """Refuse a seed that an evaluation suite uses (scene-disjointness, M4.13)."""
    if int(seed) in EVAL_SEEDS:
        raise ValueError(f"seed {seed} belongs to an evaluation suite; training data must use another seed")


def overrides(pins: dict | None = None) -> dict[str, dict[str, float]]:
    out = {a: dict(w) for a, w in OVERRIDES.items()}
    out.update(pins or {})
    return out


def extras(
    hand_share: float = DEFAULT_HAND_SHARE,
    tight_share: float = DEFAULT_TIGHT_SHARE,
    mat_share: float = DEFAULT_MAT_SHARE,
    wow_share: float = DEFAULT_WOW_SHARE,
) -> dict:
    return {
        "hand_p": float(hand_share),
        "tight_p": float(tight_share),
        "mat_p": float(mat_share),
        "wow_p": float(wow_share),
        "wide_partial": True,  # partial frames may cut away up to 60 percent of the page (default 45)
    }


def apply_mat(tex: np.ndarray, rng) -> np.ndarray:
    """A photo print: a procedural coloured picture inside a white mat, painted onto the flat page
    texture (uint8 RGB). The page, and so the ground truth, stays the whole print including the mat.
    The mat is 4 to 14 percent of the short side, sometimes thicker below (instant-film style)."""
    h, w = tex.shape[:2]
    short = float(min(h, w))
    m = [float(rng.uniform(0.04, 0.14)) * short for _ in range(4)]  # left, top, right, bottom
    if rng.random() < 0.25:
        m[3] += float(rng.uniform(0.04, 0.12)) * short
    x0, y0, x1, y1 = int(m[0]), int(m[1]), int(round(w - m[2])), int(round(h - m[3]))
    if x1 - x0 < 16 or y1 - y0 < 16:
        return tex
    ih, iw = y1 - y0, x1 - x0
    chans = []
    base = rng.uniform(40, 215, size=3)
    for c in range(3):
        f = backgrounds.fbm(rng, ih, iw, 5, 3)
        chans.append(base[c] + (f - 0.5) * float(rng.uniform(60, 170)))
    pic = np.stack(chans, axis=2)
    gy, gx = np.mgrid[0:ih, 0:iw].astype(np.float32)
    vig = 1.0 - float(rng.uniform(0.0, 0.35)) * (((gx - iw / 2) / (iw / 2)) ** 2 + ((gy - ih / 2) / (ih / 2)) ** 2) / 2
    pic = pic * vig[..., None] + rng.normal(0, 2.5, size=(ih, iw, 1))
    out = tex.copy()
    out[y0:y1, x0:x1] = np.clip(pic, 0, 255).astype(np.uint8)
    # A faint edge where the picture meets the mat.
    cv2.rectangle(out, (x0, y0), (x1 - 1, y1 - 1), (int(rng.integers(150, 215)),) * 3, 1)
    return out


def desk_towards_paper(bg: np.ndarray, paper_rgb, rng) -> np.ndarray:
    """White on white: pull the desk to within a few grey levels of the paper (a mix of 88 to 98.5
    percent of a slightly darker paper colour), keeping a trace of the desk texture."""
    target = np.array(paper_rgb, dtype=np.float32) * float(rng.uniform(0.90, 0.99))
    k = float(rng.uniform(0.88, 0.985))
    return np.clip(target[None, None, :] * k + bg * (1 - k), 0, 255)


assert set(backgrounds.TRAIN_KINDS) <= set(OVERRIDES["background"])
