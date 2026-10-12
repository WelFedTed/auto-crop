#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Does enhancement help? The OCR matrix of ROADMAP M7.50 / M7.55 (dev-only).

For every degradation slice folder (`degrade.py slices`) and every variant (a mode plus an optional
tuning spec of the `enhance_dir` example), the images are enhanced, OCR'd by the pinned Tesseract
with the fixed settings of `ocr_oracle.py`, and scored against the exact transcripts. The report is
a table of mean CER per slice and variant, the change against `original` (negative is better), the
number of images that got worse by more than one CER point, and the amount-token accuracy. The
claim "enhancement helps" is only as good as these numbers; a variant that makes a slice worse
shows up here as a positive delta and is named in the summary.

    python3 tools/ocr_oracle/enhance_matrix.py --bin target/release/examples/enhance_dir \\
        --slices-dir work/slices --variants tools/ocr_oracle/variants.json --out work/matrix

Needs `tesseract` on the PATH (CI only: it is not installed on the dev machine).
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ocr_oracle as o  # noqa: E402

WORSE_BY = 0.01  # an image is "worse" when its CER rose by more than one point


def enhance_dir(binary: str, src: Path, dst: Path, variant: dict) -> dict:
    """Runs the enhancer over one slice folder; returns the timing line."""
    dst.mkdir(parents=True, exist_ok=True)
    argv = [binary, "--in", str(src), "--out", str(dst), "--mode", variant["mode"]]
    if variant.get("strength") is not None:
        argv += ["--strength", str(variant["strength"])]
    if variant.get("spec"):
        argv += ["--spec", variant["spec"]]
    t = time.time()
    proc = subprocess.run(argv, capture_output=True, text=True, check=True)
    ms = [json.loads(ln)["ms"] for ln in proc.stdout.splitlines() if ln.startswith("{")]
    return {"seconds": time.time() - t, "mean_ms": statistics.fmean(ms) if ms else 0.0}


def ocr_images(images: list[Path], truth_dir: Path, thresholding: int, tesseract: str, jobs: int) -> dict:
    def one(img: Path) -> tuple[str, dict]:
        reference = (truth_dir / f"{img.stem}.txt").read_text(encoding="utf-8")
        return img.stem, o.score(reference, o.ocr(tesseract, img, thresholding, []))

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        return dict(pool.map(one, images))


def summarise(per_image: dict[str, dict]) -> dict:
    cers = [r["cer"] for r in per_image.values()]
    am_t = sum(r["amounts_total"] for r in per_image.values())
    am_m = sum(r["amounts_matched"] for r in per_image.values())
    return {"images": len(cers), "cer_mean": statistics.fmean(cers),
            "amounts_accuracy": (am_m / am_t) if am_t else None}


def compare(base: dict[str, dict], other: dict[str, dict]) -> dict:
    """Paired change of `other` against `base`, image by image."""
    deltas = [other[k]["cer"] - base[k]["cer"] for k in base if k in other]
    return {"delta_mean": statistics.fmean(deltas) if deltas else 0.0,
            "worse": sum(d > WORSE_BY for d in deltas),
            "better": sum(d < -WORSE_BY for d in deltas),
            "n": len(deltas)}


def lost_amounts(base: dict[str, dict], other: dict[str, dict]) -> int:
    """Amount tokens the original read that this variant lost (PLAN 5.10 damage gate, summed)."""
    return sum(max(0, base[k]["amounts_matched"] - other[k]["amounts_matched"])
               for k in base if k in other)


def build_tables(results: dict, variants: list[dict], slices: list[str], thresholding: list[int]) -> tuple[dict, str]:
    """`results[label][slice][t]` is `{image: score}`. Returns (summary json, markdown)."""
    labels = [v["label"] for v in variants]
    summary: dict = {}
    md: list[str] = []
    for t in thresholding:
        md.append(f"### Tesseract thresholding_method={t}\n")
        md.append("Mean CER %, in brackets the paired change against `original` (negative = better), "
                  "then `w` = images worse by more than 1 point, `a` = amount tokens lost.\n")
        md.append("| slice | " + " | ".join(labels) + " |")
        md.append("|---|" + "---:|" * len(labels))
        for s in slices:
            base = results["original"][s][t]
            cells = []
            for lab in labels:
                cur = results[lab][s][t]
                m = summarise(cur)
                if lab == "original":
                    cells.append(f"{m['cer_mean'] * 100:.2f}")
                    summary.setdefault(str(t), {}).setdefault(s, {})[lab] = {**m}
                    continue
                c = compare(base, cur)
                la = lost_amounts(base, cur)
                cells.append(f"{m['cer_mean'] * 100:.2f} ({c['delta_mean'] * 100:+.2f}) w{c['worse']} a{la}")
                summary.setdefault(str(t), {}).setdefault(s, {})[lab] = {**m, **c, "lost_amounts": la}
            md.append(f"| {s} | " + " | ".join(cells) + " |")
        md.append("")
        # The mean over slices, per variant: the headline.
        md.append("Mean over slices:\n")
        md.append("| variant | mean CER % | change vs original | slices worse | images worse |")
        md.append("|---|---:|---:|---:|---:|")
        for lab in labels:
            vals = [summary[str(t)][s][lab] for s in slices]
            mean_cer = statistics.fmean(v["cer_mean"] for v in vals) * 100
            if lab == "original":
                md.append(f"| {lab} | {mean_cer:.2f} | | | |")
                continue
            dm = statistics.fmean(v["delta_mean"] for v in vals) * 100
            sw = sum(v["delta_mean"] > 0.0025 for v in vals)
            iw = sum(v["worse"] for v in vals)
            md.append(f"| {lab} | {mean_cer:.2f} | {dm:+.2f} | {sw} | {iw} |")
        md.append("")
    return summary, "\n".join(md)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--bin", required=True, help="the enhance_dir example binary")
    ap.add_argument("--slices-dir", required=True)
    ap.add_argument("--variants", required=True, help="JSON list of {label, mode, spec?, strength?}")
    ap.add_argument("--out", required=True)
    ap.add_argument("--thresholding", type=int, nargs="+", default=[0])
    ap.add_argument("--tesseract", default="tesseract")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--only", nargs="*", help="restrict to these slice folder names")
    args = ap.parse_args(argv)

    variants = json.loads(Path(args.variants).read_text(encoding="utf-8"))
    if variants[0]["label"] != "original":
        print("the first variant must be `original`", file=sys.stderr)
        return 2
    slices_dir, out = Path(args.slices_dir), Path(args.out)
    slices = sorted(p.name for p in slices_dir.iterdir() if p.is_dir())
    if args.only:
        slices = [s for s in slices if s in args.only]
    info = o.tesseract_info(args.tesseract)
    print(f"tesseract {info['version']}  traineddata sha256 {info['traineddata_sha256']}")
    results: dict = {}
    timings: dict = {}
    for v in variants:
        lab = v["label"]
        results[lab] = {}
        for s in slices:
            src = slices_dir / s
            dst = out / "enhanced" / lab / s
            timings[f"{lab}/{s}"] = enhance_dir(args.bin, src, dst, v)
            images = sorted(p for p in dst.iterdir() if p.suffix == ".png")
            results[lab][s] = {}
            for t in args.thresholding:
                results[lab][s][t] = ocr_images(images, src, t, info["binary"], args.jobs)
            print(f"  {lab:28} {s:24} CER " + "  ".join(
                f"t{t}={summarise(results[lab][s][t])['cer_mean'] * 100:.2f}%" for t in args.thresholding),
                flush=True)
    summary, md = build_tables(results, variants, slices, args.thresholding)
    out.mkdir(parents=True, exist_ok=True)
    (out / "matrix.json").write_text(json.dumps({
        "tesseract": info, "settings": o.FIXED, "thresholding": args.thresholding,
        "variants": variants, "summary": summary, "timings": timings,
        "per_image": {lab: {s: {str(t): r for t, r in by_t.items()} for s, by_t in by_s.items()}
                      for lab, by_s in results.items()},
    }, indent=1) + "\n", encoding="utf-8")
    (out / "matrix.md").write_text(md + "\n", encoding="utf-8")
    print(md)
    return 0


if __name__ == "__main__":
    sys.exit(main())
