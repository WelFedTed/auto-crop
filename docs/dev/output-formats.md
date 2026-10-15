# Output formats: TIFF, lossless WebP and PDF (engineering notes)

Roadmap: M11.20 to M11.21 (lossless half), M11.29, M11.30 (one page), M11.32 to M11.35, M11.38, M11.39, M11.40, M11.56 to M11.59 (Linux slice); M7.28 to M7.31 (1-bit output). Design: [PLAN 3.2.3, 3.3, 3.6](../plan/03-image-io-formats.md). Decision B11: "support all image formats", exports JPEG, PNG, WebP, AVIF, TIFF (including 1-bit CCITT G4) and PDF (JPEG XL later). This file says what exists, how it is checked, and what does not exist yet.

## What can be written

| Format | Writer | Where | Notes |
|---|---|---|---|
| JPEG, PNG | `JpegTurboEncoder` / `JpegRsEncoder`, `PngEncoder` | `crates/codecs/src/encoders.rs` | unchanged |
| TIFF | `TiffEncoder` (own, pure Rust) | `encoders/tiff_encoder.rs` | none, LZW, Deflate, PackBits, CCITT Group 4; 8 and 16 bit grey and RGB |
| WebP (lossless) | `WebpEncoder` over `image-webp` | `encoders/webp.rs` | VP8L only; **no lossy WebP in this build** |
| PDF | `PdfWriter`, `PdfEncoder` (own, pure Rust) | `encoders/pdf.rs` | one image per page; multi-page in the writer |
| Scan to PDF | `Engine::save_run_pdf`, `PdfRun` | `crates/engine/src/run_pdf.rs` | many crops, one multi-page PDF; see the Scan to PDF section |
| 1-bit | `BilevelImage` + `fax` Group 4 | `encoders/bilevel.rs` | shared by the TIFF and PDF writers |
| AVIF (lossy) | `AvifEncoder` over `rav1e` + own container | `encoders/avif.rs`, `avif_box.rs` | feature `avif`; see the AVIF section |

Not written yet: lossy WebP (needs libwebp, a C library built with CMake in CI: a cargo feature like `turbojpeg`, M11.21), JPEG XL (M11.25), BigTIFF, multi-page TIFF, TIFF alpha, the PDF page-size presets and the "combine N results into one file" front ends (the PDF writer itself takes any number of pages; M11.31, M11.54).

No `unsafe`, no new native code, no network. New dependencies: `fax` 0.2 (MIT, was optional), `weezl` 0.1 (MIT OR Apache-2.0, LZW; already in the tree through `tiff`), `image-webp` 0.2 (MIT OR Apache-2.0; already in the tree through `image`). `cargo deny check` passes.

## TIFF

Classic TIFF, little endian, one directory written last, strips first (about 256 KiB of raw samples each; a Group 4 page is one strip unless asked otherwise). Tags: ImageWidth/Length, BitsPerSample, Compression, Photometric (1 grey, 2 RGB, 0 for Group 4), StripOffsets, **Orientation 1** (the pixels were turned once, PLAN 3.6), SamplesPerPixel, RowsPerStrip, StripByteCounts, XResolution/YResolution/ResolutionUnit, PlanarConfiguration, Software (`Auto Crop`, no version), Predictor (2 for LZW and Deflate; a TIFF source keeps its own compression class unless one is chosen, else LZW), T6Options for Group 4, the ICC profile (34675, byte-exact) and the carried tags below.

* **Resolution** comes from the source (PNG `pHYs`, TIFF resolution tags, JPEG JFIF). Unknown: `1/1` with unit "none". A dpi is never invented (PDF alone uses a nominal 300).
* **Carried tags** (a TIFF source only; `strip_all` drops them, `strip_location` drops XMP, IPTC and the free-text ones): ImageDescription, Make, Model, DocumentName, DateTime, Artist, HostComputer, Copyright, XMP (orientation reset) and IPTC. **EXIF and GPS directories are not written.** A TIFF source that has one (or a sub-image directory, or is BigTIFF) is therefore never replaced in place: decode notice `tiff.exif_not_carried`; a copy is still possible and says it carries no Exif.
* **Grey from colour:** an RGB raster whose three channels are equal and that has no ICC profile is written as one grey channel (the decoder gives identical RGB back, so the verify step stays exact).
* **Group 4 polarity** is the classic bug. The row coder (`fax`) emits white and black runs; the file says WhiteIsZero (0), so a black mark on white stays black. Checked in four readers (below). The decode side of the `tiff` crate returns Group 4 as BlackIsZero; the tests assert that.
* **1-bit** (`EngineOptions::one_bit`, CLI `--one-bit`): the enhanced grey page is thresholded at 128 (luma below is black), no dithering. TIFF and PDF only; any other output, and any colour page, is `UnsupportedOutput`, never a silent 8-bit file. Width limit 65,535 px (the Group 4 coder is `u16`).

## WebP

`image-webp` VP8L, 8-bit grey or RGB, dimensions up to 16,383. ICC in `ICCP`, the patched EXIF in `EXIF` (Orientation reset, size rewritten, GPS per policy), no XMP yet. There is no lossy mode: the quality of the spec is ignored and the UI calls it "WebP (lossless)". A lossy WebP **source** (`webp.lossy`, set by the header parse) is never replaced in place (a lossless re-encode would be a different, larger file); its copies are PNG.

## PDF

Own writer, about 1,000 lines with tests, classic xref, deterministic object numbers (1 catalogue, 2 page tree, 3 info, then image, content and page per page). Pages stream to the sink; the tree, info, xref and trailer come last. One image XObject per page and a content stream that only draws it. No text, fonts, annotations, JavaScript, `/Metadata`, JBIG2; the information dictionary has `Producer (Auto Crop)` and, when asked, `Title` and `CreationDate` (none by default, so equal input gives equal bytes).

* **JPEG pages** (`DCTDecode`): the encoder's bytes, with APP1 to APP13, APP15 and COM dropped (EXIF, XMP, IPTC, the ICC `APP2`, comments); JFIF and Adobe `APP14` stay. 8-bit, 1 or 3 components, baseline or progressive Huffman only; CMYK, 12-bit and arithmetic streams are refused. The profile goes into an `ICCBased` colour space when its colour space matches the components.
* **Lossless pages** (`FlateDecode`, `/Predictor 15`): 8-bit grey or RGB with adaptive PNG row filters (the img2pdf technique), about as small as the PNG of the same pixels.
* **Group 4 pages** (`CCITTFaxDecode`, `/K -1`, default `/BlackIs1 false`): 1-bit, the same bytes as the TIFF.
* **Page size** = pixels over dpi times 72 points; 300 dpi (flagged) when unknown; at most 14,400 and at least 3 points per side (scaled and flagged).
* **Which page type the engine picks:** a colour picture is a JPEG page at the save's quality; an enhanced grey page is a lossless Flate page, or Group 4 with `one_bit`. (A lossless colour PDF is not offered yet.)
* **Verification** (`encoders::pdf::read_pages`, run by the commit protocol's verify step through `commit::verify_bytes`): header, `startxref`, every xref offset pointing at its `N 0 obj`, trailer size and root, page tree and count, `/Length` of every stream, no text operator in the content stream; then the page image is decoded again and compared (exact blake3 for lossless pages, the luma fingerprint for JPEG pages) and the profile must come back byte-exact. It parses what this writer makes, not arbitrary PDFs.

## AVIF

`AvifEncoder` (feature `avif`; the `auto-crop-cli` and `auto-crop-shell` crates turn it on by default, the library crates do not). It drives **rav1e 0.8.1 directly** and writes its own ISOBMFF container (`avif_box.rs`, writer and reader, no dependency). `ravif` (the plan's choice) is not used: it always writes 4:4:4, has no ICC route, and its container crate `avif-serialize` writes `colr` as `nclx` only, so the "own ISOBMFF box insertion" fallback of PLAN 3.3 would have been needed anyway; driving rav1e is shorter than patching `iloc` offsets in someone else's output. rav1e is built **without its `asm` feature** (that needs `nasm`, which only CI has): pure Rust, and the package build needs no assembler.

* **Pixels:** 8-bit, full range, BT.601 (nclx matrix 6, the most widely decoded choice); 4:2:0 (default) or 4:4:4 for colour, **4:0:0 (one plane) for a grey page** and for a colour raster whose channels are equal and that has no profile. Alpha is not written (the engine's rasters are flattened; `alpha.flattened` already says so). Odd sizes are exact (chroma planes round up); 1 x 1 works.
* **Boxes:** `ftyp avif/mif1/miaf`; `meta` with `hdlr pict`, `pitm`, `iloc` (v0), `iinf`, `iref cdsc`, `iprp` (`ispe`, `pixi`, `av1C` from rav1e's own sequence header, `colr`); `mdat`. **No `irot`, no `imir`, no `clap`**: the pixels were turned once. `colr`: without a profile `nclx` sRGB (primaries 1, transfer 13); with one a `prof` box with the profile **byte-exact** next to an `nclx` that claims only the matrix (primaries and transfer unspecified, as libavif does). EXIF: an `Exif` item with the 4-byte TIFF-offset prefix, Orientation 1, GPS per the metadata policy (`strip_metadata` drops it). XMP, pixel density: not written.
* **Quality** (`QualitySetting::avif_quality`, PROVISIONAL): Small 60, Balanced 75, Best 88, or the fixed number; unlike JPEG it does not follow the source's quality. `quality_to_quantizer` maps 1 to 100 to rav1e's base quantiser index with the curve of the `cavif` tool (75 -> 128, 90 -> 66, 100 -> 0). A fixed quantiser for the whole picture (no rate control).
* **Speed:** rav1e preset 0 to 10, `EngineOptions::avif_speed`, CLI `--avif-speed`, default **8**. Tiles (never smaller than 256 x 256 px) are the only parallelism inside one frame.
* **Cancellation:** rav1e cannot stop inside a frame. The encode runs on its own thread, the caller polls every 20 ms, and a cancel returns `Cancelled` at once; the thread finishes its frame and drops the result (a test checks the return is well under the encode time). The engine's save and convert paths do not pass the batch token into the encoder yet, so a cancel there takes effect between files (M11.24 is not done).
* **Verify:** the commit protocol's verify step re-reads the temp (blake3), then `avif_box::read_info` checks the primary picture's size, the profile byte-exact, no rotation property, an AV1 OBU header, EXIF Orientation 1. A build with the libheif decoder (`heif`) also decodes the picture and compares the luma fingerprint. Without it the check is structural: the file's pixels are verified by the independent decoders in CI, not in the shipped app.
* **Replace in place: never.** AVIF is lossy, so an AVIF source stays open-only (`format.write_unavailable`, `Format::is_encodable` is false for AVIF) and its default copy is PNG; choosing AVIF is explicit (`SaveFormat::Avif`, Convert target, `--format avif`). Converting any other source to AVIF with "replace" goes through the same verified backup and Restore as any conversion (the loss is the user's choice; notice `avif.lossy`).

**Measured** (this machine: Windows 11, 12 threads; a synthetic 4000 x 3000 picture of multi-octave noise, harder than most photographs: 1.5 bpp at quality 75; `cargo run --release -p auto-crop-codecs --features avif,fixtures --example avif_bench`):

| Setting | Time | Size |
|---|---|---|
| speed 10, 4:2:0, q75 | 4.0 s | 2.29 MB |
| speed 8 (default), 4:2:0, q75 | 7.0 s | 2.29 MB |
| speed 6, 4:2:0, q75 | 13.2 s | 2.30 MB |
| speed 8, 4:4:4, q75 | about 2x the 4:2:0 time | 4.30 MB |

The same runs were 2.5x slower while other builds were running on the machine; peak RSS of the 4:2:0 speed 8 run was 311 MB (including the 36 MB source picture). The PROVISIONAL gate "AVIF default <= 10 s at 12 MP" holds on this machine and is **not** established for the Tier-M reference machine. The CLI release binary grows from 6.23 MB to 7.55 MB (+1.3 MB, +21%) with the feature and a cold build takes about a minute longer, which is why the feature is on by default for the CLI and the app (B10 keeps AVIF in 1.0) and off for the library crates.

**Independent decoders** (`tools/interop/check-avif.py`, CI job `Output formats`, plus the `heif` workflow): on the first run here Pillow 12.3.0 (libavif with dav1d) and ImageMagick 7.1.2 (libheif) decode every sample at the right size; PSNR against the picture the encoder was given, quality 80, 251 x 173 synthetic photo: 4:4:4 39.8 dB (bar 38), 4:2:0 39.7 dB (bar 36), grey 44.3 dB (bar 40); the profile and the EXIF come back byte-exact; Orientation 1; odd sizes down to 1 x 1 decode. `avifdec` was not installed on this machine: the CI job installs it (`libavif-bin`) and must be green. The repo's own libheif/dav1d path decodes the same files in the `heif` workflow (`libheif_decodes_what_this_writer_makes`, PSNR >= 33 dB at 160 x 120, both chroma layouts).

## Scan to PDF (many crops, one document)

Roadmap: M11.11 (`ManyToOne`), M11.31, M11.36 to M11.38. `Engine::save_run_pdf(selection, out, options, cancel, progress)` writes the included crops of the selected scans as the pages of ONE PDF; `Engine::begin_run_pdf` gives the same thing one scan at a time (`PdfRun::add`, `finish`), which is what the CLI feeds as its workers finish.

* **Pages:** each selected scan's included crops in their own reading order, the scans in the order given (the review grid's order; the CLI's `--order name|given`). Every crop is rendered as it is saved in the edit state: quad, curved page, enhancement. A page's size is pixels over the density times 72 points (`PdfRunOptions::dpi`, else the source's, else the nominal 300, with the notice `pdf.dpi_assumed`).
* **Page types:** an unedited JPEG crop that the lossless transform cuts exactly (the whole frame, or a cut on block boundaries) is embedded **as it is**, no new generation (`pdf.jpeg_pass_through`, `PdfRunSaved::pass_through_pages`); other colour crops are JPEG pages at the save's quality; enhanced grey pages are lossless Flate, or CCITT Group 4 with `one_bit` (an unenhanced colour crop with `one_bit` is `UnsupportedOutput`, never a silent colour page in a black-and-white document).
* **Memory:** one page at a time. The scan being cut is decoded once for its consecutive pages; nothing else is kept. `run_pdf_memory.rs` measures the peak heap of a 4-page and a 40-page run of 1.1 MP noise pages: 12.8 MB and 12.95 MB while the file grows from 3.2 MB to 31.6 MB (the writer streams each page to the temp file).
* **Safe write** (copy semantics, so no backup; the originals are only read): the document goes to `.autocrop-<id>.tmp` beside the target and is synced; then the parse-back: the file's length and blake3 equal what the writer hashed while writing; `pdf::read_pages_streaming` re-reads the structure with seeks (header, trailer, every cross-reference offset, page tree, stream framing; one page in memory at a time) and each page must have the codec, size, media box and profile that were planned, its stream must hash to the stream that was written, and (`VerifyMode::Full`) it must decode to the exact pixels (Flate, Group 4) or the luma fingerprint (JPEG). Only then is the temp moved to its name with a no-clobber hard link: **an existing file is never replaced**; the planner chooses `Name (2).pdf` (`pdf.renamed`) and a name that appears between planning and the move is planned again. Any error, a cancel (checked before every page and between steps) or a drop removes the temp and leaves no output. One failed page fails the file.
* **Tests:** `crates/engine/tests/run_pdf.rs` (three crops: a straight JPEG crop, a curved one, an enhanced one, give three pages in the order given with the right sizes, the enhanced page pixel-equal to the same crop written as a PNG; dpi option; pass-through equal to the source's pixels and stream; collisions `(2)`, `(3)`; a failure injected at every step; cancel; the parse-back catching a flipped byte, a truncation and appended junk before the name is taken; typed errors; Group 4; progress), `run_pdf_memory.rs`, and the codecs tests of `read_pages_streaming` (same answer as `read_pages` on every damage case, a 1,000-page document read with one page in memory).
* **Front ends:** the review grid's selection bar has "Save as one PDF" (shell commands `save_pdf`, `cancel_pdf`, `open_pdf_folder`; the Save dialog is Rust-side and starts in the first scan's folder; the webview gets the file name, never the path; progress in `pdf-progress` events; a toast with "Show in folder"). The CLI: `process --pdf FILE`.

## What may be replaced in place now

`fsplan::replace_refusal` is still the only rule. A source is replaced when it is a single-frame image and none of the decode notices `LOSSY_DECODE_NOTICES` is present:

| Source | Replaced? |
|---|---|
| JPEG, PNG | as before |
| TIFF, single page, 8-bit | **yes**, after the verified backup (density, profile and descriptive tags kept) |
| WebP, still, lossless | **yes** |
| TIFF with `tiff.low_depth` (1-bit), `tiff.exif_not_carried`, `depth.reduced_to_8`, `alpha.dropped`, `cmyk.naive_conversion` | no, with that notice |
| WebP with `webp.lossy`, `depth.reduced_to_8`, `alpha.dropped` | no, with that notice |
| multi-page TIFF, animated WebP/PNG/GIF, HEIC, AVIF (lossy writer: never replaced), BMP, GIF | no (unchanged) |

The 16-bit and alpha refusals stay: the engine's raster is 8-bit RGB, so a writer that could hold 16 bit (the TIFF encoder can) is never given the pixels. Converting to another format is the Convert flow (`convert --to`), not an in-place save. Copies keep the source's format when it can be written back as itself (`output::copy_format`): TIFF stays TIFF, lossless WebP stays WebP, a lossy WebP or a 1-bit TIFF gives PNG.

## Choosing the format

* **Convert screen and `auto-crop convert --to jpg|png|tiff|webp|pdf|avif`**: any decodable source; replace (verified backup, restore), keep, or a folder.
* **Editor "Save as"** (`Settings.save_format`): applies to copies. Another format than the source's turns "Replace original" off (`format.change_needs_copy` in the engine, the gate `format-change` in the UI), because replacing under another name is a conversion.
* **`process --format tiff|webp|pdf|avif`**, `render --format` or the output extension: copies only, like the existing `--format`.
* `--tiff-compression none|lzw|deflate|packbits`, `--one-bit`, `--avif-speed 0-10`, `--avif-chroma 420|444`.

## How it is checked

* Writer tests (`crates/codecs/src/encoders/tests_formats.rs`, `tests_pdf.rs`, `tests_notices.rs`): every compression and layout round-trips exactly in the `tiff` crate and the repo's decoder (8 and 16 bit, strips of 1, 7 and 256 KiB), tags (Orientation 1, ICC byte-exact, dpi), Group 4 polarity against `fax`, `tiff` and the repo decoder for several strip layouts, PackBits unpacking, WebP exact in `image-webp` and the repo decoder, PDF structure and every damage case (truncation at every 7th byte, a wrong offset, a bad length, trailing junk), page sizes, profile handling, refusals (CMYK, arithmetic, wrong sample count), 1,000 pages in a counting sink.
* Engine (`crates/engine/tests/output_formats.rs`, `formats.rs`, `restore_matrix.rs`, `single_faults.rs`): Convert to every new target, restore byte for byte, TIFF re-save keeping density/profile/tags by policy, one-bit and its refusals, the PDF verify step, the Save format. The restore matrix and the single-save fault matrix now run TIFF and WebP rows (every step failed and crashed in turn).
* CLI (`crates/cli/tests/output_formats.rs`): `convert --to`, `process --format`, in-place TIFF and WebP with restore, `--tiff-compression`, `--one-bit`.
* UI (`ui/src/lib/output-formats.test.ts`, `strings.test.ts`): targets and Save formats against the Rust enums, planned names, the gate, the notice copy.
* **Independent readers** (`tools/interop/check-output-formats.sh`, CI job `Output formats` on Ubuntu with `tiffinfo`, ImageMagick, `dwebp`, `qpdf`, Poppler; `cargo run -p auto-crop-codecs --features fixtures --example write_samples -- DIR` writes the samples). Results of the first run, on Windows: ImageMagick 7.1.2 (libtiff, libwebp) reads all 8 TIFFs, both Group 4 files (black stays black, 600 dpi, orientation TopLeft) and both WebPs pixel-identical to the reference; Poppler 25.07 reports page counts, sizes (60.24 x 41.52 pt at 300 dpi, nominal 300 for unknown), producer and title, extracts the JPEG without EXIF and the Flate page pixel-exact and renders the Group 4 page matching the bitmap; Pillow 12.3 (libtiff, libwebp) reads TIFFs and WebPs identical with the right compression and dpi; **libqpdf (bundled in pikepdf 10.17, the same checks as `qpdf --check`) reports no problems for the five PDFs**. `tiffinfo`, `dwebp` and the `qpdf` binary were not on that machine: the CI job runs them and must be green on main.

## Limits and follow-ups

Lossy WebP and JPEG XL are not written (M11.21 lossy half, M11.25 to M11.27). AVIF: no alpha, no XMP, no 10-bit, no HDR, no animation, no grid for pictures above rav1e's limits (65,535 px per side is the cap we set); cancel is between files in the engine. TIFF carries no EXIF/GPS/thumbnail, no alpha, no BigTIFF, one page. The PDF has no page-size presets, no multi-page front end (the writer is ready), and the engine writes lossy JPEG pages for colour pictures only. WebP carries no XMP. Fuzz targets for the writers (`tiff_multipage_writer`, `pdf_writer`, M11.66) are not written; the decoders' fuzz seeds are unchanged. Performance and memory of the writers are unmeasured (M11.61, M11.64, M11.65). The Windows, macOS and Linux behaviour is identical (pure Rust), but only Windows and the Linux CI job have run it.
