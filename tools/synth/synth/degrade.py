# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Degradations, with Augraphy behind a seam (ROADMAP M1.30, M1.33).

This is the **only** module that imports Augraphy (MIT; pinned to 8.2.6 in `requirements.lock`, a
2023 release, so it must stay replaceable). Everything else asks this module for a degradation by
purpose (paper and ink texture, photographic lighting, low-light noise) and gets pixels back. If
Augraphy cannot be imported, or ``SYNTH_BACKEND=builtin`` is set, the same purposes are served by
simple NumPy/OpenCV implementations below; the manifest records which backend produced each image
(``degrade_backend``), because the two do not produce identical pixels.

Augraphy draws from Python's ``random`` and NumPy's global generator, so each call is wrapped in
`_seeded`, which seeds both from the generator's own named stream. That makes every effect a pure
function of ``(suite seed, image, effect)``.

AlbumentationsX (AGPL-3.0) is never used or installed; `tests/test_independence.py` fails if it
appears in the lock file.
"""

from __future__ import annotations

import os
import random

import cv2
import numpy as np

from . import backgrounds
from . import rng as R

_AUG = None
_AUG_TRIED = False


def augraphy():
    """The Augraphy module, or None when unavailable or disabled."""
    global _AUG, _AUG_TRIED
    if os.environ.get("SYNTH_BACKEND", "") == "builtin":
        return None
    if not _AUG_TRIED:
        _AUG_TRIED = True
        try:
            import augraphy as mod  # the only Augraphy import in the project

            _AUG = mod
        except Exception:  # pragma: no cover - environment without Augraphy
            _AUG = None
    return _AUG


def backend_name() -> str:
    mod = augraphy()
    if mod is None:
        return "builtin"
    return f"augraphy-{getattr(mod, '__version__', '8.2.6')}"


def _seeded(seed: int, name: str):
    """Seed the global generators Augraphy uses, from the named stream."""
    s = R.int_seed(seed, "augraphy", name)
    random.seed(s)
    np.random.seed(s % (2**32))


def _run(seed: int, name: str, aug, img: np.ndarray) -> np.ndarray:
    _seeded(seed, name)
    out = aug(img.copy())
    if isinstance(out, dict):
        out = out["output"]
    return np.ascontiguousarray(out, dtype=np.uint8)


# ---------------------------------------------------------------------------------------------
# Page level: paper, ink and thermal fade (applied to the flat page before the camera)
# ---------------------------------------------------------------------------------------------


def fade_contrast_map(rng, h: int, w: int, c0: float) -> np.ndarray:
    """Spatially uneven ink contrast for faded thermal paper: mean ``c0`` with blotches and bands."""
    blot = backgrounds.fbm(rng, h, w, 4, 3)
    cmap = c0 * (0.55 + 0.9 * blot)
    # Print-head banding: a slow random walk along the paper's length.
    walk = np.cumsum(rng.normal(0, 0.03, size=h)).astype(np.float32)
    walk = (walk - walk.mean()) / (np.abs(walk - walk.mean()).max() + 1e-6)
    cmap *= 1.0 + 0.18 * walk[:, None]
    # Heavily faded lines near one edge (head wear).
    edge = 0.75 + 0.25 * np.clip(np.linspace(-1, 1, w)[None, :] * rng.choice([-1, 1]), 0, 1) ** 0.5
    return np.clip(cmap * edge, 0.02, 1.0).astype(np.float32)


def compose_page(page, rng, c0: float | None) -> tuple[np.ndarray, float]:
    """Paper colour, ink colour and ink contrast ``c0`` (None = normal) applied to the ink mask.

    Returns the page as uint8 RGB and the mean ink contrast actually used (1.0 for a normal page).
    """
    h, w = page.ink.shape
    a = page.ink.astype(np.float32) / 255.0
    if c0 is None:
        c = float(rng.uniform(0.86, 1.0))
        cmap = np.full((h, w), c, dtype=np.float32)
    else:
        cmap = fade_contrast_map(rng, h, w, c0)
    paper = np.array(page.paper_rgb, dtype=np.float32)
    ink = np.array(page.ink_rgb, dtype=np.float32)
    k = (a * cmap)[..., None]
    rgb = paper[None, None, :] * (1 - k) + ink[None, None, :] * k
    return np.clip(rgb + 0.5, 0, 255).astype(np.uint8), float(cmap[a > 0.5].mean()) if (a > 0.5).any() else 1.0


def page_texture(rgb: np.ndarray, seed: int, rng, fancy: bool) -> np.ndarray:
    """Paper grain, brightness mottling and ink bleed on the flat page (Augraphy or builtin)."""
    mod = augraphy()
    if mod is None:
        h, w = rgb.shape[:2]
        grain = backgrounds.fbm(rng, h, w, 4, max(4, w // 40))
        noise = rng.normal(0, 1.8, size=(h, w)).astype(np.float32)
        out = rgb.astype(np.float32) * (0.965 + 0.05 * grain[..., None]) + noise[..., None]
        return np.clip(out, 0, 255).astype(np.uint8)
    out = _run(seed, "ink-bleed", mod.InkBleed(intensity_range=(0.25, 0.45), kernel_size=(3, 3), severity=(0.2, 0.35), p=1), rgb)
    out = _run(seed, "noise-texturize", mod.NoiseTexturize(sigma_range=(2, 6), turbulence_range=(2, 4), texture_width_range=(120, 400), texture_height_range=(120, 400), p=1), out)
    out = _run(seed, "brightness-texturize", mod.BrightnessTexturize(texturize_range=(0.9, 0.99), deviation=0.04, p=1), out)
    if fancy:
        out = _run(seed, "stains", mod.Stains(stains_type="random", stains_blend_method="darken", stains_blend_alpha=0.35, p=1), out)
    return out


# ---------------------------------------------------------------------------------------------
# Photo level: lighting, noise and blur at the final resolution
# ---------------------------------------------------------------------------------------------


def lighting_gradient(img: np.ndarray, seed: int, rng, strength: float) -> np.ndarray:
    """A soft directional light fall-off over the whole picture (Augraphy or builtin)."""
    mod = augraphy()
    if mod is None or strength <= 0:
        h, w = img.shape[:2]
        ang = float(rng.uniform(0, 2 * np.pi))
        yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
        t = ((xx - w / 2) * np.cos(ang) + (yy - h / 2) * np.sin(ang)) / max(h, w)
        g = 1.0 + strength * t
        return np.clip(img.astype(np.float32) * g[..., None], 0, 255).astype(np.uint8)
    aug = mod.LightingGradient(
        light_position=None,
        direction=None,
        max_brightness=255,
        min_brightness=int(255 * (1 - 0.7 * strength) * 0.6),
        mode="gaussian",
        transparency=float(0.25 + 0.5 * strength),
        numba_jit=1,
        p=1,
    )
    return _run(seed, "lighting-gradient", aug, img)


def low_light_noise(img: np.ndarray, seed: int) -> np.ndarray:
    """Sensor noise of a dim scene (Augraphy), or a plain Gaussian equivalent."""
    mod = augraphy()
    if mod is None:
        return img
    return _run(seed, "low-light", mod.LowLightNoise(num_photons_range=(60, 110), alpha_range=(0.8, 1.0), beta_range=(5, 15), gamma_range=(1, 1.3), bias_range=(10, 25), p=1), img)


def blur(img: np.ndarray, level: str, rng, scale: float) -> np.ndarray:
    """Gaussian or motion blur by tag; ``scale`` = image long edge / 512 keeps strengths relative."""
    if level == "sharp":
        sigma = float(rng.uniform(0.0, 0.45)) * scale
        return cv2.GaussianBlur(img, (0, 0), sigma) if sigma > 0.25 else img
    if level == "soft":
        sigma = float(rng.uniform(0.9, 1.5)) * scale
        return cv2.GaussianBlur(img, (0, 0), sigma)
    if rng.random() < 0.5:
        return cv2.GaussianBlur(img, (0, 0), float(rng.uniform(2.0, 3.2)) * scale)
    n = max(3, int(round(float(rng.uniform(5, 11)) * scale)) | 1)  # motion blur
    k = np.zeros((n, n), dtype=np.float32)
    ang = float(rng.uniform(0, np.pi))
    c = n // 2
    cv2.line(k, (round(c - c * np.cos(ang)), round(c - c * np.sin(ang))), (round(c + c * np.cos(ang)), round(c + c * np.sin(ang))), 1.0, 1)
    k /= max(k.sum(), 1e-6)
    return cv2.filter2D(img, -1, k)


NOISE_SIGMA = {"low": (0.8, 2.0), "medium": (2.5, 4.5), "high": (5.0, 9.0)}


def sensor_noise(img: np.ndarray, level: str, rng, exposure: float) -> np.ndarray:
    """Luminance plus chroma Gaussian noise; a dim exposure (< 1) raises it."""
    lo, hi = NOISE_SIGMA[level]
    sigma = float(rng.uniform(lo, hi)) / max(exposure, 0.25) ** 0.5
    h, w = img.shape[:2]
    lum = rng.normal(0, sigma, size=(h, w, 1)).astype(np.float32)
    chroma = rng.normal(0, sigma * 0.35, size=(h, w, 3)).astype(np.float32)
    return np.clip(img.astype(np.float32) + lum + chroma, 0, 255).astype(np.uint8)
