# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Command line: ``python -m synth --seed S --count N --out DIR`` and the check subcommands."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import GENERATOR, multi_item, plan

SUBCOMMANDS = ("check-geometry", "check-ocr", "variants", "quotas", "licences", "contact-sheet")


def _generate_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="python -m synth",
        description=f"Synthetic page and receipt photographs with exact ground truth ({GENERATOR}). "
        "Subcommands: " + ", ".join(SUBCOMMANDS) + " (python -m synth <sub> --help).",
    )
    p.add_argument(
        "--suite",
        choices=sorted(plan.SUITES) + sorted(multi_item.SUITES),
        help="preset: smoke (200 images) or full (5,200); multi-smoke (160) or multi-full (1,200) are the "
        "multi-item scenes (several photos or receipts per picture, manifest `items`)",
    )
    p.add_argument("--seed", type=lambda s: int(s, 0), help="suite seed (default: the preset's)")
    p.add_argument("--count", type=int, help="number of images (default: the preset's)")
    p.add_argument("--name", help="id prefix and suite name (default: the preset or `synth`)")
    p.add_argument("--out", required=True, help="output directory (never commit it)")
    p.add_argument("--max-edge", type=int, default=None, help="long edge of every image in pixels (default: the preset's, else 512)")
    p.add_argument("--jobs", type=int, default=None, help="worker processes (default: min(cpus, 8))")
    p.add_argument("--truth", choices=["none", "text", "full"], default="text",
                   help="sidecars: none, text (transcript JSON), full (adds the clean binary render)")
    p.add_argument("--pin", action="append", default=[], metavar="AXIS=VALUE",
                   help="always draw this tag value (repeatable), for single-factor experiments, e.g. "
                   "--pin lighting=normal --pin clutter=none; the quotas no longer apply")
    p.add_argument("--backend", choices=["auto", "builtin"], default="auto",
                   help="degradation backend: Augraphy when importable (auto) or the builtin NumPy one")
    return p


def run_generate(argv: list[str]) -> int:
    import os

    a = _generate_parser().parse_args(argv)
    if (a.suite or "") in multi_item.SUITES or (a.name or "").startswith("multi"):
        return run_generate_multi(a)
    preset = plan.SUITES.get(a.suite or "", {})
    seed = a.seed if a.seed is not None else preset.get("seed", 1)
    count = a.count if a.count is not None else preset.get("count", 200)
    name = a.name or a.suite or "synth"
    max_edge = a.max_edge if a.max_edge is not None else preset.get("max_edge", 512)
    if a.backend == "builtin":
        os.environ["SYNTH_BACKEND"] = "builtin"
    from . import suite

    overrides = {**preset.get("overrides", {}), **plan.pins_to_overrides(a.pin)}
    s = suite.generate(Path(a.out), name, seed, count, max_edge, a.jobs, a.truth, overrides=overrides)
    print(
        f"wrote {s['count']} images ({s['image_bytes'] / 1e6:.1f} MB) in {s['scenes']} scenes and "
        f"manifest.jsonl to {a.out} in {s['seconds']}s [{GENERATOR}, {s['degrade_backend']}, "
        f"seed {seed}, manifest sha256 {s['manifest_sha256'][:16]}...]"
    )
    return 0


def run_generate_multi(a) -> int:
    preset = multi_item.SUITES.get(a.suite or "multi-smoke")
    seed = a.seed if a.seed is not None else preset["seed"]
    count = a.count if a.count is not None else preset["count"]
    name = a.name or a.suite or "multi-smoke"
    max_edge = a.max_edge if a.max_edge is not None else preset["max_edge"]
    pins = plan.pins_to_multi_overrides(a.pin)
    s = multi_item.generate_multi(Path(a.out), name, seed, count, max_edge, a.jobs, preset["jpeg"], preset["png_share"], pins)
    print(
        f"wrote {s['count']} multi-item scenes ({s['image_bytes'] / 1e6:.1f} MB) and manifest.jsonl to {a.out} "
        f"in {s['seconds']}s [{GENERATOR}, seed {seed}, manifest sha256 {s['manifest_sha256'][:16]}...]"
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in SUBCOMMANDS:
        from . import verify

        return verify.main(argv[0], argv[1:])
    return run_generate(argv)


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
