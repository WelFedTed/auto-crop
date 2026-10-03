# OCR oracle (M1.66)

Dev-only: Auto Crop ships no OCR (B19). The oracle measures how readable an image is by running
Tesseract 5.x with fixed, logged settings and scoring the text against a transcript. M1.68 (enhance
sweep), M7.50 and M12.55 extend it; the hand transcripts of the golden receipts (M1.67) stay
private and never enter this repository (B21).

    python3 -m pip install --require-hashes -r tools/ocr_oracle/requirements.lock   # Pillow, synth only
    python3 tools/ocr_oracle/ocr_oracle.py synth --out work --count 12 --blur 3.0
    python3 tools/ocr_oracle/ocr_oracle.py run --images work/clean --label clean --thresholding 0 --out clean.json
    python3 tools/ocr_oracle/ocr_oracle.py run --images work/blurred --truth work/clean --label blurred --thresholding 0 --out blurred.json
    python3 tools/ocr_oracle/ocr_oracle.py check --clean clean.json --blurred blurred.json
    python3 -m unittest discover -s tools/ocr_oracle                                # metric tests, no Tesseract needed

Tesseract is not installed on the Windows dev machine; the job `.github/workflows/ocr-oracle.yml`
(Ubuntu 22.04, Tesseract from `ppa:alex-p/tesseract-ocr5`, the same PPA as the devcontainer) runs it
on pushes to `main` that touch this directory and on dispatch.

## Settings (fixed, and written into every JSON report)

`-l eng --oem 1 --psm 6 --dpi 300 -c thresholding_method=N`, one OpenMP thread. `thresholding_method`
(0 Otsu, 1 adaptive Otsu, 2 Sauvola; Tesseract >= 5.3) has no default: `run` requires it. The report
records the Tesseract version lines, the `eng.traineddata` SHA-256 and the exact argument list. The
`check` command refuses two reports whose settings or Tesseract version differ.

## Metrics

* **CER** = Levenshtein(reference, OCR) / len(reference) after `normalise`: whitespace runs collapse,
  blank lines go, and separator rules (four or more of `-=_*.~#` or a dash) are dropped from both
  sides. Rules are drawing, not content; Tesseract drops them, which alone was 32% CER of otherwise
  perfect text in the first CI run.
* **amounts**: share of the reference's `\d+[.,]\d{2}` tokens found as exact strings (multiset recall).
* **numerics**: the same for every numeric token `\d+([.,]\d+)*`.

## Result on synthetic receipts (M1.66 acceptance)

12 receipts (seed 20261003, DejaVu Sans Mono 34 px, paper 250, ink 25, 32 columns, exact transcripts),
and a copy of each blurred with a Gaussian of sigma 3 px. CI run
[37117596234](https://github.com/WelFedTed/auto-crop/actions/runs/37117596234), Tesseract 5.5.1,
`eng.traineddata` sha256 `7d4322bd...170b2`, `thresholding_method=0`:

| Set | mean CER | amounts | numerics |
|---|---:|---:|---:|
| clean (bar <= 5%, PROVISIONAL) | **0.16%** | 99.2% | 99.7% |
| blurred, sigma 3 (must be worse) | **1.58%** | 95.0% | 90.9% |

Blur sweep (information only, thresholding 0): sigma 2: 0.95%, sigma 3: 1.58%, sigma 4: 48.7%,
sigma 5: 80.2%. The same images with other thresholding methods, for the record: method 1 (adaptive
Otsu) clean 0.03% / blurred 3.03%; method 2 (Sauvola) clean 0.16% / blurred 2.29%.

What this does and does not say: clean, evenly lit, monospace, scan-like images are the easy end
and the 5% bar is met with a wide margin; the blur pair shows the metric moves with image quality
(the cliff between sigma 3 and 4 is the point where strokes merge). It says nothing about real receipts
(thermal fade, shadows, perspective); that is M1.67/M1.68 on the private golden set.
