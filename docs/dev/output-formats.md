# Output formats: TIFF, lossless WebP and PDF (engineering notes)

Roadmap: M11.20 to M11.21 (lossless half), M11.29, M11.30 (one page), M11.32 to M11.35, M11.38, M11.39, M11.40, M11.56 to M11.59 (Linux slice); M7.28 to M7.31 (1-bit output). Design: [PLAN 3.2.3, 3.3, 3.6](../plan/03-image-io-formats.md). Decision B11: "support all image formats", exports JPEG, PNG, WebP, AVIF, TIFF (including 1-bit CCITT G4) and PDF (JPEG XL later). This file says what exists, how it is checked, and what does not exist yet.

## What can be written

| Format | Writer | Where | Notes |
|---|---|---|---|
| JPEG, PNG | `JpegTurboEncoder` / `JpegRsEncoder`, `PngEncoder` | `crates/codecs/src/encoders.rs` | unchanged |
| TIFF | `TiffEncoder` (own, pure Rust) | `encoders/tiff_encoder.rs` | none, LZW, Deflate, PackBits, CCITT Group 4; 8 and 16 bit grey and RGB |
| WebP (lossless) | `WebpEncoder` over `image-webp` | `encoders/webp.rs` | VP8L only; **no lossy WebP in this build** |
| PDF | `PdfWriter`, `PdfEncoder` (own, pure Rust) | `encoders/pdf.rs` | one image per page; multi-page in the writer |
| 1-bit | `BilevelImage` + `fax` Group 4 | `encoders/bilevel.rs` | shared by the TIFF and PDF writers |

Not written yet: lossy WebP (needs libwebp, a C library built with CMake in CI: a cargo feature like `turbojpeg`, M11.21), AVIF (M11.22), JPEG XL (M11.25), BigTIFF, multi-page TIFF, TIFF alpha, the PDF page-size presets and the "combine N results into one file" front ends (the PDF writer itself takes any number of pages; M11.31, M11.54).

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

## What may be replaced in place now

`fsplan::replace_refusal` is still the only rule. A source is replaced when it is a single-frame image and none of the decode notices `LOSSY_DECODE_NOTICES` is present:

| Source | Replaced? |
|---|---|
| JPEG, PNG | as before |
| TIFF, single page, 8-bit | **yes**, after the verified backup (density, profile and descriptive tags kept) |
| WebP, still, lossless | **yes** |
| TIFF with `tiff.low_depth` (1-bit), `tiff.exif_not_carried`, `depth.reduced_to_8`, `alpha.dropped`, `cmyk.naive_conversion` | no, with that notice |
| WebP with `webp.lossy`, `depth.reduced_to_8`, `alpha.dropped` | no, with that notice |
| multi-page TIFF, animated WebP/PNG/GIF, HEIC, AVIF, BMP, GIF | no (unchanged) |

The 16-bit and alpha refusals stay: the engine's raster is 8-bit RGB, so a writer that could hold 16 bit (the TIFF encoder can) is never given the pixels. Converting to another format is the Convert flow (`convert --to`), not an in-place save. Copies keep the source's format when it can be written back as itself (`output::copy_format`): TIFF stays TIFF, lossless WebP stays WebP, a lossy WebP or a 1-bit TIFF gives PNG.

## Choosing the format

* **Convert screen and `auto-crop convert --to jpg|png|tiff|webp|pdf`**: any decodable source; replace (verified backup, restore), keep, or a folder.
* **Editor "Save as"** (`Settings.save_format`): applies to copies. Another format than the source's turns "Replace original" off (`format.change_needs_copy` in the engine, the gate `format-change` in the UI), because replacing under another name is a conversion.
* **`process --format tiff|webp|pdf`**, `render --format` or the output extension: copies only, like the existing `--format`.
* `--tiff-compression none|lzw|deflate|packbits`, `--one-bit`.

## How it is checked

* Writer tests (`crates/codecs/src/encoders/tests_formats.rs`, `tests_pdf.rs`, `tests_notices.rs`): every compression and layout round-trips exactly in the `tiff` crate and the repo's decoder (8 and 16 bit, strips of 1, 7 and 256 KiB), tags (Orientation 1, ICC byte-exact, dpi), Group 4 polarity against `fax`, `tiff` and the repo decoder for several strip layouts, PackBits unpacking, WebP exact in `image-webp` and the repo decoder, PDF structure and every damage case (truncation at every 7th byte, a wrong offset, a bad length, trailing junk), page sizes, profile handling, refusals (CMYK, arithmetic, wrong sample count), 1,000 pages in a counting sink.
* Engine (`crates/engine/tests/output_formats.rs`, `formats.rs`, `restore_matrix.rs`, `single_faults.rs`): Convert to every new target, restore byte for byte, TIFF re-save keeping density/profile/tags by policy, one-bit and its refusals, the PDF verify step, the Save format. The restore matrix and the single-save fault matrix now run TIFF and WebP rows (every step failed and crashed in turn).
* CLI (`crates/cli/tests/output_formats.rs`): `convert --to`, `process --format`, in-place TIFF and WebP with restore, `--tiff-compression`, `--one-bit`.
* UI (`ui/src/lib/output-formats.test.ts`, `strings.test.ts`): targets and Save formats against the Rust enums, planned names, the gate, the notice copy.
* **Independent readers** (`tools/interop/check-output-formats.sh`, CI job `Output formats` on Ubuntu with `tiffinfo`, ImageMagick, `dwebp`, `qpdf`, Poppler; `cargo run -p auto-crop-codecs --features fixtures --example write_samples -- DIR` writes the samples). Results of the first run, on Windows: ImageMagick 7.1.2 (libtiff, libwebp) reads all 8 TIFFs, both Group 4 files (black stays black, 600 dpi, orientation TopLeft) and both WebPs pixel-identical to the reference; Poppler 25.07 reports page counts, sizes (60.24 x 41.52 pt at 300 dpi, nominal 300 for unknown), producer and title, extracts the JPEG without EXIF and the Flate page pixel-exact and renders the Group 4 page matching the bitmap; Pillow 12.3 (libtiff, libwebp) reads TIFFs and WebPs identical with the right compression and dpi; **libqpdf (bundled in pikepdf 10.17, the same checks as `qpdf --check`) reports no problems for the five PDFs**. `tiffinfo`, `dwebp` and the `qpdf` binary were not on that machine: the CI job runs them and must be green on main.

## Limits and follow-ups

Lossy WebP, AVIF and JPEG XL are not written (M11.21 lossy half, M11.22 to M11.27). TIFF carries no EXIF/GPS/thumbnail, no alpha, no BigTIFF, one page. The PDF has no page-size presets, no multi-page front end (the writer is ready), and the engine writes lossy JPEG pages for colour pictures only. WebP carries no XMP. Fuzz targets for the writers (`tiff_multipage_writer`, `pdf_writer`, M11.66) are not written; the decoders' fuzz seeds are unchanged. Performance and memory of the writers are unmeasured (M11.61, M11.64, M11.65). The Windows, macOS and Linux behaviour is identical (pure Rust), but only Windows and the Linux CI job have run it.
