# 0012 - The Convert flow: one engine path for the CLI and the app, what a conversion replaces, colour and metadata

- **Status:** accepted (three deviations from PLAN 3.2.3 and 3.7 are marked for the owner below)
- **Date:** 2026-10-12
- **Roadmap items:** M2.36 (conversion replaces the source), M6.47, M6.49, M6.50, M6.56 (convert-only pipeline, flow, colour and location controls, `convert` subcommand), M6.29 (colour conversion), M11.40; the owner's original requirement "automated conversion between formats, especially HEIC/HEIF to JPG"
- **Decision log links:** B3 (conversion replaces the source, reversible), B12 (HEIC), B2 (no GPL), D3 (multi-frame sources are never replaced), C2 (sRGB default for HEIC to JPG, strip-location toggle)
- **Time box:** none. Code: `crates/engine/src/convert.rs`, `convert_batch.rs`, `colour.rs`, `crates/cli/src/convert.rs`, `crates/shell/src/app.rs` (Convert commands), `ui/src/routes/Convert.svelte`. Tests: `crates/engine/tests/convert.rs`, `convert_heif.rs`, `crates/cli/tests/convert.rs`, `colour/tests.rs`.

## Context

`Engine::convert_items` converted an *opened, analysed crop item* (HEIC to JPEG, BMP to PNG) and nothing used it. The owner's first requirement had no command, no screen and no tests with real conversions beyond one BMP.

## Decision

1. **One path for everything.** `Engine::convert_file(ConvertRequest)` takes a path and options, never an item: sniff and probe the header, decode once, convert the colours, encode with the save path's encoders and metadata policy, commit with the group commit (`commit_group_verified`, source unlinked last) or, when the new name is the source's own, the in-place commit (`commit_single`). The CLI (`auto-crop convert`) and the app (`convert_run` over a list held by the engine, `convert_batch.rs`) call it; `convert_items` is a thin wrapper that keeps its old contract (automatic target, only what the plan converts). The journal, fault points, backup store and `restore` are the ones the save path uses, so the existing fault-injection and restore-matrix suites cover them and the new `tests/convert.rs` adds crash/failure injection at every step for replace, in-place re-encode and keep.
2. **Placement.** `replace` (default, B3: verified backup, no-clobber commit, source unlinked last), `keep` (beside the source, nothing that exists changes, no backup), `folder` (the same, into a folder). A taken name gets `(2)`; nothing is overwritten. A conversion is a `OneToN` backup whose derived file is the converted one, so `restore` already offers Keep or Remove for it (CLI `--derived`, Backups view).
3. **Skip, not fail.** A file already in the target format is skipped (`SAME_FORMAT`) unless `reencode`; a HEIC/HEIF/AVIF file in a build without the decoder is `HEIF_UNAVAILABLE` (a skip with a clear message, found by the header sniff before anything is parsed); not-an-image is `UNSUPPORTED_FORMAT`. A source with several frames, and a CMYK source, is never *replaced* (`NOT_REPLACEABLE` plus the notice that says why); `keep` and `folder` convert its first frame and carry the notice.
4. **Colour** (`colour.rs`, `moxcms` 0.8.1, BSD-3-Clause OR Apache-2.0, pure Rust, already in the tree through `image`). `srgb` (default): a non-sRGB RGB profile is converted (relative colorimetric) and the output embeds the generated sRGB profile; a profile that maps a 5x5x5 probe grid to sRGB within one level *is* sRGB and is kept byte for byte; no profile means sRGB; an unreadable or non-RGB profile keeps pixels and profile with `colour.profile_unusable`. `preserve`: pixels and profile untouched. The EXIF `ColorSpace` tag is set to 1 when the pixels were converted. Accuracy against an independent f64 reference (profiles built byte by byte in the test, expected values from the standard primaries and curves, no moxcms in the reference): Display P3 to sRGB mean 0.027 levels, worst 1; Adobe RGB (1998) to sRGB mean 0.013 levels, worst 1, on 100 patches (primaries, grays, ColorChecker-like, clipping colours).
5. **Metadata.** EXIF of a JPEG source (as before) and now also of PNG (`eXIf`), WebP (`EXIF`), HEIC and AVIF (the `Exif` item, found through `iloc`, the 4-byte TIFF offset skipped) is carried into JPEG (APP1) and PNG (`eXIf`) with Orientation reset to 1 and the dimensions rewritten; `strip_location` removes GPS, and XMP/IPTC for JPEG output. HEIC orientation comes from `irot`/`imir` only (ADR-0009), so it is applied once.
6. **Alpha and depth.** The decode has a flattening mode (`decode_flattened_with`): translucent pixels are composited over white (`alpha.flattened`, instead of the crop path's `alpha.dropped`), for the libheif path too. More than 8 bits per channel is reduced with `depth.reduced_to_8`. Neither is a reason to refuse a conversion: the user chose a format that cannot hold them, the notice says so, and a replaced source is in the backup.
7. **Live Photo.** The still is converted; a paired `.MOV` with the same stem is never read, moved or changed, and the result carries `livephoto.video_untouched` (B12).

## Deviations from the plan, for the owner (A-4)

* **PLAN 3.2.3 "never replaced: WebP, AVIF, TIFF (writer ships later)".** That rule exists so a *same-format* save cannot silently turn a WebP into a PNG. In the Convert flow the user names the target, so TIFF, WebP and AVIF sources are replaced (after the verified backup) like HEIC. Multi-frame sources stay protected as above. Say if you want them copy-only here too: it is one match arm in `convert_file_inner`.
* **PLAN 3.7 "sRGB is the default only for HEIC to JPG".** The Convert flow offers the choice for every source and defaults to sRGB (`--colour preserve` / "Keep original colours" keeps the old behaviour). A converted Adobe RGB TIFF therefore changes pixel values by default.
* **PLAN 3.2.3 "alpha is kept for PNG".** The PNG writer here has no alpha channel (no new codec was in scope), so alpha is flattened on white for PNG too, with the notice. Transparent PNG to PNG is a `SAME_FORMAT` skip, so the only loss is from WebP, TIFF, AVIF and HEIC sources with transparency.

## Consequences and limits

* nclx-only wide-gamut HEIC files (no ICC profile) are not converted: the decoder reports `colour.nclx_not_srgb` and the pixels pass through; synthesising a profile from the nclx code points is not done. iPhone files carry an ICC profile and are converted.
* HDR (PQ/HLG) is not tone-mapped (`hdr.not_tonemapped`, ADR-0009).
* Metadata of a TIFF or BMP source is not carried (no reader); ICC is.
* A `--dry-run` reads headers only, so it cannot report decode-time notices, and it does not see names the run itself will take.
* Progress and cancel in the app work per file; a cancel finishes the files in flight.
