# Real-device HEIC corpus plan

Decision references: B12, B21. Design: [PLAN 3.4 and 7.5](../plan/03-image-io-formats.md). Roadmap: M0.49 (plan), M6 (use). The files are **private** and never enter the public repo.

## Goal

Cover the real variety of HEIC/HEIF files that users will drop in, so conversion fidelity and failure messages are tested on real device output, not only on conformance samples.

## What to collect (at least 6 per family, PROVISIONAL)

| Family | Variants wanted |
|---|---|
| iPhone | Display P3 stills; HDR with gain map; Live Photo still; burst frame; 48 MP grid-tiled image; portrait with depth map; EXIF-rotated portrait and landscape |
| Android / Samsung | HEIC stills; 10-bit HDR HEIC; other-vendor HEIF variants |
| Edge cases | Multi-image HEIF; image sequences; an unsupported codec inside HEIF (must fail with a named reason); truncated and corrupted files |

## Handling rules

- Files stay on the maintainer's **encrypted disk**; a BLAKE3 manifest records name-free identifiers, sizes and variant tags (see the golden-set CI dry run, roadmap M0.52).
- Debug copies have GPS and serial numbers stripped; the originals keep them under a recorded exception and are only processed on the maintainer's Mac (decision A-8).
- No file is published. Only aggregates (pass counts, per-variant outcomes) may appear in public results.

## Log

| # | Family | Variant | Device / OS | Size | Logged |
|---|---|---|---|---|---|
| | | | | | |

The first four files are logged here (variant tags only) when the owner provides them.
