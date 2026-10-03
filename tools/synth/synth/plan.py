# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Suite planning: which scene and which image gets which tags (ROADMAP M1.33, M1.35).

A *scene* is one page (text, paper, ink state) on one background (kind and texture seed); a scene
yields ``IMAGES_PER_SCENE`` photographs of it from different camera poses with different lighting,
clutter, blur, noise, file format, EXIF orientation and colour space. Splits are decided per scene,
so a page or a background never reaches both ``dev`` and ``test`` (M1.35 ``check-splits``).

Tag values are assigned by *balanced shuffles*: each axis gets exactly its quota of every value
(largest-remainder rounding), in an order shuffled by a stream named by ``(seed, axis)``. That is
what makes every slice cell of the full suite reach its quota instead of leaving it to chance.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

from . import rng as R

IMAGES_PER_SCENE = 3

# Quotas are shares of scenes (scene axes) or of images (image axes). Every value of every axis
# must reach QUOTA_MIN images on the full suite (>= 5,000 images), see `check_quotas`.
SCENE_AXES: dict[str, dict[str, float]] = {
    "aspect": {"document": 0.40, "receipt": 0.30, "long": 0.21, "strip": 0.09},
    "paper": {"white": 0.60, "cream": 0.20, "coloured": 0.20},
    "background": {
        "wood": 0.16,
        "fabric": 0.14,
        "stone": 0.14,
        "plain": 0.14,
        "tile": 0.12,
        "dark-mat": 0.14,
        "white-desk": 0.16,
    },
}
# Only receipts can fade (thermal paper); documents are always `normal`.
INK_QUOTA = {"normal": 0.55, "faded": 0.45}

IMAGE_AXES: dict[str, dict[str, float]] = {
    "lighting": {
        "normal": 0.34,
        "dim": 0.16,
        "low-contrast": 0.16,
        "harsh-shadow": 0.18,
        "colour-cast": 0.16,
    },
    "clutter": {"none": 0.30, "light": 0.35, "heavy": 0.35},
    "tilt": {"0-10": 0.34, "10-30": 0.33, "30-45": 0.33},
    "rotation": {"upright": 0.50, "tilted": 0.30, "any": 0.20},
    "framing": {"full": 0.88, "partial": 0.12},
    "curl": {"flat": 0.90, "curled": 0.10},
    "blur": {"sharp": 0.55, "soft": 0.30, "blurry": 0.15},
    "noise": {"low": 0.50, "medium": 0.30, "high": 0.20},
    "format": {"jpeg": 0.55, "png": 0.10, "tiff": 0.10, "webp": 0.25},
    "exif": {"1": 0.34, **{str(o): 0.66 / 7 for o in range(2, 9)}},
    "colorspace": {"srgb": 0.78, "display-p3": 0.22},
}

# Aspect ratio (long side : short side) range of the page for each `aspect` value.
ASPECT_RANGE = {
    "document": (1.29, 1.42),
    "receipt": (2.1, 3.9),
    "long": (4.2, 7.8),
    "strip": (8.4, 11.5),
}

QUOTA_MIN_FULL = 200
FULL_COUNT = 5200
SMOKE_COUNT = 200

# The smoke suite has a <= 5 MB archive budget (ROADMAP M1.35, no Git LFS). Noisy photographs do not
# compress, so it uses a smaller picture (long edge 320 px) and a small share of the lossless
# formats; the full suite is never archived and uses 512 px and the quotas above.
SUITES = {
    "smoke": {
        "count": SMOKE_COUNT,
        "seed": 0x5A0C_0001,
        "max_edge": 320,
        "overrides": {"format": {"jpeg": 0.62, "png": 0.06, "tiff": 0.06, "webp": 0.26}},
    },
    "full": {"count": FULL_COUNT, "seed": 0x5A0C_0002, "max_edge": 512, "overrides": {}},
}


@dataclass(frozen=True)
class SceneSpec:
    index: int
    scene_id: str
    split: str
    seed: int
    aspect: str
    paper: str
    ink: str
    background: str


@dataclass(frozen=True)
class ImageSpec:
    index: int
    image_id: str
    scene: SceneSpec
    seed: int
    lighting: str
    clutter: str
    tilt: str
    rotation: str
    framing: str
    curl: str
    blur: str
    noise: str
    format: str
    exif: int
    colorspace: str
    jpeg_quality: int

    def tags(self) -> dict[str, str]:
        s = self.scene
        return {
            "lighting": self.lighting,
            "clutter": self.clutter,
            "tilt": self.tilt,
            "rotation": self.rotation,
            "aspect": s.aspect,
            "framing": self.framing,
            "curl": self.curl,
            "ink": s.ink,
            "paper": s.paper,
            "background": s.background,
            "blur": self.blur,
            "noise": self.noise,
            "format": self.format,
            "exif": str(self.exif),
            "colorspace": self.colorspace,
        }


def balanced(rng, n: int, weights: dict[str, float]) -> list[str]:
    """``n`` values with each key appearing round(share * n) times (largest remainder), shuffled."""
    keys = list(weights)
    total = sum(weights.values())
    exact = [weights[k] / total * n for k in keys]
    counts = [math.floor(e) for e in exact]
    order = sorted(range(len(keys)), key=lambda i: (-(exact[i] - counts[i]), i))
    for i in order[: n - sum(counts)]:
        counts[i] += 1
    seq = [k for k, c in zip(keys, counts) for _ in range(c)]
    rng.shuffle(seq)
    return seq


def split_of(seed: int, scene_index: int) -> str:
    """30% ``dev``, 70% ``test``, decided by a hash of the scene alone."""
    return "dev" if R.int_seed(seed, "split", scene_index) % 10 < 3 else "test"


def pins_to_overrides(pins: list[str]) -> dict[str, dict[str, float]]:
    """``["lighting=dim", "clutter=none"]`` as weight overrides that always draw that value."""
    known = {**SCENE_AXES, **IMAGE_AXES, "ink": INK_QUOTA}
    out: dict[str, dict[str, float]] = {}
    for pin in pins:
        axis, _, value = pin.partition("=")
        axis_values = known.get(axis)
        if axis_values is None or value not in axis_values:
            raise ValueError(f"cannot pin {pin!r}: axes are " + ", ".join(f"{a}={'|'.join(v)}" for a, v in known.items()))
        out[axis] = {value: 1.0}
    return out


def build(
    name: str, seed: int, count: int, overrides: dict[str, dict[str, float]] | None = None
) -> tuple[list[SceneSpec], list[ImageSpec]]:
    """The scenes and images of a suite. ``overrides`` replaces the weights of named axes (the
    smoke suite's format mix, and ``--pin`` for single-factor experiments)."""
    ov = overrides or {}
    axes = {a: ov.get(a, w) for a, w in IMAGE_AXES.items()}
    ink_quota = ov.get("ink", INK_QUOTA)
    n_scenes = math.ceil(count / IMAGES_PER_SCENE)
    scene_cols = {
        axis: balanced(R.stream(seed, "scene-axis", axis), n_scenes, ov.get(axis, w))
        for axis, w in SCENE_AXES.items()
    }
    # Thermal fade only on receipts: a balanced draw over the receipt-like scenes.
    receipt_idx = [i for i, a in enumerate(scene_cols["aspect"]) if a != "document"]
    ink = ["normal"] * n_scenes
    drawn = balanced(R.stream(seed, "scene-axis", "ink"), len(receipt_idx), ink_quota)
    for i, v in zip(receipt_idx, drawn):
        ink[i] = v

    scenes = [
        SceneSpec(
            index=j,
            scene_id=f"{name}-s{j:05d}",
            split=split_of(seed, j),
            seed=R.int_seed(seed, "scene", j),
            aspect=scene_cols["aspect"][j],
            paper=scene_cols["paper"][j],
            ink=ink[j],
            background=scene_cols["background"][j],
        )
        for j in range(n_scenes)
    ]

    cols = {
        axis: balanced(R.stream(seed, "image-axis", axis), count, w)
        for axis, w in axes.items()
    }
    images = []
    for i in range(count):
        r = R.stream(seed, "image-misc", i)
        images.append(
            ImageSpec(
                index=i,
                image_id=f"{name}-{i:05d}",
                scene=scenes[i // IMAGES_PER_SCENE],
                seed=R.int_seed(seed, "image", i),
                lighting=cols["lighting"][i],
                clutter=cols["clutter"][i],
                tilt=cols["tilt"][i],
                rotation=cols["rotation"][i],
                framing=cols["framing"][i],
                curl=cols["curl"][i],
                blur=cols["blur"][i],
                noise=cols["noise"][i],
                format=cols["format"][i],
                exif=int(cols["exif"][i]),
                colorspace=cols["colorspace"][i],
                jpeg_quality=int(r.integers(40, 96)),
            )
        )
    return scenes, images


def histogram(tag_rows: list[dict[str, str]]) -> dict[str, dict[str, int]]:
    """Counts of every tag value over manifest-style tag dicts."""
    out: dict[str, dict[str, int]] = {}
    for tags in tag_rows:
        for axis, value in tags.items():
            out.setdefault(axis, {})
            out[axis][value] = out[axis].get(value, 0) + 1
    return out


def check_quotas(tag_rows: list[dict[str, str]], minimum: int = QUOTA_MIN_FULL) -> list[str]:
    """Problems with a tag histogram: planned values below ``minimum`` images (empty = fine)."""
    hist = histogram(tag_rows)
    problems = []
    planned = {**SCENE_AXES, **IMAGE_AXES, "ink": INK_QUOTA}
    for axis, values in planned.items():
        for value in values:
            n = hist.get(axis, {}).get(value, 0)
            if n < minimum:
                problems.append(f"{axis}={value}: {n} images, need >= {minimum}")
    return problems
