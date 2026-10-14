#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Reads the files of `write_samples` (crates/codecs/examples) with readers that are not ours and
# checks pixels, polarity, resolution and structure (ROADMAP M7.32, M11.56, M11.57, M11.59).
#
#   tools/interop/check-output-formats.sh <samples-dir> [--require-all]
#
# Readers: libtiff (`tiffinfo`), ImageMagick (`magick`), libwebp (`dwebp`), qpdf, Poppler
# (`pdfinfo`, `pdfimages`, `pdftoppm`). A missing tool is skipped with a message, unless
# `--require-all` (the CI Linux job) is given, which makes it a failure. Exit 1 on any mismatch.

set -u
DIR="${1:?usage: check-output-formats.sh <samples-dir> [--require-all]}"
REQUIRE="${2:-}"
FAILS=0
CHECKS=0

# ImageMagick 7 is `magick`; ImageMagick 6 (Ubuntu apt) has `compare` and `identify`.
if command -v magick >/dev/null 2>&1; then
  IM_COMPARE=(magick compare)
  IM_IDENTIFY=(magick identify)
elif command -v compare >/dev/null 2>&1; then
  IM_COMPARE=(compare)
  IM_IDENTIFY=(identify)
else
  IM_COMPARE=()
  IM_IDENTIFY=()
fi

have() { command -v "$1" >/dev/null 2>&1; }

need() {
  if have "$1"; then return 0; fi
  if [ "$REQUIRE" = "--require-all" ]; then
    echo "FAIL missing tool: $1"
    FAILS=$((FAILS + 1))
  else
    echo "SKIP missing tool: $1"
  fi
  return 1
}

ok() { CHECKS=$((CHECKS + 1)); echo "ok   $*"; }
bad() { CHECKS=$((CHECKS + 1)); FAILS=$((FAILS + 1)); echo "FAIL $*"; }

# Number of pixels that differ between a reference image and a file (0 = identical).
im_diff() {
  local out
  out=$("${IM_COMPARE[@]}" -metric AE "$1" "$2" null: 2>&1)
  # compare exits 1 when the images differ and prints the count first; take the leading number.
  echo "$out" | head -1 | sed 's/[^0-9].*//'
}

if [ ${#IM_COMPARE[@]} -eq 0 ]; then
  need magick || true
else
  for f in "$DIR"/tiff_rgb_*.tif; do
    d=$(im_diff "$DIR/ref_rgb.png" "$f")
    [ "$d" = "0" ] && ok "ImageMagick reads $(basename "$f") exactly" || bad "ImageMagick $(basename "$f"): $d pixels differ"
  done
  for f in "$DIR"/tiff_gray_*.tif; do
    d=$(im_diff "$DIR/ref_gray.png" "$f")
    [ "$d" = "0" ] && ok "ImageMagick reads $(basename "$f") exactly" || bad "ImageMagick $(basename "$f"): $d pixels differ"
  done
  for f in "$DIR"/tiff_g4_*.tif; do
    d=$(im_diff "$DIR/ref_bilevel.png" "$f")
    [ "$d" = "0" ] && ok "ImageMagick reads $(basename "$f") with the right polarity" || bad "ImageMagick $(basename "$f"): $d pixels differ (inverted?)"
    info=$("${IM_IDENTIFY[@]}" -verbose "$f" 2>&1)
    echo "$info" | grep -qi "Compression: Group4" && ok "$(basename "$f") is Group 4 for ImageMagick" || bad "$(basename "$f") is not seen as Group 4"
    echo "$info" | grep -qE "Resolution: 600x600" && ok "$(basename "$f") resolution 600x600" || bad "$(basename "$f") resolution not 600x600"
    echo "$info" | grep -qi "Orientation: TopLeft" && ok "$(basename "$f") orientation TopLeft" || bad "$(basename "$f") orientation"
  done
  d=$(im_diff "$DIR/ref_rgb.png" "$DIR/webp_rgb.webp")
  [ "$d" = "0" ] && ok "ImageMagick reads webp_rgb.webp exactly" || bad "ImageMagick webp_rgb: $d pixels differ"
  d=$(im_diff "$DIR/ref_gray.png" "$DIR/webp_gray.webp")
  [ "$d" = "0" ] && ok "ImageMagick reads webp_gray.webp exactly" || bad "ImageMagick webp_gray: $d pixels differ"
fi

if need tiffinfo; then
  for f in "$DIR"/tiff_*.tif; do
    out=$(tiffinfo "$f" 2>&1)
    if echo "$out" | grep -qiE "error|warning|not a TIFF"; then
      bad "tiffinfo complains about $(basename "$f"): $(echo "$out" | grep -iE 'error|warning' | head -1)"
    else
      ok "tiffinfo reads $(basename "$f") without a warning"
    fi
  done
  out=$(tiffinfo "$DIR/tiff_g4_onestrip.tif" 2>&1)
  echo "$out" | grep -q "Compression Scheme: CCITT Group 4" && ok "libtiff says CCITT Group 4" || bad "libtiff does not say CCITT Group 4"
  echo "$out" | grep -q "Photometric Interpretation: min-is-white" && ok "libtiff says min-is-white" || bad "libtiff photometric"
  echo "$out" | grep -q "Resolution: 600, 600 pixels/inch" && ok "libtiff says 600 dpi" || bad "libtiff resolution"
fi
if have tiffcmp && have tiff2png; then :; fi

if need dwebp; then
  for f in webp_rgb webp_gray; do
    if dwebp "$DIR/$f.webp" -o "$DIR/$f.dwebp.png" >/dev/null 2>&1; then
      ref=ref_rgb.png
      [ "$f" = webp_gray ] && ref=ref_gray.png
      if [ ${#IM_COMPARE[@]} -gt 0 ]; then
        d=$(im_diff "$DIR/$ref" "$DIR/$f.dwebp.png")
        [ "$d" = "0" ] && ok "dwebp decodes $f.webp exactly" || bad "dwebp $f: $d pixels differ"
      else
        ok "dwebp decodes $f.webp"
      fi
    else
      bad "dwebp failed on $f.webp"
    fi
  done
fi

if need qpdf; then
  for f in "$DIR"/pdf_*.pdf; do
    if qpdf --check "$f" >/dev/null 2>&1; then ok "qpdf --check $(basename "$f")"; else bad "qpdf --check $(basename "$f")"; fi
  done
fi

if need pdfinfo; then
  out=$(pdfinfo "$DIR/pdf_multi.pdf" 2>&1)
  echo "$out" | grep -q "^Pages: *4" && ok "pdfinfo: 4 pages" || bad "pdfinfo page count: $out"
  echo "$out" | grep -q "^Producer: *Auto Crop" && ok "pdfinfo: producer" || bad "pdfinfo producer"
  echo "$out" | grep -q "^Title: *Auto Crop interop sample" && ok "pdfinfo: title" || bad "pdfinfo title"
  echo "$out" | grep -qiE "^(Author|Creator|Keywords|Subject):" && bad "pdfinfo shows unexpected metadata" || ok "pdfinfo: no author, creator, keywords or subject"
  # Page 1: 251 x 173 px at 300 dpi = 60.24 x 41.52 pt; page 2 at 150 dpi; page 4 nominal 300 dpi.
  p1=$(pdfinfo -f 1 -l 4 "$DIR/pdf_multi.pdf" 2>&1 | grep "^Page *1 size:")
  echo "$p1" | grep -q "60.24 x 41.52" && ok "pdfinfo: page 1 is 60.24 x 41.52 pt" || bad "pdfinfo page 1 size: $p1"
  p2=$(pdfinfo -f 1 -l 4 "$DIR/pdf_multi.pdf" 2>&1 | grep "^Page *2 size:")
  echo "$p2" | grep -q "120.48 x 83.04" && ok "pdfinfo: page 2 is 120.48 x 83.04 pt" || bad "pdfinfo page 2 size: $p2"
  p4=$(pdfinfo -f 1 -l 4 "$DIR/pdf_multi.pdf" 2>&1 | grep "^Page *4 size:")
  echo "$p4" | grep -q "60.24 x 41.52" && ok "pdfinfo: page 4 (unknown dpi) is the nominal 300 dpi size" || bad "pdfinfo page 4 size: $p4"
fi

if need pdfimages; then
  out=$(pdfimages -list "$DIR/pdf_multi.pdf" 2>&1)
  echo "$out" | awk 'NR>2 && $3=="image"' | grep -q jpeg && ok "pdfimages: page 1 is a jpeg" || bad "pdfimages: no jpeg"
  echo "$out" | grep -q "ccitt" && ok "pdfimages: a ccitt page" || bad "pdfimages: no ccitt page"
  # Embedded JPEG bytes carry no EXIF or comment: extract and look.
  pdfimages -j "$DIR/pdf_jpeg.pdf" "$DIR/extracted" >/dev/null 2>&1
  if [ -f "$DIR/extracted-000.jpg" ]; then
    if grep -qa "Exif" "$DIR/extracted-000.jpg"; then bad "extracted JPEG still has EXIF"; else ok "extracted JPEG has no EXIF"; fi
  fi
  # Lossless pages come back pixel-exact.
  pdfimages -png "$DIR/pdf_flate_rgb.pdf" "$DIR/flate_rgb" >/dev/null 2>&1
  if [ ${#IM_COMPARE[@]} -gt 0 ] && [ -f "$DIR/flate_rgb-000.png" ]; then
    d=$(im_diff "$DIR/ref_rgb.png" "$DIR/flate_rgb-000.png")
    [ "$d" = "0" ] && ok "pdfimages extracts the Flate page exactly" || bad "pdfimages flate_rgb: $d pixels differ"
  fi
fi

if need pdftoppm; then
  # Render the G4 page onto exactly its own pixel grid: black must stay black (no inversion).
  pdftoppm -scale-to-x 251 -scale-to-y 173 -gray -png "$DIR/pdf_g4.pdf" "$DIR/render_g4" >/dev/null 2>&1
  f=$(ls "$DIR"/render_g4*.png 2>/dev/null | head -1)
  if [ -n "$f" ] && [ ${#IM_COMPARE[@]} -gt 0 ]; then
    d=$("${IM_COMPARE[@]}" -metric AE -fuzz 30% "$DIR/ref_bilevel.png" "$f" null: 2>&1 | head -1 | sed 's/[^0-9].*//')
    # A renderer may smooth an edge pixel here and there: allow 2% of the pixels (thin diagonals alias), far below the
    # 50% an inverted page would show.
    total=$((251 * 173))
    if [ -n "$d" ] && [ "$d" -le $((total / 50)) ]; then ok "Poppler renders the G4 page like the bitmap ($d of $total pixels differ)"; else bad "Poppler renders the G4 page differently ($d of $total pixels; inverted?)"; fi
  elif [ -n "$f" ]; then
    ok "Poppler renders the G4 page"
  else
    bad "pdftoppm did not render the G4 page"
  fi
fi

echo "---"
echo "$CHECKS checks, $FAILS failed"
[ "$FAILS" -eq 0 ]
