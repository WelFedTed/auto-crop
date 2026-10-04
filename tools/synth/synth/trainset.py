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
    "framing": {"full": 0.80, "partial": 0.20},
    "curl": {"flat": 0.84, "curled": 0.16},
    "format": {"jpeg": 0.70, "png": 0.10, "webp": 0.20},
    "exif": {"1": 1.0},
    "colorspace": {"srgb": 1.0},
    "rotation": {"upright": 0.30, "tilted": 0.30, "any": 0.40},
}
DEFAULT_HAND_SHARE = 0.4
DEFAULT_MAX_EDGE = 448


def check_seed(seed: int) -> None:
    """Refuse a seed that an evaluation suite uses (scene-disjointness, M4.13)."""
    if int(seed) in EVAL_SEEDS:
        raise ValueError(f"seed {seed} belongs to an evaluation suite; training data must use another seed")


def overrides(pins: dict | None = None) -> dict[str, dict[str, float]]:
    out = {a: dict(w) for a, w in OVERRIDES.items()}
    out.update(pins or {})
    return out


def extras(hand_share: float = DEFAULT_HAND_SHARE) -> dict:
    return {"hand_p": float(hand_share)}


assert set(backgrounds.TRAIN_KINDS) <= set(OVERRIDES["background"])
