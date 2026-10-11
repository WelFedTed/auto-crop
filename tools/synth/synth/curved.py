# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Curved-boundary suites (``curved-smoke``, ``curved-full``): pages whose EDGES are curved.

The default suites are untouched (the same bytes as before this module existed); this module only
runs for the two curved suites, reached through ``--suite curved-smoke|curved-full`` or
``--curved``.

A page is bent by a :class:`Bend`, a displacement of the flat texture in 3-D:

* ``cylinder``  the page is bent about an axis (a parabolic lift, with the in-plane compression of an
  inextensible sheet: the arc length stays the same);
* ``wave``      a sine lift across the page (a page held loosely, a curling receipt), same compression;
* ``corner``    one end lifts quadratically (a corner curl), same compression;
* ``bow``       a hand-held receipt: the long edges are pulled into arcs IN the page plane (the two
  edges may bow by different amounts, so the page is a banana), plus a slight cylinder lift.

The camera is the ordinary pinhole pipeline of ``camera.py`` (same pose ranges, same renderer). The
ground-truth boundary is **analytic**: the four edges of the texture rectangle are projected point by
point through the same camera and reduced to Catmull-Rom control points with the spec of
``docs/dev/curved-pages.md`` (centripetal, alpha 0.5, the first and last point are the quad corners)
until the spline is within a quarter of a pixel of the true projected edge. The shapes are plausible
and their curvature is controlled; they are NOT a physical model of paper (the bow in particular
stretches the sheet), and the fitter under test shares the "edges are smooth arcs" assumption with
them. A synthetic result says how the fitter works on such pages, nothing about real photographs.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, replace

import numpy as np

from . import plan
from . import rng as R

# Curvature of an edge: the largest distance of the edge from its chord, in percent of the chord
# length. A page's curvature is the largest of its four edges.
BUCKETS = (("slight", 0.0, 0.6), ("mild", 0.6, 1.5), ("moderate", 1.5, 3.5), ("strong", 3.5, 1e9))
# What the amplitude is calibrated to (percent), per planned bucket; jittered by 0.8 to 1.25.
BUCKET_TARGET = {"mild": 1.0, "moderate": 2.4, "strong": 5.0}
BUCKET_QUOTA = {"mild": 0.34, "moderate": 0.36, "strong": 0.30}
BEND_QUOTA = {"cylinder": 0.25, "wave": 0.20, "corner": 0.15, "bow": 0.40}
ASPECT_QUOTA = {"document": 0.30, "receipt": 0.40, "long": 0.23, "strip": 0.07}
FIT_TOLERANCE_PX = 0.25  # how closely the ground-truth spline follows the projected edge
SAMPLES_PER_EDGE = 129

SUITES = {
    "curved-smoke": {
        "count": 150,
        "seed": 0x5A0C_0C01,
        "max_edge": 512,
        "overrides": {"format": {"jpeg": 0.62, "png": 0.06, "tiff": 0.06, "webp": 0.26}},
    },
    "curved-full": {"count": 1200, "seed": 0x5A0C_0C02, "max_edge": 512, "overrides": {}},
}


def bucket_of(pct: float) -> str:
    for name, lo, hi in BUCKETS:
        if lo <= pct < hi:
            return name
    return BUCKETS[-1][0]


# --------------------------------------------------------------------------------------- the bend


def _compression(profile, half: float, n: int = 513):
    """Table ``(p, shift)`` of the in-plane shift along the bend direction that keeps arc length:
    a sheet lifted by ``profile(p)`` over ``p`` in ``[-half, half]`` is shortened by
    ``integral(sqrt(1 - z'^2)) - p`` (zero at the centre)."""
    p = np.linspace(-half, half, n)
    z = profile(p)
    dz = np.clip(np.gradient(z, p), -0.92, 0.92)
    f = np.sqrt(1.0 - dz * dz)
    arc = np.concatenate([[0.0], np.cumsum(0.5 * (f[1:] + f[:-1]) * np.diff(p))])
    arc -= np.interp(0.0, p, arc)
    return p, arc - p


@dataclass(frozen=True)
class Bend:
    """A smooth displacement of the flat page (texture pixels); same interface as ``camera.Curl``
    plus ``shift`` (in-plane) and ``newton_iters`` (the renderer inverts the map by Newton steps)."""

    kind: str  # cylinder | wave | corner | bow
    amp: float  # peak lift towards the camera (px)
    angle: float  # direction of the bend (radians)
    freq: float = 1.5  # wave: cycles across the page
    phase: float = 0.0
    onset: float = 0.5  # corner: where the lift starts, 0..1
    bow: float = 0.0  # bow: peak in-plane displacement of the long edges (px)
    asym: float = 0.0  # bow: the two long edges bow by (1 + asym) and (1 - asym)
    side: float = 0.0  # bow: peak displacement of the short edges (px), across the page
    newton_iters: int = 9

    def scaled(self, k: float) -> "Bend":
        return replace(self, amp=self.amp * k, bow=self.bow * k, side=self.side * k)

    def _u(self, x, y, w, h):
        c, s = math.cos(self.angle), math.sin(self.angle)
        half = 0.5 * (abs(w * c) + abs(h * s))
        return (x * c + y * s) / half, half, c, s

    def _profile(self, u):
        if self.kind in ("cylinder", "bow"):
            return self.amp * (1.0 - np.clip(u, -1, 1) ** 2)
        if self.kind == "wave":
            return self.amp * np.sin(2 * math.pi * self.freq * (np.clip(u, -1, 1) + 1.0) / 2.0 + self.phase)
        t = np.clip((u - (2 * self.onset - 1)) / (2 - (2 * self.onset - 1)), 0.0, 1.0)
        return self.amp * t**2

    def lift(self, x, y, w, h):
        u, _, _, _ = self._u(x, y, w, h)
        return self._profile(u)

    def shift(self, x, y, w, h):
        u, half, c, s = self._u(x, y, w, h)
        p, comp = _compression(lambda q: self._profile(q / half), half)
        d = np.interp(u * half, p, comp)
        dx, dy = d * c, d * s
        if self.kind == "bow" and (self.bow or self.side):
            sx, sy = (x + w / 2) / w, (y + h / 2) / h
            # The long edges are the horizontal ones when the page is wider than tall.
            if w >= h:
                f = 4.0 * sx * (1.0 - sx)
                dy = dy + self.bow * f * ((1.0 + self.asym) * (1.0 - sy) + (1.0 - self.asym) * sy)
                dx = dx + self.side * 4.0 * sy * (1.0 - sy)
            else:
                f = 4.0 * sy * (1.0 - sy)
                dx = dx + self.bow * f * ((1.0 + self.asym) * (1.0 - sx) + (1.0 - self.asym) * sx)
                dy = dy + self.side * 4.0 * sx * (1.0 - sx)
        return dx, dy


def draw_bend(rng, kind: str, tex: tuple[int, int]) -> Bend:
    """A bend of the given kind for a page of ``tex`` pixels; amplitudes are first guesses that
    ``calibrate`` rescales to the planned curvature."""
    w, h = tex
    short, long_ = float(min(tex)), float(max(tex))
    angle = float(rng.uniform(0.0, 2 * math.pi))
    if kind == "cylinder":
        return Bend("cylinder", float(rng.uniform(0.06, 0.2)) * short, angle)
    if kind == "wave":
        return Bend(
            "wave",
            float(rng.uniform(0.03, 0.1)) * short,
            angle,
            freq=float(rng.uniform(1.0, 2.4)),
            phase=float(rng.uniform(0, 2 * math.pi)),
        )
    if kind == "corner":
        # Towards one of the four corners (the diagonal), a little off.
        diag = math.atan2(h, w) * (1 if rng.random() < 0.5 else -1) + (0 if rng.random() < 0.5 else math.pi)
        return Bend("corner", float(rng.uniform(0.1, 0.3)) * short, diag + float(rng.uniform(-0.25, 0.25)), onset=float(rng.uniform(0.35, 0.7)))
    if kind == "bow":
        # The bow acts along the longer side of the page; the lift is a gentle cylinder about it.
        axis = 0.0 if w >= h else math.pi / 2
        return Bend(
            "bow",
            float(rng.uniform(0.02, 0.07)) * short,
            axis + float(rng.uniform(-0.1, 0.1)),
            bow=float(rng.uniform(0.03, 0.08)) * long_,
            asym=float(rng.uniform(-0.45, 0.45)),
            side=float(rng.uniform(0.0, 0.02)) * short * float(rng.choice([-1.0, 1.0])),
        )
    raise ValueError(f"unknown bend kind {kind!r}")


# ------------------------------------------------------------------------------- the ground truth


def _lerp(a, b, t):
    return a + (b - a) * t


def cr_polyline(points: np.ndarray, per_segment: int = 64) -> np.ndarray:
    """The centripetal Catmull-Rom spline of ``docs/dev/curved-pages.md`` (alpha 0.5, Barry-Goldman,
    phantom end points ``2 P0 - P1``), ``per_segment`` steps per segment, the knots exact."""
    p = np.asarray(points, dtype=np.float64)
    n = len(p)
    if n == 2:
        t = np.linspace(0.0, 1.0, per_segment + 1)[:, None]
        return p[0] + (p[1] - p[0]) * t
    out = [p[0]]
    for seg in range(n - 1):
        p1, p2 = p[seg], p[seg + 1]
        p0 = 2 * p[0] - p[1] if seg == 0 else p[seg - 1]
        p3 = 2 * p[-1] - p[-2] if seg + 2 >= n else p[seg + 2]
        knot = lambda a, b: max(math.sqrt(float(np.hypot(*(a - b)))), 1e-9)  # noqa: E731
        t0 = 0.0
        t1 = t0 + knot(p0, p1)
        t2 = t1 + knot(p1, p2)
        t3 = t2 + knot(p2, p3)
        for k in range(1, per_segment + 1):
            if k == per_segment:
                out.append(p2)
                continue
            t = t1 + (k / per_segment) * (t2 - t1)

            def mix(a, b, ta, tb):
                d = tb - ta
                return ((tb - t) / d) * a + ((t - ta) / d) * b

            a1, a2, a3 = mix(p0, p1, t0, t1), mix(p1, p2, t1, t2), mix(p2, p3, t2, t3)
            b1, b2 = mix(a1, a2, t0, t2), mix(a2, a3, t1, t3)
            out.append(mix(b1, b2, t1, t2))
    return np.array(out)


def _dist_to_polyline(pts_px: np.ndarray, poly_px: np.ndarray) -> np.ndarray:
    """Distance of each of ``pts_px`` to the polyline ``poly_px`` (vectorised, exact)."""
    a, b = poly_px[:-1], poly_px[1:]
    ab = b - a
    l2 = np.maximum((ab * ab).sum(axis=1), 1e-18)
    d = pts_px[:, None, :] - a[None, :, :]
    t = np.clip((d * ab[None]).sum(axis=2) / l2[None], 0.0, 1.0)
    proj = a[None] + t[..., None] * ab[None]
    return np.sqrt(((pts_px[:, None, :] - proj) ** 2).sum(axis=2)).min(axis=1)


def fit_control_points(dense_norm: np.ndarray, size: tuple[int, int], tol_px: float = FIT_TOLERANCE_PX, max_points: int = 32):
    """Control points (a subset of ``dense_norm``, in order, ends included) whose Catmull-Rom spline is
    within ``tol_px`` of the dense edge. Greedy: add the worst-fitted point until it fits."""
    scale = np.array(size, dtype=np.float64)
    dense_px = dense_norm * scale
    idx = [0, len(dense_norm) - 1]
    while True:
        spline = cr_polyline(dense_norm[idx]) * scale
        err = _dist_to_polyline(dense_px, spline)
        worst = int(np.argmax(err))
        if err[worst] <= tol_px or len(idx) >= max_points or worst in idx:
            return dense_norm[idx]
        idx = sorted(idx + [worst])


def edge_samples(cam, n: int = SAMPLES_PER_EDGE) -> list[np.ndarray]:
    """The page edges top, right, bottom, left (corner to corner, clockwise) projected through the
    camera, in canvas pixels, ``n`` points each (both end points included)."""
    wt, ht = cam.tex
    u = np.linspace(0.0, 1.0, n)
    hw, hh = wt / 2.0, ht / 2.0
    runs = [
        (-hw + u * wt, np.full(n, -hh)),  # top: TL -> TR
        (np.full(n, hw), -hh + u * ht),  # right: TR -> BR
        (hw - u * wt, np.full(n, hh)),  # bottom: BR -> BL
        (np.full(n, -hw), hh - u * ht),  # left: BL -> TL
    ]
    out = []
    for x, y in runs:
        px, py = cam.project(x, y)
        out.append(np.c_[px, py])
    return out


def curvature_pct(edge_px: np.ndarray) -> float:
    """Largest distance of an edge from its chord, in percent of the chord length."""
    a, b = edge_px[0], edge_px[-1]
    ab = b - a
    length = float(np.hypot(*ab))
    if length < 1e-9:
        return 0.0
    d = np.abs(ab[0] * (edge_px[:, 1] - a[1]) - ab[1] * (edge_px[:, 0] - a[0])) / length
    return float(100.0 * d.max() / length)


def truth_curves(cam, quad_norm: np.ndarray):
    """``(curves, curvature)``: the manifest ``curves`` (only the edges that are bent, 9 decimals; an
    absent edge is straight) and the curvature of the four edges in percent of their chord."""
    cw, ch = cam.canvas
    scale = np.array([cw, ch], dtype=np.float64)
    names = ("top", "right", "bottom", "left")
    corners = [quad_norm[0], quad_norm[1], quad_norm[2], quad_norm[3], quad_norm[0]]
    curves: dict = {}
    pct = []
    for i, edge in enumerate(edge_samples(cam)):
        pct.append(curvature_pct(edge))
        dense = edge / scale
        dense[0], dense[-1] = corners[i], corners[i + 1]  # the corners are exactly the quad's
        pts = fit_control_points(dense, (cw, ch))
        if len(pts) > 2:
            curves[names[i]] = [[round(float(x), 9), round(float(y), 9)] for x, y in pts]
    return curves, [round(v, 4) for v in pct]


# ------------------------------------------------------------------------------------- the plan


def build(name: str, seed: int, count: int, overrides: dict | None = None):
    """Scenes and images of a curved suite: the ordinary plan with every page full-framed and flat
    (no ``curl``), each image given a bend kind and a planned curvature bucket by balanced shuffles
    of their own (``bend``, ``curvature`` streams), so the default plan is never touched."""
    ov = {
        "aspect": ASPECT_QUOTA,
        "framing": {"full": 1.0},
        "curl": {"flat": 1.0},
        **(overrides or {}),
    }
    scenes, images = plan.build(name, seed, count, ov)
    kinds = plan.balanced(R.stream(seed, "image-axis", "bend"), count, BEND_QUOTA)
    buckets = plan.balanced(R.stream(seed, "image-axis", "curvature"), count, BUCKET_QUOTA)
    images = [replace(im, bend=kinds[i], curvature=buckets[i]) for i, im in enumerate(images)]
    return scenes, images


def outline_is_simple(cam) -> bool:
    """The projected page outline does not cross itself (a very strong wave can fold an edge back
    over its neighbour; such a page has no valid ground-truth curves)."""
    poly = np.concatenate([e[:-1] for e in edge_samples(cam, 65)])
    a, b = poly, np.roll(poly, -1, axis=0)
    n = len(poly)

    def ccw(p, q, r):
        return (r[..., 1] - p[..., 1]) * (q[..., 0] - p[..., 0]) - (q[..., 1] - p[..., 1]) * (r[..., 0] - p[..., 0])

    i, j = np.triu_indices(n, k=2)
    keep = ~((i == 0) & (j == n - 1))  # the first and last segments share a vertex
    i, j = i[keep], j[keep]
    d1, d2 = ccw(a[i], b[i], a[j]), ccw(a[i], b[i], b[j])
    d3, d4 = ccw(a[j], b[j], a[i]), ccw(a[j], b[j], b[i])
    return not bool(np.any((d1 * d2 < 0) & (d3 * d4 < 0)))


def calibrate(tex: tuple[int, int], make_cam, bend: Bend, target: float, jitter: float):
    """Rescale ``bend`` until the page's curvature in the picture is within 12% of ``target`` times
    ``jitter`` (at most four rounds). A page that cannot reach it, such as a cylinder seen flat-on,
    stops at the amplitude cap and stays below it. ``make_cam(bend)`` frames and returns the
    camera. Returns ``(bend, camera)``."""
    want = target * jitter
    cap = 0.45 * min(tex)
    cur = bend
    for _ in range(4):
        cam = make_cam(cur)
        got = max(curvature_pct(e) for e in edge_samples(cam))
        if abs(got - want) <= 0.12 * want:
            break
        step = float(np.clip(want / max(got, 0.02), 0.25, 4.0))
        peak = max(abs(cur.amp), abs(cur.bow), abs(cur.side), 1e-9)
        step = min(step, cap / peak)  # never beyond the cap
        if abs(step - 1.0) < 1e-6:
            break
        cur = cur.scaled(step)
    cam = make_cam(cur)
    for _ in range(8):  # a fold is eased off until the outline is a simple curve
        if outline_is_simple(cam):
            break
        cur = cur.scaled(0.8)
        cam = make_cam(cur)
    return cur, cam


def fresh(rng):
    """A copy of ``rng`` in the same state (the camera consumes random numbers, so each calibration
    round starts from the state the first one saw)."""
    twin = np.random.Generator(type(rng.bit_generator)())
    twin.bit_generator.state = rng.bit_generator.state
    return twin
