# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Acceptance checks for the generator itself (ROADMAP M1.31, M1.32, M1.34).

* ``check-geometry``: unwarp the render with only its ground-truth quad and compare with the clean
  page (SSIM). The ground truth is analytic, so this fails if the camera math, the corner order or
  the edge-coordinate convention ever drifts from what the render does.
* ``check-ocr``: Tesseract 5 character error rate on the clean render of known text. Needs a
  ``tesseract`` binary; CI runs it where one exists (the devcontainer job).
* ``variants``: write every format x EXIF orientation x colour-space variant of two upright
  pictures, with the reference each must decode to, for the Rust decoder check
  (``auto-crop-eval check-variants``).
* ``contact-sheet``: a grid of N pictures with their ground-truth quads drawn, for the human review
  that ROADMAP M1.33 and M1.37 ask for.
* ``quotas``: tag histogram of a manifest against the slice quotas of the full suite.
* ``licences``: licences of the installed Python packages; fails on a copyleft or non-commercial one.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import importlib.metadata as md
import io
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

from . import camera, degrade, encode, icc, page as pagemod, plan, scene
from . import rng as R

SSIM_MIN = 0.98  # PROVISIONAL (ROADMAP M1.32)
CER_MAX = 0.05  # PROVISIONAL (ROADMAP M1.31)


# ---------------------------------------------------------------------------------------------
# Geometry
# ---------------------------------------------------------------------------------------------


def clean_assets(spec: plan.SceneSpec) -> scene.SceneAssets:
    """The scene with a clean page texture (no paper grain, no ink bleed): the reference render."""
    rng = R.stream(spec.seed, "page")
    pg = pagemod.render(rng, spec.aspect, spec.paper)
    rgb, used = degrade.compose_page(pg, rng, None)
    return scene.SceneAssets(spec, pg, rgb, used, None)


def unwarp_ssim(image_rgb: np.ndarray, quad: np.ndarray, reference_rgb: np.ndarray) -> tuple[float, tuple[int, int]]:
    """SSIM between the render unwarped with ``quad`` alone and the clean page at the same scale.

    The page rectangle is sized from the quad's own side lengths, so the comparison is at the
    picture's resolution. Both sides are low-passed (Gaussian, sigma 1.5 px) first: a picture of
    1.6 pixels per text stroke cannot hold the stroke's exact profile, and the check is about where
    the page is, not about resampling loss. A 0.5 px error of the whole quad already costs about
    two points of SSIM and a 1 px error of one corner more than that (`tests/test_geometry.py`).
    """
    from skimage.metrics import structural_similarity

    h, w = image_rgb.shape[:2]
    q = quad * np.array([w, h])
    ow = int(round((np.linalg.norm(q[1] - q[0]) + np.linalg.norm(q[2] - q[3])) / 2))
    oh = int(round((np.linalg.norm(q[3] - q[0]) + np.linalg.norm(q[2] - q[1])) / 2))
    gray = cv2.cvtColor(image_rgb, cv2.COLOR_RGB2GRAY)
    un = cv2.GaussianBlur(camera.inverse_warp(gray, quad, (ow, oh)), (0, 0), 1.5)
    ref = cv2.GaussianBlur(
        cv2.resize(cv2.cvtColor(reference_rgb, cv2.COLOR_RGB2GRAY), (ow, oh), interpolation=cv2.INTER_AREA), (0, 0), 1.5
    )
    m = max(2, int(0.01 * min(ow, oh)))  # the page rim is excluded: desk mixes into edge pixels
    s = structural_similarity(un[m:-m, m:-m], ref[m:-m, m:-m], data_range=255, gaussian_weights=True, sigma=1.5, use_sample_covariance=False)
    return float(s), (ow, oh)


def geometry_samples(count: int, seed: int, max_edge: int = 640, curl: bool = False):
    """Yield (image_id, ssim, tilt, aspect, rect) for the flat (or curled) full-frame renders of a plan."""
    scenes, images = plan.build("geo", seed, count)
    cache: dict[int, scene.SceneAssets] = {}
    for im in images:
        if im.framing != "full" or (im.curl == "curled") != curl:
            continue
        a = cache.setdefault(im.scene.index, clean_assets(im.scene))
        img, quad, meta = scene.render_image(a, im, max_edge, geometry_only=True)
        s, rect = unwarp_ssim(img, quad, a.texture)
        yield im.image_id, s, im.tilt, im.scene.aspect, rect


def check_geometry(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="python -m synth check-geometry")
    ap.add_argument("--count", type=int, default=60)
    ap.add_argument("--seed", type=int, default=20261003)
    ap.add_argument("--min-ssim", type=float, default=SSIM_MIN)
    ap.add_argument("--curl", action="store_true", help="check curled pages (reported, not gated)")
    a = ap.parse_args(argv)
    rows = list(geometry_samples(a.count, a.seed, curl=a.curl))
    if not rows:
        print("no samples", file=sys.stderr)
        return 2
    vals = np.array([r[1] for r in rows])
    by_tilt: dict[str, list[float]] = {}
    for r in rows:
        by_tilt.setdefault(r[2], []).append(r[1])
    print(f"inverse-warp SSIM over {len(rows)} renders: min {vals.min():.4f} median {np.median(vals):.4f} mean {vals.mean():.4f}")
    for k in sorted(by_tilt):
        print(f"  tilt {k:6s}: n={len(by_tilt[k]):3d} min {min(by_tilt[k]):.4f} mean {np.mean(by_tilt[k]):.4f}")
    worst = sorted(rows, key=lambda r: r[1])[:3]
    for r in worst:
        print(f"  lowest: {r[0]} {r[3]} {r[2]} ssim {r[1]:.4f} rect {r[4]}")
    if a.curl:
        print("curled pages are not planar, so a four-corner unwarp cannot match; shown for information")
        return 0
    if vals.min() < a.min_ssim:
        print(f"FAIL: minimum SSIM {vals.min():.4f} < {a.min_ssim} (PROVISIONAL threshold, ROADMAP M1.32)", file=sys.stderr)
        return 1
    print(f"ok: every SSIM >= {a.min_ssim}")
    return 0


# ---------------------------------------------------------------------------------------------
# OCR
# ---------------------------------------------------------------------------------------------


def levenshtein(a: str, b: str) -> int:
    """Edit distance by Hyyro's bit-parallel algorithm (Python integers as bit vectors)."""
    m = len(a)
    if m == 0:
        return len(b)
    peq: dict[str, int] = {}
    for i, ch in enumerate(a):
        peq[ch] = peq.get(ch, 0) | (1 << i)
    mask = (1 << m) - 1
    last = 1 << (m - 1)
    pv, mv, score = mask, 0, m
    for ch in b:
        eq = peq.get(ch, 0)
        xv = eq | mv
        xh = (((eq & pv) + pv) ^ pv) | eq
        ph = mv | (~(xh | pv) & mask)
        mh = pv & xh
        if ph & last:
            score += 1
        elif mh & last:
            score -= 1
        ph = ((ph << 1) | 1) & mask
        mh = (mh << 1) & mask
        pv = (mh | (~(xv | ph) & mask)) & mask
        mv = ph & xv
    return score


def normalise(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def cer(reference: str, hypothesis: str) -> float:
    ref, hyp = normalise(reference), normalise(hypothesis)
    return levenshtein(ref, hyp) / max(1, len(ref))


def tesseract_version() -> str | None:
    exe = shutil.which("tesseract")
    if not exe:
        return None
    out = subprocess.run([exe, "--version"], capture_output=True, text=True)
    return (out.stdout or out.stderr).splitlines()[0] if (out.stdout or out.stderr) else "unknown"


def check_ocr(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="python -m synth check-ocr")
    ap.add_argument("--count", type=int, default=24)
    ap.add_argument("--seed", type=int, default=20261003)
    ap.add_argument("--max-cer", type=float, default=CER_MAX)
    ap.add_argument("--require", action="store_true", help="fail (exit 3) instead of skipping when tesseract is missing")
    ap.add_argument("--keep", help="write the rendered pages and OCR text to this directory")
    a = ap.parse_args(argv)
    ver = tesseract_version()
    if ver is None:
        msg = "tesseract not found on PATH: the CER acceptance (ROADMAP M1.31) was NOT run"
        print(("FAIL: " if a.require else "SKIPPED: ") + msg, file=sys.stderr)
        return 3 if a.require else 0
    if not re.search(r"\b5\.\d", ver):
        print(f"warning: ROADMAP M1.31 asks for Tesseract 5; found {ver}", file=sys.stderr)
    kinds = ["document", "receipt", "long", "strip"]
    results: dict[str, list[float]] = {k: [] for k in kinds}
    keep = Path(a.keep) if a.keep else None
    if keep:
        keep.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for i in range(a.count):
            kind = kinds[i % len(kinds)]
            rng = R.stream(a.seed, "ocr", i)
            # 8 px per mm is about 203 dpi, the resolution of a thermal print head and a plain scan.
            pg = pagemod.render(rng, kind, "white", ppm=8.0)
            gray = pg.clean_gray()
            path = Path(tmp) / f"p{i}.png"
            cv2.imwrite(str(path), gray)
            psm = "3" if kind == "document" else "6"
            r = subprocess.run(
                ["tesseract", str(path), "stdout", "-l", "eng", "--psm", psm],
                capture_output=True, text=True, encoding="utf8", errors="replace",
            )
            if r.returncode != 0:
                print(f"tesseract failed on page {i}: {r.stderr[:200]}", file=sys.stderr)
                return 2
            c = cer(pg.text, r.stdout)
            results[kind].append(c)
            if keep:
                shutil.copy(path, keep / f"p{i}.png")
                (keep / f"p{i}.ref.txt").write_text(pg.text, encoding="utf8")
                (keep / f"p{i}.ocr.txt").write_text(r.stdout, encoding="utf8")
    print(f"{ver}; CER on the clean render (8 px/mm), threshold {a.max_cer:.0%} (PROVISIONAL)")
    bad = False
    for k in kinds:
        v = results[k]
        if not v:
            continue
        print(f"  {k:9s} n={len(v):3d} mean {np.mean(v):.4f} max {max(v):.4f}")
        bad |= float(np.mean(v)) > a.max_cer
    allv = [x for v in results.values() for x in v]
    print(f"  all       n={len(allv):3d} mean {np.mean(allv):.4f}")
    if bad:
        print("FAIL: mean CER above the threshold for at least one page kind", file=sys.stderr)
        return 1
    print("ok")
    return 0


# ---------------------------------------------------------------------------------------------
# Variants for the Rust decoder check
# ---------------------------------------------------------------------------------------------


def make_variants(out: Path, seed: int = 20261003, max_edge: int = 200) -> int:
    """Every format x orientation x colour space of two pictures, plus references."""
    out.mkdir(parents=True, exist_ok=True)
    scenes, images = plan.build("var", seed, 6)
    picks = [images[0], images[3]]  # two scenes
    rows = []
    srgb_icc = encode.srgb_profile()
    for k, spec in enumerate(picks):
        a = scene.build_scene(spec.scene)
        flat = dataclasses.replace(spec, lighting="normal", curl="flat", framing="full", blur="sharp", noise="low")
        img, quad, meta = scene.render_image(a, flat, max_edge)
        h, w = img.shape[:2]
        refs = {"srgb": img, "display-p3": icc.srgb_to_p3(img)}
        for cs, arr in refs.items():
            Image.fromarray(arr).save(out / f"ref-{k}-{cs}.png")
        for fmt in encode.FORMATS:
            for lossless in ([False, True] if fmt == "webp" else [False]):
                for o in range(1, 9):
                    for cs in ("srgb", "display-p3"):
                        embed = cs == "srgb" and o % 2 == 0
                        data = encode.encode(img, fmt, 95, o, cs, embed_srgb=embed, lossless_webp=lossless)
                        tag = f"{fmt}{'-lossless' if lossless else ''}"
                        name = f"v{k}-{tag}-o{o}-{cs}.{encode.EXT[fmt]}"
                        (out / name).write_bytes(data)
                        is_lossless = fmt in ("png", "tiff") or lossless
                        ref = refs[cs]
                        dec = np.asarray(Image.open(io.BytesIO(data)).convert("RGB"))
                        # Pillow leaves the tag alone for JPEG, PNG and WebP but its libtiff path turns
                        # TIFF pixels itself, so do not turn it twice.
                        shown = dec if fmt == "tiff" else encode.display(dec, o)
                        err = float(np.abs(shown.astype(np.int16) - ref.astype(np.int16)).mean())
                        profile = icc.display_p3_profile() if cs == "display-p3" else (srgb_icc if embed else None)
                        rows.append(
                            {
                                "file": name,
                                "format": fmt,
                                "orientation": o,
                                "colorspace": cs,
                                "lossless": is_lossless,
                                "reference": f"ref-{k}-{cs}.png",
                                "width": w,
                                "height": h,
                                "icc_sha256": hashlib.sha256(profile).hexdigest() if profile else None,
                                "reference_error": round(err, 4),
                                "max_mean_abs_err": 0.0 if is_lossless else round(max(3.0, 2.0 * err), 3),
                            }
                        )
    (out / "variants.jsonl").write_text("".join(json.dumps(r, separators=(",", ":")) + "\n" for r in rows), encoding="utf8")
    bad = [r for r in rows if r["lossless"] and r["reference_error"] != 0.0]
    if bad:
        print(f"internal error: {len(bad)} lossless variants do not reproduce their reference in PIL", file=sys.stderr)
        return 1
    print(f"wrote {len(rows)} variants and {len(refs) * len(picks)} references to {out}")
    return 0


# ---------------------------------------------------------------------------------------------
# Contact sheet
# ---------------------------------------------------------------------------------------------


def contact_sheet(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="python -m synth contact-sheet")
    ap.add_argument("manifest")
    ap.add_argument("--out", required=True)
    ap.add_argument("--count", type=int, default=100)
    ap.add_argument("--cell", type=int, default=176)
    ap.add_argument("--columns", type=int, default=10)
    a = ap.parse_args(argv)
    from PIL import ImageOps

    base = Path(a.manifest).parent
    rows = [json.loads(l) for l in Path(a.manifest).read_text(encoding="utf8").splitlines() if l.strip()]
    step = max(1, len(rows) // a.count)
    picks = rows[::step][: a.count]
    cell, tiles = a.cell, []
    for r in picks:
        im = ImageOps.exif_transpose(Image.open(base / r["image"])).convert("RGB")
        arr = np.asarray(im).copy()
        h, w = arr.shape[:2]
        q = np.round(np.array(r["quad"]) * [w, h]).astype(np.int32)
        cv2.polylines(arr, [q], True, (255, 40, 40), 2, cv2.LINE_AA)
        cv2.circle(arr, tuple(q[0]), 5, (40, 255, 40), -1)
        k = min(cell / w, cell / h)
        arr = cv2.resize(arr, (max(1, int(w * k)), max(1, int(h * k))), interpolation=cv2.INTER_AREA)
        tile = np.zeros((cell, cell, 3), np.uint8)
        tile[: arr.shape[0], : arr.shape[1]] = arr
        t = r["tags"]
        cv2.putText(tile, f"{r['id'][-5:]} {t['aspect'][:3]} {t['lighting'][:4]} {t['tilt']}", (2, cell - 4), cv2.FONT_HERSHEY_SIMPLEX, 0.32, (255, 255, 0), 1, cv2.LINE_AA)
        tiles.append(tile)
    cols = a.columns
    while len(tiles) % cols:
        tiles.append(np.zeros_like(tiles[0]))
    grid = np.vstack([np.hstack(tiles[i : i + cols]) for i in range(0, len(tiles), cols)])
    Image.fromarray(grid).save(a.out, quality=88) if a.out.endswith(".jpg") else Image.fromarray(grid).save(a.out)
    print(f"wrote {a.out}: {len(picks)} pictures, quads in red, top-left corner in green")
    return 0


# ---------------------------------------------------------------------------------------------
# Quotas and licences
# ---------------------------------------------------------------------------------------------


def check_quotas(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="python -m synth quotas")
    ap.add_argument("manifest")
    ap.add_argument("--min", type=int, default=plan.QUOTA_MIN_FULL, help="images required per tag value")
    a = ap.parse_args(argv)
    rows = [json.loads(line) for line in Path(a.manifest).read_text(encoding="utf8").splitlines() if line.strip()]
    hist = plan.histogram([r["tags"] for r in rows])
    for axis in sorted(hist):
        print(f"{axis:11s} " + "  ".join(f"{v}={n}" for v, n in sorted(hist[axis].items())))
    problems = plan.check_quotas([r["tags"] for r in rows], a.min)
    for p in problems:
        print("QUOTA", p, file=sys.stderr)
    print(f"{len(rows)} images; " + ("quotas met" if not problems else f"{len(problems)} cells under {a.min}"))
    return 1 if problems else 0


BAD_LICENCE = re.compile(r"(?<!L)GPL|AGPL|Non-?Commercial|\bCC[- ]BY-NC|Commons Clause|SSPL", re.I)
BANNED_PACKAGES = ("albumentations", "albumentationsx")


def licence_of(dist: md.Distribution) -> str:
    m = dist.metadata
    parts = [m.get("License-Expression") or "", m.get("License") or ""]
    parts += [c.split("::")[-1].strip() for c in (m.get_all("Classifier") or []) if c.startswith("License ::")]
    text = "; ".join(p for p in parts if p and p.strip() and p.upper() != "UNKNOWN")
    return " ".join(text.split())[:160] or "UNDECLARED"


def check_licences(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="python -m synth licences")
    ap.add_argument("--lock", default=str(Path(__file__).resolve().parent.parent / "requirements.lock"))
    a = ap.parse_args(argv)
    bad = 0
    names = []
    lock = Path(a.lock)
    lock_names = []
    if lock.exists():
        lock_names = re.findall(r"^([A-Za-z0-9_.\-]+)==", lock.read_text(encoding="utf8"), re.M)
    for dist in sorted(md.distributions(), key=lambda d: (d.metadata["Name"] or "").lower()):
        name = dist.metadata["Name"] or "?"
        if lock_names and name.lower().replace("_", "-") not in {n.lower().replace("_", "-") for n in lock_names} and name.lower() not in ("pip", "setuptools", "wheel"):
            continue
        lic = licence_of(dist)
        flag = ""
        if BAD_LICENCE.search(lic) or name.lower() in BANNED_PACKAGES:
            flag = "  <-- NOT ALLOWED"
            bad += 1
        names.append(name)
        print(f"{name:28s} {dist.version:14s} {lic}{flag}")
    banned = [n for n in lock_names if n.lower() in BANNED_PACKAGES]
    for n in banned:
        print(f"banned package in requirements.lock: {n}", file=sys.stderr)
        bad += 1
    print(f"{len(names)} packages checked")
    return 1 if bad else 0


def main(cmd: str, argv: list[str]) -> int:
    if cmd == "check-geometry":
        return check_geometry(argv)
    if cmd == "check-ocr":
        return check_ocr(argv)
    if cmd == "variants":
        ap = argparse.ArgumentParser(prog="python -m synth variants")
        ap.add_argument("--out", required=True)
        ap.add_argument("--seed", type=int, default=20261003)
        a = ap.parse_args(argv)
        return make_variants(Path(a.out), a.seed)
    if cmd == "quotas":
        return check_quotas(argv)
    if cmd == "contact-sheet":
        return contact_sheet(argv)
    if cmd == "licences":
        return check_licences(argv)
    raise SystemExit(f"unknown subcommand {cmd}")
