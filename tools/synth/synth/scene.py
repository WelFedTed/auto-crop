# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""One scene (a page on a desk) photographed into several images (ROADMAP M1.31 to M1.33)."""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import cv2
import numpy as np

from . import backgrounds, camera, curved, degrade, occluders, page as pagemod
from . import rng as R
from .plan import ImageSpec, SceneSpec

TILT_RANGE = {"0-10": (0.0, 10.0), "10-30": (10.0, 30.0), "30-45": (30.0, 45.0)}
ROLL_RANGE = {"upright": (0.0, 15.0), "tilted": (15.0, 45.0), "any": (45.0, 180.0)}
EXPOSURE = {"dim": (0.34, 0.52)}


@dataclass
class SceneAssets:
    spec: SceneSpec
    page: pagemod.Page
    texture: np.ndarray  # uint8 RGB, the degraded flat page
    ink_contrast: float
    fade_c0: float | None
    meta: dict = field(default_factory=dict)


def build_scene(spec: SceneSpec, extras: dict | None = None) -> SceneAssets:
    """Render the scene's page once; its photographs reuse it. ``extras`` is for the TRAINING mode
    only: ``{"mat_p": p}`` turns a share of the document scenes into photo prints with a white mat
    (drawn from a stream of its own; without extras nothing changes)."""
    rng = R.stream(spec.seed, "page")
    pg = pagemod.render(rng, spec.aspect, spec.paper)
    c0 = None if spec.ink == "normal" else float(rng.uniform(0.10, 0.60))
    rgb, used = degrade.compose_page(pg, rng, c0)
    tex = degrade.page_texture(rgb, spec.seed, rng, fancy=bool(rng.random() < 0.10))
    meta: dict = {}
    if extras and extras.get("mat_p", 0.0) > 0.0 and spec.aspect == "document":
        from . import trainset

        mrng = R.stream(spec.seed, "train-mat")
        if mrng.random() < float(extras["mat_p"]):
            tex = trainset.apply_mat(tex, mrng)
            meta["mat"] = True
    return SceneAssets(spec, pg, tex, used, c0, meta)


def _pose(spec: ImageSpec, rng) -> tuple[float, float, float]:
    lo, hi = TILT_RANGE[spec.tilt]
    dom = float(rng.uniform(lo, hi))
    other = float(rng.uniform(0.0, dom))
    sign = lambda: 1.0 if rng.random() < 0.5 else -1.0  # noqa: E731
    pitch, yaw = (dom * sign(), other * sign()) if rng.random() < 0.5 else (other * sign(), dom * sign())
    rlo, rhi = ROLL_RANGE[spec.rotation]
    roll = float(rng.uniform(rlo, rhi)) * sign()
    return pitch, yaw, roll


def _curl(rng, tex: tuple[int, int]) -> camera.Curl:
    kind = ["cylinder", "edge", "corner"][int(rng.integers(3))]
    short = float(min(tex))
    return camera.Curl(kind, float(rng.uniform(0.07, 0.20)) * short, float(rng.uniform(0, 2 * math.pi)), float(rng.uniform(0.25, 0.7)))


def _bent_camera(spec, kind: str, tex: tuple[int, int], pose, rng, max_edge: int):
    """Camera and bend of a curved-boundary page: the amplitude is calibrated so that the page's
    curvature in the picture matches the planned bucket (`curved.calibrate`)."""
    brng = R.stream(spec.seed, "bend")
    first = curved.draw_bend(brng, kind, tex)
    jitter = float(brng.uniform(0.8, 1.25))
    pitch, yaw, roll = pose

    def make_cam(b):
        return camera.fit(tex, pitch, yaw, roll, curved.fresh(rng), max_edge, False, b, outline=24)

    bend, _ = curved.calibrate(tex, make_cam, first, curved.BUCKET_TARGET[spec.curvature], jitter)
    # The real camera consumes the stream as the flat path does.
    return camera.fit(tex, pitch, yaw, roll, rng, max_edge, False, bend, outline=24), bend


def _shadow_field(rng, h: int, w: int, centre: np.ndarray) -> np.ndarray:
    """A soft hard-edged shadow (band or half plane) across the frame, 1 = lit, < 1 = shadowed."""
    scale = max(h, w)
    ang = float(rng.uniform(0, math.pi))
    nx, ny = math.cos(ang), math.sin(ang)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    off = float(rng.uniform(-0.25, 0.25)) * scale
    t = (xx - centre[0]) * nx + (yy - centre[1]) * ny - off
    if rng.random() < 0.5:  # half plane
        inside = (t > 0).astype(np.float32)
    else:  # band
        wid = float(rng.uniform(0.12, 0.35)) * scale
        inside = (np.abs(t) < wid / 2).astype(np.float32)
    sigma = float(rng.uniform(0.01, 0.045)) * scale
    inside = cv2.GaussianBlur(inside, (0, 0), sigma)
    depth = float(rng.uniform(0.38, 0.62))
    return 1.0 - (1.0 - depth) * inside


def _illumination(spec: ImageSpec, rng, h: int, w: int, centre: np.ndarray) -> tuple[np.ndarray, float]:
    """Multiplicative light field (h, w, 3) and the exposure used."""
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    ang = float(rng.uniform(0, 2 * math.pi))
    grad = ((xx - w / 2) * math.cos(ang) + (yy - h / 2) * math.sin(ang)) / max(h, w)
    field = 1.0 + float(rng.uniform(0.06, 0.16)) * grad
    r2 = ((xx - w / 2) ** 2 + (yy - h / 2) ** 2) / ((0.5 * math.hypot(w, h)) ** 2)
    field = field * (1.0 - float(rng.uniform(0.0, 0.12)) * r2)
    light = np.repeat(field[..., None], 3, axis=2).astype(np.float32)
    if spec.lighting == "harsh-shadow":
        light *= _shadow_field(rng, h, w, centre)[..., None]
    if spec.lighting == "colour-cast":
        s = float(rng.uniform(0.12, 0.32))
        gains = {
            "warm": (1 + s, 1.0, 1 - s),
            "cool": (1 - s, 0.98, 1 + s),
            "green": (1 - s / 2, 1 + s / 2, 1 - s / 2),
            "magenta": (1 + s / 2, 1 - s / 2, 1 + s / 2),
        }[["warm", "cool", "green", "magenta"][int(rng.integers(4))]]
        g = np.array(gains, dtype=np.float32)
        light *= (g / g.mean())[None, None, :]
    if spec.lighting == "dim":
        exposure = float(rng.uniform(*EXPOSURE["dim"]))
    else:
        exposure = float(rng.uniform(0.93, 1.08))
    return light * exposure, exposure


def render_image(assets: SceneAssets, spec: ImageSpec, max_edge: int, geometry_only: bool = False, extras: dict | None = None):
    """Photograph the scene. Returns ``(upright uint8 RGB, quad_norm float64 4x2, meta)``.

    ``geometry_only`` skips background clutter, lighting, blur and noise (only the camera, the
    page texture and a flat background): the picture `verify.geometry_check` unwarps.

    ``extras`` is for the TRAINING mode only (``trainset``): ``{"hand_p": probability}`` draws a hand
    that holds the page, from a stream of its own, so the default path (``extras=None``) draws the
    same random numbers and makes the same bytes as before.
    """
    rng = R.stream(spec.seed, "image")
    pitch, yaw, roll = _pose(spec, rng)
    tex_wh = (assets.page.ink.shape[1], assets.page.ink.shape[0])
    curl = _curl(rng, tex_wh) if spec.curl == "curled" else None
    bend_kind = getattr(spec, "bend", "")
    if bend_kind:  # curved-boundary suites (`curved.py`); the default path below is untouched
        cam, curl = _bent_camera(spec, bend_kind, tex_wh, (pitch, yaw, roll), rng, max_edge)
    else:
        tight = False
        if extras and extras.get("tight_p", 0.0) > 0.0 and spec.framing != "partial":
            tight = bool(R.stream(spec.seed, "train-tight").random() < float(extras["tight_p"]))
        cam = camera.fit(tex_wh, pitch, yaw, roll, rng, max_edge, spec.framing == "partial", curl, tight=tight, wide_partial=bool(extras and extras.get("wide_partial")))
    cw, ch = cam.canvas
    rgb, alpha, shade = camera.render(cam, assets.texture)
    quad_px = cam.quad_px()
    quad = cam.quad_norm()

    brng = R.stream(assets.spec.seed, "bg", spec.index)
    bg = backgrounds.make(assets.spec.background, brng, ch, cw)
    clutter_kinds: list[str] = []
    meta = {
        "pitch_deg": pitch,
        "yaw_deg": yaw,
        "roll_deg": roll,
        "curl": None if curl is None or bend_kind else curl.kind,
        "visible_fraction": camera.visible_fraction(quad_px, (cw, ch)),
        "backend": degrade.backend_name(),
    }
    if bend_kind:
        meta["curves"], meta["curvature_pct"] = curved.truth_curves(cam, quad)
        meta["bend"] = {k: (round(v, 4) if isinstance(v, float) else v) for k, v in vars(curl).items()}
    if geometry_only:
        bgf = np.full_like(bg, 90.0)
        out = bgf * (1 - alpha[..., None]) + rgb * alpha[..., None]
        meta["clutter_objects"] = clutter_kinds
        return np.clip(out + 0.5, 0, 255).astype(np.uint8), quad, meta

    if spec.lighting == "low-contrast":
        # A desk whose brightness sits within about 12 to 30 grey levels of the paper.
        target = np.array(assets.page.paper_rgb, dtype=np.float32) * float(rng.uniform(0.86, 0.95))
        m = float(rng.uniform(0.80, 0.93))
        bg = np.clip(target[None, None, :] * m + bg * (1 - m), 0, 255)
    if extras and extras.get("wow_p", 0.0) > 0.0:
        from . import trainset

        wrng = R.stream(spec.seed, "train-wow")
        if wrng.random() < float(extras["wow_p"]):
            bg = trainset.desk_towards_paper(bg, assets.page.paper_rgb, wrng)
            meta["white_on_white"] = True
    if spec.clutter != "none":
        clutter_kinds = backgrounds.add_clutter(bg, brng, spec.clutter, avoid=quad_px)
    centre = quad_px.mean(axis=0)
    light, exposure = _illumination(spec, rng, ch, cw, centre)

    scale = max(cw, ch) / 512.0
    # The page lifts a little off the desk: a soft contact shadow along the lower and one side.
    off = np.array([rng.uniform(-1, 1), rng.uniform(0.3, 1.0)]) * float(rng.uniform(0.004, 0.014)) * max(cw, ch)
    sh = cv2.warpAffine(alpha, np.float32([[1, 0, off[0]], [0, 1, off[1]]]), (cw, ch))
    sh = cv2.GaussianBlur(sh, (0, 0), float(rng.uniform(1.2, 4.5)) * scale)
    strength = float(rng.uniform(0.25, 0.55))
    bg = bg * (1.0 - strength * sh * (1.0 - alpha))[..., None]
    page_rgb = rgb if shade is None else rgb * shade[..., None]
    # A thin darker rim where the page ends: paper has thickness and its edge catches little light.
    inner = cv2.erode(alpha, np.ones((3, 3), np.uint8))
    rim = np.clip(alpha - inner, 0, 1)
    page_rgb = page_rgb * (1.0 - 0.10 * rim[..., None])
    hand = None
    if extras and extras.get("hand_p", 0.0) > 0.0:
        hrng = R.stream(spec.seed, "train-hand")
        if hrng.random() < float(extras["hand_p"]):
            hand = occluders.plan_hand(hrng, quad_px, (cw, ch), assets.page.size_mm)
            occluders.draw_hand(bg, hand, "behind", hrng)
    comp = bg * (1 - alpha[..., None]) + page_rgb * alpha[..., None]
    if hand is not None:
        occluders.draw_hand(comp, hand, "front", hrng)
        meta["hand"] = hand["kind"]
    if extras:
        meta["tight"] = bool(tight) if not bend_kind else False
        if assets.meta.get("mat"):
            meta["mat"] = True
    out = comp * light
    out = np.clip(out + 0.5, 0, 255).astype(np.uint8)

    if spec.lighting in ("normal", "colour-cast") and rng.random() < 0.5:
        out = degrade.lighting_gradient(out, spec.seed, rng, strength=float(rng.uniform(0.1, 0.35)))
    out = degrade.blur(out, spec.blur, rng, scale)
    out = degrade.sensor_noise(out, spec.noise, rng, exposure)
    if spec.lighting == "dim" and degrade.augraphy() is not None:
        out = degrade.low_light_noise(out, spec.seed)
    meta["clutter_objects"] = clutter_kinds
    meta["exposure"] = round(exposure, 3)
    return out, quad, meta
