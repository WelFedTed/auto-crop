# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Seeded random streams.

Every random decision in the generator comes from a stream named by ``(seed, *keys)``, so an image
depends only on the suite seed and its own index, never on how many worker processes ran or in what
order. The mapping from names to streams is a BLAKE2b hash, which is stable across platforms; the
streams themselves are NumPy's PCG64, stable within one pinned NumPy version.
"""

from __future__ import annotations

import hashlib

import numpy as np


def _digest(seed: int, keys: tuple) -> bytes:
    h = hashlib.blake2b(digest_size=16)
    h.update(str(int(seed)).encode())
    for k in keys:
        h.update(b"\x00")
        h.update(str(k).encode())
    return h.digest()


def stream(seed: int, *keys) -> np.random.Generator:
    """A NumPy generator named by ``(seed, *keys)``."""
    return np.random.Generator(np.random.PCG64(int.from_bytes(_digest(seed, keys), "little")))


def int_seed(seed: int, *keys) -> int:
    """A 63-bit integer seed named by ``(seed, *keys)`` (for libraries that want an int)."""
    return int.from_bytes(_digest(seed, keys)[:8], "little") >> 1


def choice_weighted(rng: np.random.Generator, weights: dict):
    """One key of ``weights`` drawn with probability proportional to its value."""
    keys = list(weights)
    p = np.array([weights[k] for k in keys], dtype=np.float64)
    return keys[int(rng.choice(len(keys), p=p / p.sum()))]
