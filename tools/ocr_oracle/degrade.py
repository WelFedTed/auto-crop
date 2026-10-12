#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Synthetic documents and photographic degradations for the enhancement oracle (ROADMAP M7.50,
M7.53; dev-only, nothing here ships).

* `docs`    writes clean prose pages (DejaVu Sans, justified-looking left-aligned paragraphs with
            numbers and money amounts) with exact transcripts, next to the receipts that
            `ocr_oracle.py synth` writes.
* `slices`  takes a folder of clean images with transcripts and writes one folder per degradation
            slice: uneven lighting (tilt and vignette), a soft cast shadow, thermal fade, fade under
            shading, a dim noisy exposure, a warm lamp. Every slice is JPEG q85 (phone photos are),
            except `clean`, the control, which is the PNG untouched.

The slices are the conditions the enhancer is for; they share no code with the enhancer
(Pillow only, fields built at low resolution and resized). They are PROVISIONAL stand-ins for the
private receipt slice (B21): synthetic degradations, not photographs.

    python3 tools/ocr_oracle/degrade.py docs --out work/docs --count 6
    python3 tools/ocr_oracle/degrade.py slices --src work/blur-3.0/clean --out work/slices --family receipt
"""

from __future__ import annotations

import argparse
import json
import random
import shutil
import sys
from pathlib import Path

DOC_FONTS = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "C:/Windows/Fonts/arial.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
]

WORDS = (
    "the of and to in is that for it as with was on be by at this have from or an but not are we "
    "all their which will one can has more if no out about who so up what when your how time "
    "invoice payment account statement balance order delivery customer service report total "
    "quarter amount due date reference agreement notice office address contract review budget "
    "schedule meeting project manager supplier shipment warranty policy receipt tax paid"
).split()


def money(rng: random.Random) -> str:
    whole = rng.randrange(1, 9000)
    cents = rng.randrange(100)
    return f"{whole:,}.{cents:02d}" if whole >= 1000 else f"{whole}.{cents:02d}"


def paragraph(rng: random.Random, sentences: int) -> str:
    out = []
    for _ in range(sentences):
        n = rng.randrange(8, 16)
        words = []
        for i in range(n):
            if rng.randrange(14) == 0:
                words.append(money(rng))
            else:
                words.append(rng.choice(WORDS))
        words[0] = words[0].capitalize() if words[0][0].isalpha() else words[0]
        out.append(" ".join(words) + ".")
    return " ".join(out)


def wrap(text: str, font, max_width: float) -> list[str]:
    lines, cur = [], ""
    for word in text.split():
        trial = f"{cur} {word}".strip()
        if font.getlength(trial) <= max_width or not cur:
            cur = trial
        else:
            lines.append(cur)
            cur = word
    if cur:
        lines.append(cur)
    return lines


def find_doc_font(explicit: str | None) -> str:
    for cand in ([explicit] if explicit else []) + DOC_FONTS:
        if cand and Path(cand).exists():
            return cand
    raise SystemExit("no proportional font found; pass --font /path/to/DejaVuSans.ttf")


def cmd_docs(args: argparse.Namespace) -> int:
    from PIL import Image, ImageDraw, ImageFont  # noqa: PLC0415

    font_path = find_doc_font(args.font)
    font = ImageFont.truetype(font_path, args.font_px)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    margin = int(args.font_px * 2.2)
    width, height = 1240, 1000
    line_h = int(args.font_px * 1.45)
    for i in range(args.count):
        img = Image.new("L", (width, height), 250)
        draw = ImageDraw.Draw(img)
        transcript, y = [], margin
        while y + line_h < height - margin:
            para = paragraph(rng, rng.randrange(2, 4))
            for line in wrap(para, font, width - 2 * margin):
                if y + line_h >= height - margin:
                    break
                draw.text((margin, y), line, font=font, fill=25)
                transcript.append(line)
                y += line_h
            y += line_h // 2
            transcript.append("")
        stem = f"doc_{i:03d}"
        img.save(out / f"{stem}.png", dpi=(150, 150))
        (out / f"{stem}.txt").write_text("\n".join(transcript).rstrip() + "\n", encoding="utf-8")
    manifest = {"count": args.count, "seed": args.seed, "font": font_path, "font_px": args.font_px,
                "size": [width, height]}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {args.count} documents to {out}")
    return 0


# ---------------------------------------------------------------------------------------------
# Degradation slices


def field(w: int, h: int, fn):
    """An 'L' image whose value is 255 * fn(u, v) for u, v in 0..1, built at 48 x 48 and resized."""
    from PIL import Image  # noqa: PLC0415

    small = Image.new("L", (48, 48))
    small.putdata([max(0, min(255, round(255 * fn((x + 0.5) / 48, (y + 0.5) / 48))))
                   for y in range(48) for x in range(48)])
    return small.resize((w, h), Image.BILINEAR)


def tilt(dx: float, dy: float):
    return lambda u, v: 1.0 - dx * u - dy * v


def vignette(k: float):
    return lambda u, v: 1.0 - k * ((u - 0.5) ** 2 + (v - 0.5) ** 2) / 0.5


def shadow(depth: float, soft: float):
    def f(u, v):
        d = ((u - 0.55) * 0.8 + (v - 0.45) * 0.6) / soft
        t = max(0.0, min(1.0, d * 0.5 + 0.5))
        return 1.0 - depth * t * t * (3 - 2 * t)
    return f


def product(*fns):
    def f(u, v):
        r = 1.0
        for fn in fns:
            r *= fn(u, v)
        return r
    return f


def lit(img, fn):
    from PIL import ImageChops  # noqa: PLC0415

    return ImageChops.multiply(img, field(img.width, img.height, fn))


def noisy(img, sigma: float):
    from PIL import Image, ImageChops  # noqa: PLC0415

    if sigma <= 0:
        return img
    # Luminance noise: one gaussian field (mean 128), the same on every channel of a colour image.
    noise = Image.effect_noise(img.size, sigma).convert(img.mode)
    return ImageChops.add(img, noise, scale=1.0, offset=-128)


def blur(img, sigma: float):
    from PIL import ImageFilter  # noqa: PLC0415

    return img.filter(ImageFilter.GaussianBlur(sigma)) if sigma > 0 else img


def fade(img, keep: float, paper: int = 250):
    return img.point([max(0, min(255, round(paper - keep * (paper - v)))) for v in range(256)])


def tint(img, rgb):
    from PIL import Image  # noqa: PLC0415

    r, g, b = img.convert("RGB").split()
    return Image.merge("RGB", (
        r.point([round(v * rgb[0]) for v in range(256)]),
        g.point([round(v * rgb[1]) for v in range(256)]),
        b.point([round(v * rgb[2]) for v in range(256)]),
    ))


# name -> function(L image) -> image. Each is a recipe for a phone-photo condition.
SLICES = {
    "clean": lambda im: im,
    "shading": lambda im: blur(noisy(lit(im, product(tilt(0.45, 0.2), vignette(0.2))), 4), 0.8),
    "shadow": lambda im: blur(noisy(lit(im, shadow(0.55, 0.30)), 4), 0.8),
    "fade": lambda im: blur(noisy(fade(im, 0.30), 3), 0.8),
    "fade_shading": lambda im: blur(noisy(lit(fade(im, 0.45), tilt(0.4, 0.15)), 4), 0.8),
    "dim_noisy": lambda im: blur(noisy(lit(im, lambda u, v: 0.55), 10), 1.0),
    "warm_lamp": lambda im: blur(noisy(tint(lit(im, product(tilt(0.35, 0.1), vignette(0.15))),
                                            (1.0, 0.86, 0.62)), 4), 0.8),
}


def cmd_slices(args: argparse.Namespace) -> int:
    from PIL import Image  # noqa: PLC0415

    src = Path(args.src)
    out = Path(args.out)
    names = args.slices or list(SLICES)
    images = sorted(src.glob("*.png"))
    if not images:
        print(f"no PNG images in {src}", file=sys.stderr)
        return 2
    for name in names:
        recipe = SLICES[name]
        d = out / f"{args.family}-{name}"
        d.mkdir(parents=True, exist_ok=True)
        for p in images:
            img = Image.open(p).convert("L")
            res = recipe(img)
            ref = p.with_suffix(".txt")
            if name == "clean":
                res.save(d / f"{p.stem}.png", dpi=(300, 300))
            else:
                res.convert("RGB" if res.mode == "RGB" else "L").save(
                    d / f"{p.stem}.jpg", quality=85, dpi=(300, 300))
            shutil.copyfile(ref, d / f"{p.stem}.txt")
        print(f"{d}: {len(images)} images")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("docs", help="write clean prose documents with transcripts")
    d.add_argument("--out", required=True)
    d.add_argument("--count", type=int, default=6)
    d.add_argument("--seed", type=int, default=20261012)
    d.add_argument("--font")
    d.add_argument("--font-px", type=int, default=26)
    d.set_defaults(fn=cmd_docs)
    s = sub.add_parser("slices", help="write the degradation slices of a clean folder")
    s.add_argument("--src", required=True)
    s.add_argument("--out", required=True)
    s.add_argument("--family", required=True)
    s.add_argument("--slices", nargs="*", choices=sorted(SLICES))
    s.set_defaults(fn=cmd_slices)
    args = ap.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
