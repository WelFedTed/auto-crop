# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Generate a suite: images, ``manifest.jsonl`` (v1) and ground-truth sidecars."""

from __future__ import annotations

import hashlib
import json
import multiprocessing as mp
import os
import shutil
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from concurrent.futures.process import BrokenProcessPool
from pathlib import Path

import cv2
import numpy as np

from . import GENERATOR, MANIFEST_VERSION, backgrounds, degrade, encode, fonts, plan, scene

LICENCE = "MIT OR Apache-2.0"


def _r9(a) -> list:
    return [[round(float(x), 9) for x in p] for p in a]


def _crop_box(quad: np.ndarray) -> list[float]:
    """Axis-aligned box of the page quad clipped to the frame (normalised x0, y0, x1, y1)."""
    x0, y0 = np.clip(quad.min(axis=0), 0, 1)
    x1, y1 = np.clip(quad.max(axis=0), 0, 1)
    return [round(float(v), 9) for v in (x0, y0, x1, y1)]


def _truth(assets: scene.SceneAssets, spec: plan.ImageSpec, quad: np.ndarray, meta: dict) -> dict:
    pg = assets.page
    return {
        "id": spec.image_id,
        "scene_id": spec.scene.scene_id,
        "text": pg.text,
        "lines": pg.lines,
        "amounts": pg.amounts,
        "page": {
            "kind": pg.kind,
            "layout": pg.layout,
            "font_family": pg.font_family,
            "font_licence": fonts.licences()[pg.font_family],
            "size_mm": list(pg.size_mm),
            "ppm": pg.ppm,
            "thermal": pg.thermal,
            "paper_rgb": list(pg.paper_rgb),
            "ink_rgb": list(pg.ink_rgb),
            "ink_contrast": round(assets.ink_contrast, 4),
            **pg.extra,
        },
        "camera": {k: meta[k] for k in ("pitch_deg", "yaw_deg", "roll_deg", "curl")},
        "quad": _r9(quad),
    }


def _render_scene(task):
    """Worker: render every image of one scene and write its files; returns manifest rows."""
    sc, imgs, out, max_edge, truth_mode, suite = task
    out = Path(out)
    assets = scene.build_scene(sc)
    rows = []
    for spec in imgs:
        img, quad, meta = scene.render_image(assets, spec, max_edge)
        data = encode.encode(
            img,
            spec.format,
            spec.jpeg_quality,
            spec.exif,
            spec.colorspace,
            embed_srgb=(spec.index % 10 < 3),
        )
        rel = f"images/{spec.image_id}.{encode.EXT[spec.format]}"
        (out / rel).write_bytes(data)
        h, w = img.shape[:2]
        # `width`/`height` are those of the oriented image, like the quad.
        row = {
            "v": MANIFEST_VERSION,
            "id": spec.image_id,
            "image": rel,
            "scene_id": sc.scene_id,
            "split": sc.split,
            "width": int(w),
            "height": int(h),
            "quad": _r9(quad),
            "tags": spec.tags(),
            "generator": GENERATOR,
            "suite": suite,
            "scene_seed": sc.seed,
            "image_seed": spec.seed,
            "exif_orientation": spec.exif,
            "rotation_deg": round(meta["roll_deg"], 4),
            "pitch_deg": round(meta["pitch_deg"], 4),
            "yaw_deg": round(meta["yaw_deg"], 4),
            "crop_box": _crop_box(quad),
            "item_count": 1,
            "visible_fraction": round(meta["visible_fraction"], 4),
            "page_mm": [round(v, 2) for v in assets.page.size_mm],
            "page_aspect": round(max(assets.page.size_mm) / min(assets.page.size_mm), 4),
            "jpeg_quality": spec.jpeg_quality if spec.format in ("jpeg", "webp") else None,
            "background": {"kind": sc.background, "licence": backgrounds.LICENCE},
            "clutter_objects": meta["clutter_objects"],
            "degrade_backend": meta["backend"],
            "licence": LICENCE,
            "source": "tools/synth",
        }
        if truth_mode != "none":
            (out / "truth").mkdir(exist_ok=True)
            t = _truth(assets, spec, quad, meta)
            (out / "truth" / f"{spec.image_id}.json").write_text(json.dumps(t, separators=(",", ":")), encoding="utf8")
            row["truth"] = f"truth/{spec.image_id}.json"
            if truth_mode == "full":
                cv2.imwrite(str(out / "truth" / f"{spec.image_id}.clean.png"), assets.page.clean_binary())
                row["clean_render"] = f"truth/{spec.image_id}.clean.png"
        rows.append(row)
    return rows


def _group_by_scene(images: list[plan.ImageSpec]):
    groups: dict[int, list[plan.ImageSpec]] = {}
    for im in images:
        groups.setdefault(im.scene.index, []).append(im)
    return [(images_[0].scene, images_) for _, images_ in sorted(groups.items())]


def generate(
    out: Path,
    name: str,
    seed: int,
    count: int,
    max_edge: int = 512,
    jobs: int | None = None,
    truth: str = "text",
    quiet: bool = False,
    overrides: dict | None = None,
    _worker=None,
) -> dict:
    """Write a suite under ``out``. Returns a summary dict (also written as ``suite.json``)."""
    out = Path(out)
    for stale in ("images", "truth"):  # files of an earlier run would otherwise linger in the suite
        shutil.rmtree(out / stale, ignore_errors=True)
    (out / "images").mkdir(parents=True, exist_ok=True)
    scenes, images = plan.build(name, seed, count, overrides)
    tasks = [(sc, imgs, str(out), max_edge, truth, name) for sc, imgs in _group_by_scene(images)]
    jobs = jobs or min(os.cpu_count() or 1, 8)
    worker = _worker or _render_scene  # replaceable so a test can make a worker die
    started = time.time()
    results: dict[int, list[dict]] = {}
    done = 0

    def note(n_rows: int):
        nonlocal done
        done += n_rows
        if not quiet and (done == count or done % max(1, count // 20) < n_rows):
            print(f"  {done}/{count} images, {time.time() - started:.0f}s", file=sys.stderr, flush=True)

    if jobs <= 1 or len(tasks) == 1:
        for i, task in enumerate(tasks):
            results[i] = worker(task)
            note(len(results[i]))
    else:
        # A worker that dies (out of memory, a crashed native library) must not hang the run, which
        # `multiprocessing.Pool` would: a broken pool raises, and the unfinished scenes are retried
        # on a fresh pool. Every scene is a pure function of the seed, so a retry changes nothing.
        for attempt in range(3):
            pending = [i for i in range(len(tasks)) if i not in results]
            if not pending:
                break
            try:
                with ProcessPoolExecutor(max_workers=jobs, mp_context=mp.get_context("spawn")) as pool:
                    futures = {pool.submit(worker, tasks[i]): i for i in pending}
                    for f in as_completed(futures):
                        results[futures[f]] = f.result()
                        note(len(results[futures[f]]))
            except BrokenProcessPool:
                print(f"  a worker process died; retrying {len(tasks) - len(results)} unfinished scene(s) (attempt {attempt + 1} of 3)", file=sys.stderr, flush=True)
        if len(results) != len(tasks):
            raise RuntimeError(f"{len(tasks) - len(results)} scene(s) could not be rendered: workers keep dying (memory?)")
    rows = [r for i in sorted(results) for r in results[i]]
    rows.sort(key=lambda r: r["id"])
    text = "".join(json.dumps(r, separators=(",", ":")) + "\n" for r in rows)
    (out / "manifest.jsonl").write_text(text, encoding="utf8", newline="\n")
    hist = plan.histogram([r["tags"] for r in rows])
    image_bytes = sum((out / r["image"]).stat().st_size for r in rows)
    summary = {
        "generator": GENERATOR,
        "name": name,
        "seed": seed,
        "count": count,
        "scenes": len(scenes),
        "max_edge": max_edge,
        "degrade_backend": degrade.backend_name(),
        "manifest_sha256": hashlib.sha256(text.encode("utf8")).hexdigest(),
        "image_bytes": image_bytes,
        "seconds": round(time.time() - started, 1),
        "tag_histogram": hist,
    }
    (out / "suite.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n", encoding="utf8")
    return summary


def hash_tree(out: Path) -> dict[str, str]:
    """SHA-256 of every file listed in the manifest plus the manifest itself (for determinism tests)."""
    out = Path(out)
    result = {"manifest.jsonl": hashlib.sha256((out / "manifest.jsonl").read_bytes()).hexdigest()}
    for line in (out / "manifest.jsonl").read_text(encoding="utf8").splitlines():
        row = json.loads(line)
        result[row["image"]] = hashlib.sha256((out / row["image"]).read_bytes()).hexdigest()
    return result

