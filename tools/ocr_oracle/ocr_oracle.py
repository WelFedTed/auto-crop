#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Tesseract OCR oracle (ROADMAP M1.66; extended by M1.68, M7.50, M12.55). Dev-only.

Auto Crop never ships OCR (B19). This tool measures how readable an image is by running
Tesseract 5.x over it with FIXED, LOGGED settings and scoring the text against a transcript:

* CER        character error rate = Levenshtein(reference, hypothesis) / len(reference), on text
             normalised by `normalise` (whitespace runs collapsed, blank lines dropped).
* amounts    share of the reference's money amounts (`\\d+[.,]\\d{2}`) that appear, as exact
             strings, in the OCR text (multiset recall).
* numerics   the same for every numeric token (`\\d+(?:[.,]\\d+)*`).

Sub-commands

    synth  --out DIR [--count N --seed S --font TTF --blur SIGMA]
           Writes clean synthetic receipts (PNG + exact transcript) and a blurred copy of each.
    run    --images DIR [--truth DIR] --label NAME --out FILE.json [--thresholding 0|1|2 ...]
           OCRs every image in DIR, scores it against DIR/<stem>.txt (or --truth) and writes a
           JSON report: the Tesseract version, traineddata hash, the exact command line and
           every setting, per-image and pooled results.
    check  --clean A.json --blurred B.json [--max-clean-cer 0.05]
           The M1.66 acceptance: clean CER <= bar and the blurred copies are worse.

Settings are never defaulted silently: they are fixed here (OEM 1 = LSTM only, PSM 6 = one uniform
block, 300 dpi, `eng`, a single OpenMP thread) and written into every report. `thresholding_method`
(Tesseract >= 5.3: 0 Otsu, 1 adaptive Otsu, 2 Sauvola) is an explicit required choice per run.

Python 3.10+; Pillow is needed for `synth` only (requirements.lock). Metrics and `run` use only the
standard library plus the `tesseract` binary.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
from collections import Counter
from pathlib import Path

AMOUNT_RE = re.compile(r"\d+[.,]\d{2}")
NUMERIC_RE = re.compile(r"\d+(?:[.,]\d+)*")
IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp", ".webp"}

# Fixed Tesseract settings (everything that can change the text must be here or in the report).
FIXED = {
    "lang": "eng",
    "oem": 1,
    "psm": 6,
    "dpi": 300,
    "omp_thread_limit": 1,
}


# ---------------------------------------------------------------------------------------------
# Metrics


RULE_RE = re.compile(r"^[-=_*.~#—–─]{4,}$")


def normalise(text: str) -> str:
    """Whitespace runs inside a line collapse to one space; blank lines, form feeds and separator
    rules (a line of four or more `-=_*.~#` or dash characters) are dropped. Rules are drawing, not
    content: Tesseract skips or garbles them (measured: it dropped every rule line of the synthetic
    receipts, which alone was 32% CER of otherwise perfect text), so scoring them would measure
    that instead of readability. The same filter applies to the reference and the OCR text."""
    lines = []
    for raw in text.replace("\f", "\n").replace("\r", "").split("\n"):
        line = " ".join(raw.split())
        if line and not RULE_RE.match(line.replace(" ", "")):
            lines.append(line)
    return "\n".join(lines)


def levenshtein(a: str, b: str) -> int:
    """Edit distance (insert, delete, substitute; all cost 1), two-row dynamic programme."""
    if a == b:
        return 0
    if not a:
        return len(b)
    if not b:
        return len(a)
    if len(a) < len(b):
        a, b = b, a
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def cer(reference: str, hypothesis: str) -> float:
    ref, hyp = normalise(reference), normalise(hypothesis)
    if not ref:
        raise ValueError("empty reference transcript")
    return levenshtein(ref, hyp) / len(ref)


def token_recall(pattern: re.Pattern[str], reference: str, hypothesis: str) -> tuple[int, int]:
    """(matched, total): reference tokens found as exact strings in the hypothesis (multiset)."""
    ref = Counter(pattern.findall(reference))
    hyp = Counter(pattern.findall(hypothesis))
    matched = sum(min(n, hyp[tok]) for tok, n in ref.items())
    return matched, sum(ref.values())


def score(reference: str, hypothesis: str) -> dict:
    am, at = token_recall(AMOUNT_RE, reference, hypothesis)
    nm, nt = token_recall(NUMERIC_RE, reference, hypothesis)
    ref_n = normalise(reference)
    return {
        "cer": levenshtein(ref_n, normalise(hypothesis)) / len(ref_n),
        "ref_chars": len(ref_n),
        "edits": levenshtein(ref_n, normalise(hypothesis)),
        "amounts_matched": am,
        "amounts_total": at,
        "numerics_matched": nm,
        "numerics_total": nt,
    }


def pool(results: list[dict]) -> dict:
    """Aggregate per-image scores: mean and median CER, pooled CER (total edits / total chars),
    and pooled token accuracies."""
    cers = [r["cer"] for r in results]
    chars = sum(r["ref_chars"] for r in results)
    out = {
        "images": len(results),
        "cer_mean": statistics.fmean(cers),
        "cer_median": statistics.median(cers),
        "cer_max": max(cers),
        "cer_pooled": sum(r["edits"] for r in results) / chars,
    }
    for key in ("amounts", "numerics"):
        total = sum(r[f"{key}_total"] for r in results)
        out[f"{key}_accuracy"] = (
            sum(r[f"{key}_matched"] for r in results) / total if total else None
        )
        out[f"{key}_tokens"] = total
    return out


# ---------------------------------------------------------------------------------------------
# Tesseract


def tesseract_info(binary: str) -> dict:
    """Version lines and the traineddata hash, so a result can be tied to what produced it."""
    exe = shutil.which(binary) or binary
    ver = subprocess.run([exe, "--version"], capture_output=True, text=True, check=True)
    lines = [ln.strip() for ln in (ver.stdout + ver.stderr).splitlines() if ln.strip()]
    langs = subprocess.run([exe, "--list-langs"], capture_output=True, text=True, check=True)
    text = langs.stdout + langs.stderr
    # Tesseract 5.3+: `List of available languages in "/usr/share/tesseract-ocr/5/tessdata/" (2):`
    m = re.search(r'in "([^"]+)"', text)
    tessdata = Path(m.group(1)) if m else None
    if tessdata is None and os.environ.get("TESSDATA_PREFIX"):
        tessdata = Path(os.environ["TESSDATA_PREFIX"])
    traineddata = tessdata / f"{FIXED['lang']}.traineddata" if tessdata else None
    sha = None
    if traineddata and traineddata.exists():
        sha = hashlib.sha256(traineddata.read_bytes()).hexdigest()
    version = lines[0].split()[1] if lines and len(lines[0].split()) > 1 else "unknown"
    return {
        "binary": exe,
        "version": version,
        "version_lines": lines[:8],
        "tessdata_dir": str(tessdata) if tessdata else None,
        "traineddata_sha256": sha,
    }


def tesseract_argv(binary: str, image: Path, thresholding: int, extra: list[str]) -> list[str]:
    return [
        binary,
        str(image),
        "stdout",
        "-l", FIXED["lang"],
        "--oem", str(FIXED["oem"]),
        "--psm", str(FIXED["psm"]),
        "--dpi", str(FIXED["dpi"]),
        "-c", f"thresholding_method={thresholding}",
        *extra,
    ]


def ocr(binary: str, image: Path, thresholding: int, extra: list[str]) -> str:
    env = dict(os.environ, OMP_THREAD_LIMIT=str(FIXED["omp_thread_limit"]))
    argv = tesseract_argv(binary, image, thresholding, extra)
    proc = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=300)
    if proc.returncode != 0:
        raise RuntimeError(f"tesseract failed on {image}: {proc.stderr.strip()[:500]}")
    return proc.stdout


def cmd_run(args: argparse.Namespace) -> int:
    images = sorted(p for p in Path(args.images).iterdir() if p.suffix.lower() in IMAGE_EXTS)
    if not images:
        print(f"no images in {args.images}", file=sys.stderr)
        return 2
    truth_dir = Path(args.truth) if args.truth else Path(args.images)
    info = tesseract_info(args.tesseract)
    if not info["version"].startswith("5."):
        print(f"Tesseract 5.x is required, found {info['version']}", file=sys.stderr)
        return 2
    extra = list(args.extra or [])
    print(f"tesseract {info['version']}  traineddata sha256 {info['traineddata_sha256']}")
    print("command:", " ".join(tesseract_argv("tesseract", Path("<image>"), args.thresholding, extra)))
    rows = []
    for img in images:
        ref_path = truth_dir / f"{img.stem}.txt"
        if not ref_path.exists():
            print(f"no transcript for {img.name} ({ref_path})", file=sys.stderr)
            return 2
        reference = ref_path.read_text(encoding="utf-8")
        hyp = ocr(info["binary"], img, args.thresholding, extra)
        if args.dump:
            Path(args.dump).mkdir(parents=True, exist_ok=True)
            (Path(args.dump) / f"{img.stem}.ocr.txt").write_text(hyp, encoding="utf-8")
        row = {"image": img.name, **score(reference, hyp)}
        rows.append(row)
        print(f"  {img.name:28} CER {row['cer'] * 100:6.2f}%  amounts {row['amounts_matched']}/{row['amounts_total']}"
              f"  numerics {row['numerics_matched']}/{row['numerics_total']}")
    agg = pool(rows)
    report = {
        "label": args.label,
        "tesseract": info,
        "settings": {**FIXED, "thresholding_method": args.thresholding, "extra_args": extra},
        "images_dir": str(args.images),
        "results": rows,
        "aggregate": agg,
    }
    if args.out:
        Path(args.out).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(
        f"[{args.label}] {agg['images']} images: mean CER {agg['cer_mean'] * 100:.2f}%, "
        f"median {agg['cer_median'] * 100:.2f}%, pooled {agg['cer_pooled'] * 100:.2f}%, "
        f"amounts {fmt_pct(agg['amounts_accuracy'])}, numerics {fmt_pct(agg['numerics_accuracy'])}"
    )
    return 0


def fmt_pct(v: float | None) -> str:
    return "n/a" if v is None else f"{v * 100:.1f}%"


def cmd_check(args: argparse.Namespace) -> int:
    clean = json.loads(Path(args.clean).read_text(encoding="utf-8"))
    blurred = json.loads(Path(args.blurred).read_text(encoding="utf-8"))
    for k in ("tesseract", "settings"):
        if k == "tesseract":
            same = clean[k]["version"] == blurred[k]["version"]
        else:
            same = clean[k] == blurred[k]
        if not same:
            print(f"the two reports differ in {k}; they must come from the same setup", file=sys.stderr)
            return 2
    c, b = clean["aggregate"], blurred["aggregate"]
    ok_bar = c["cer_mean"] <= args.max_clean_cer
    ok_worse = b["cer_mean"] > c["cer_mean"]
    print(f"Tesseract {clean['tesseract']['version']}, thresholding_method={clean['settings']['thresholding_method']}")
    print(f"clean   mean CER {c['cer_mean'] * 100:.2f}%  (bar <= {args.max_clean_cer * 100:.1f}%)  "
          f"{'PASS' if ok_bar else 'FAIL'}")
    print(f"blurred mean CER {b['cer_mean'] * 100:.2f}%  worse than clean: {'PASS' if ok_worse else 'FAIL'}")
    print(f"amounts  clean {fmt_pct(c['amounts_accuracy'])}  blurred {fmt_pct(b['amounts_accuracy'])}")
    print(f"numerics clean {fmt_pct(c['numerics_accuracy'])}  blurred {fmt_pct(b['numerics_accuracy'])}")
    return 0 if ok_bar and ok_worse else 1


# ---------------------------------------------------------------------------------------------
# Synthetic receipts

STORES = ["CORNER MARKET", "BLUE HERON CAFE", "NORTHSIDE HARDWARE", "GREENLEAF PHARMACY", "CITY FUEL STOP",
          "OLD TOWN BAKERY", "MAPLE & MAIN DELI", "RIVERSIDE BOOKS"]
STREETS = ["12 Mill Road", "408 Station Street", "7 Harbour Lane", "93 Oak Avenue", "1500 Quay Drive"]
ITEMS = ["MILK 2L", "BREAD WHOLEMEAL", "COFFEE BEANS 250G", "BATTERIES AA 4PK", "ORANGE JUICE", "PASTA 500G",
         "CHEDDAR 200G", "NOTEBOOK A5", "USB CABLE 1M", "SPARKLING WATER", "BANANAS", "TOMATOES 1KG",
         "DISH SOAP", "LIGHT BULB E27", "BLACK PEN 3PK", "OLIVE OIL 500ML", "RICE 1KG", "YOGHURT 4PK"]
FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "C:/Windows/Fonts/consola.ttf",
    "/System/Library/Fonts/Menlo.ttc",
]
WIDTH_CHARS = 32


def money(cents: int) -> str:
    return f"{cents // 100}.{cents % 100:02d}"


def receipt_text(rng: random.Random) -> str:
    """One receipt as `WIDTH_CHARS`-column text. Every amount has exactly two decimals."""
    def row(left: str, right: str) -> str:
        return left[: WIDTH_CHARS - len(right) - 1].ljust(WIDTH_CHARS - len(right)) + right

    lines = [rng.choice(STORES).center(WIDTH_CHARS).rstrip(), rng.choice(STREETS).center(WIDTH_CHARS).rstrip(),
             f"Tel 555-{rng.randrange(1000, 9999)}".center(WIDTH_CHARS).rstrip(), "-" * WIDTH_CHARS]
    lines.append(f"{rng.randrange(1, 28):02d}/{rng.randrange(1, 13):02d}/2026  {rng.randrange(7, 22):02d}:{rng.randrange(60):02d}")
    lines.append(f"Receipt {rng.randrange(10000, 99999)}")
    lines.append("-" * WIDTH_CHARS)
    total = 0
    for name in rng.sample(ITEMS, rng.randrange(4, 9)):
        qty = rng.choice([1, 1, 1, 2, 3])
        unit = rng.randrange(89, 2599)
        total += qty * unit
        label = f"{qty} x {name}" if qty > 1 else name
        lines.append(row(label, money(qty * unit)))
    tax = round(total * 0.2 / 1.2)
    lines += ["-" * WIDTH_CHARS, row("TOTAL", money(total)), row("VAT 20% incl.", money(tax))]
    paid = ((total + 499) // 500) * 500
    lines += [row("CASH", money(paid)), row("CHANGE", money(paid - total)), "-" * WIDTH_CHARS,
              "THANK YOU FOR SHOPPING".center(WIDTH_CHARS).rstrip()]
    return "\n".join(lines) + "\n"


def find_font(explicit: str | None) -> str:
    for cand in ([explicit] if explicit else []) + FONT_CANDIDATES:
        if cand and Path(cand).exists():
            return cand
    raise SystemExit("no monospace font found; pass --font /path/to/DejaVuSansMono.ttf")


def cmd_synth(args: argparse.Namespace) -> int:
    from PIL import Image, ImageDraw, ImageFilter, ImageFont  # noqa: PLC0415 (only synth needs it)
    import PIL

    font_path = find_font(args.font)
    font = ImageFont.truetype(font_path, args.font_px)
    char_w = font.getlength("M")
    line_h = int(args.font_px * 1.35)
    out = Path(args.out)
    (out / "clean").mkdir(parents=True, exist_ok=True)
    (out / "blurred").mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    for i in range(args.count):
        text = receipt_text(rng)
        lines = text.rstrip("\n").split("\n")
        margin = int(char_w * 2)
        w = int(char_w * WIDTH_CHARS) + 2 * margin
        h = line_h * len(lines) + 2 * margin
        # Paper 250, ink 25: a clean, evenly lit scan-like image (no illumination, no noise).
        img = Image.new("L", (w, h), 250)
        draw = ImageDraw.Draw(img)
        for n, line in enumerate(lines):
            draw.text((margin, margin + n * line_h), line, font=font, fill=25)
        stem = f"receipt_{i:03d}"
        img.save(out / "clean" / f"{stem}.png", dpi=(300, 300))
        (out / "clean" / f"{stem}.txt").write_text(text, encoding="utf-8")
        img.filter(ImageFilter.GaussianBlur(args.blur)).save(out / "blurred" / f"{stem}.png", dpi=(300, 300))
    manifest = {
        "count": args.count, "seed": args.seed, "font": font_path, "font_px": args.font_px,
        "blur_sigma_px": args.blur, "pillow": PIL.__version__, "python": sys.version.split()[0],
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {args.count} receipts to {out} ({manifest})")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("synth", help="write synthetic receipts and a blurred copy")
    s.add_argument("--out", required=True)
    s.add_argument("--count", type=int, default=12)
    s.add_argument("--seed", type=int, default=20261003)
    s.add_argument("--font")
    s.add_argument("--font-px", type=int, default=34, help="em size; 34 px gives ~26 px capitals")
    s.add_argument("--blur", type=float, default=3.0, help="Gaussian sigma in px for the blurred copy")
    s.set_defaults(fn=cmd_synth)

    r = sub.add_parser("run", help="OCR a directory of images and score it")
    r.add_argument("--images", required=True)
    r.add_argument("--truth", help="directory with <stem>.txt transcripts (default: --images)")
    r.add_argument("--label", required=True)
    r.add_argument("--thresholding", type=int, required=True, choices=[0, 1, 2],
                   help="Tesseract thresholding_method: 0 Otsu, 1 adaptive Otsu, 2 Sauvola")
    r.add_argument("--tesseract", default="tesseract")
    r.add_argument("--out")
    r.add_argument("--dump", help="also write the raw OCR text of each image here")
    r.add_argument("extra", nargs="*", help="extra Tesseract arguments after `--` (logged)")
    r.set_defaults(fn=cmd_run)

    c = sub.add_parser("check", help="M1.66 acceptance on a clean and a blurred report")
    c.add_argument("--clean", required=True)
    c.add_argument("--blurred", required=True)
    c.add_argument("--max-clean-cer", type=float, default=0.05)
    c.set_defaults(fn=cmd_check)

    args = ap.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
