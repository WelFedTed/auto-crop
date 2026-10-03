# Spike: flatten uneven illumination (ROADMAP M1.65)

Throwaway prototype of PLAN 5.3.1 steps 3-4 (illumination map and gain) on a ~1.5 MP luma proxy.
Own `[workspace]`, never in default CI. It only borrows the repo's synthetic page texture
(`auto_crop_imgproc::synth::paper_texture`). Run it with

    cargo run --release --manifest-path spikes/enhance/Cargo.toml          # table, exit 1 if a bar is missed
    cargo test --release --manifest-path spikes/enhance/Cargo.toml         # the same bars as tests
    cargo run --release --manifest-path spikes/enhance/Cargo.toml -- --sweep   # parameter sweep (tuning)

Status: **acceptance met on synthetic gradients, with the limits below.** Bars are PROVISIONAL
(residual shading <= 3% of the paper level, no barcode ghosts, gain <= 3x). Nothing here has seen a
photo or a receipt; real evidence is M1.67/M1.68.

## What was built

1. Luma, optionally area-averaged 2 x 2 (`pre`) before measuring (cuts noise; see findings).
2. Blocks of 32 px at 16 px stride. Per block a 256-bin histogram gives the 88th percentile `p` and
   `m`, the mean of the pixels at or above `p`.
3. Rejection (cells whose value is not paper): `p` below 0.60 x the median `p` of the cells within
   5 cells (black band); more than 35% of the block's full-resolution pixels below 0.70 x that local
   paper level (dense barcode, QR code, dense text); then a second stage that also rejects a cell
   dimmer than 0.93 x the valid cells around it when it touches two or more rejected cells.
4. Hole filling: push-pull for the first guess, then a weighted quadratic least-squares surface
   through the valid cells within 8 cells (Gaussian weights) for every rejected cell.
5. 3 x 3 median and a Gaussian of one block (2 cells) on the grid; odd reflection at the borders.
6. Gain `235 / M(x, y)` from the grid, bilinear with linear extrapolation in the outer half block,
   clamped to [1, 3]. Two estimate-and-apply rounds: the second measures the already flattened
   image and multiplies its gain in.

## Test pages

1000 x 1400 (1.4 MP) pages: the repo texture (text-like lines) plus a 380 x 110 px barcode (1-3 px
bars and gaps, about 50% ink), a 760 x 70 px black band with a bright text line, a 120 x 120 px
QR-like block (3 px modules), the whole page blurred 3 x 3 (a camera's MTF at proxy scale), then
multiplied by a smooth illumination field and given uniform-sum noise (sigma 2 levels). Truth is the
unlit page; paper is 235. Residual shading is measured per 32 x 32 cell over the truth-paper pixels:
|mean(output) - median of all cell means| / 235. The ghost zone is every cell within 40 px of the
barcode, band or QR block. A uniform offset of the paper level is the tone stage's job and is
printed separately as `bias`.

## Results (this machine, release build)

| Case | min light | worst cell | p95 | bias | worst ghost-zone cell | max gain |
|---|---:|---:|---:|---:|---:|---:|
| flat (control) | 1.00 | 0.12% | 0.06% | 0.00% | 0.10% | 1.00x |
| tilt 30% x + 10% y | 0.60 | 0.67% | 0.26% | -0.72% | 0.32% | 1.63x |
| tilt 55% x | 0.45 | 1.45% | 0.53% | -0.87% | 0.44% | 2.16x |
| vignette 45% | 0.55 | 1.27% | 0.78% | -0.70% | 0.25% | 1.79x |
| soft shadow 50%, 150 px edge | 0.50 | 2.02% | 1.08% | -0.85% | 2.02% | 2.01x |
| held-out: 3 other page seeds x {tilt 20%/35%, vignette 50%, shadow 45%/220 px} | 0.45-0.55 | 0.8-2.1% | 0.4-1.1% | about -0.8% | 0.4-2.1% | <= 2.18x |
| **negative control**: shadow 50%, rejection OFF | 0.50 | 9.04% | 2.27% | -0.53% | 9.04% (clipped at 255) | 3.00x |

All gating cases are at or under the 3% bar, the gain never exceeds 2.2x (cap 3x), and with
rejection switched off the band and the barcode leave a 9% (clipped at 255) halo, so the ghost metric can fail. Parameters
(block, passes, pre-average, fill radius) were tuned on page seed 11 and the first five cases only;
the held-out rows use other page seeds and other light parameters and passed with the final
settings (a first version of the rejection rules did **not**: see finding 4).

### Where it does not work (printed as informational, not gating)

| Case | worst cell |
|---|---:|
| hard shadow 60% over 60 px | 32% |
| steep shadow 40% over 120 px (3 held-out pages) | 3.4-4.8% |
| large uniform mid-grey area (a "photo") on flat light | 6.2% |

## Findings worth carrying into M4.36 / M7.06

1. **A high percentile inside a block is biased by the gradient across the block** (it reads the
   bright side), so one round under-corrects steep shadows. Further rounds on the already flattened
   image remove the rest: with a wider smoothing (3 cells) the shadow-edge worst cell went 3.5%,
   2.3%, 1.9% for 2, 3, 4 rounds (the default is 2 rounds with smoothing 2: 2.0%; more rounds cost
   time and were not needed for the bar).
2. **Hole filling matters more than the percentile.** Rejection removes about 27% of the cells on
   this page (the repo's synthetic text is denser than a real receipt: about 41% ink on its lines).
   Push-pull averaging flattens any gradient across a hole (vignette worst cell 3.7%, shadow edge
   10.8%); the local quadratic fit brings those to 1.3% and 2.0%. A fit radius of 5 cells is too small
   (shadow edge 4.1%) and 12 is too large (4.3%); 8 is a compromise tuned on one page.
3. **Edges of the page:** clamping the map at the border biased every cell within a few sigma of
   it (up to 4% on a vignette); odd reflection for the smoothing and linear extrapolation for the
   outer half block remove it.
4. **Rejection by ink share alone fails on fine barcodes.** Averaged 2 x 2 (and blurred), a 1-3 px
   barcode becomes a uniform mid-grey whose own 88th percentile is about 85% of paper. Counting ink on
   the full-resolution pixels against the neighbours' paper level catches most blocks, but a few
   sparse columns slipped through on some seeds (a 4.6% ghost on a held-out page) until the second
   stage (dim and touching rejected cells) was added.
5. **The PLAN's global rule (`p` under 0.60 x the median of valid blocks) cannot be used as written:**
   a 0.45 shadow is below it. The spike compares with the median of the neighbouring cells instead.
6. **The mean above the 88th percentile reads high by about 1.7 sigma of the sensor noise**, and the
   dark side of a shadow, where the later gain amplifies noise, by more: a -0.7 to -0.9% offset
   between the lit and the dark side (printed as `bias`). Averaging 2 x 2 before measuring helps
   (worst cells 0.7/1.5/1.3/2.0% against 1.0/2.0/1.5/2.4% without; 4 x 4 gave about the same as 2 x 2).
   The white-point stage removes the uniform part; the spread is inside the 3% bar here.
7. **Limits:** a shadow edge steeper than about 0.5 % of the paper level per pixel at the proxy
   (about 40 % over 120 px) leaves 3-5% residual; large uniform grey regions are read as paper
   in shade (luma cannot tell them apart; the PLAN's chroma and ink rules do not help for grey). A
   real flatten needs a guard for both (for example an edge-sharpness test across the shadow edge,
   or "keep gain at 1 where a large area stays below 0.6 of the page level").
8. **Speed is not met:** 80-190 ms for both rounds on this loaded, unoptimised host against the M7.06
   target of grid <= 15 ms at 1.5 MP. The prototype is scalar, allocates per cell and fits a 6 x 6
   system for every rejected cell. No budget claim.

Not tested: colour (the spike is luma only; PLAN 5.3.1 uses per-channel gain), real noise and JPEG
blocking, perspective-warped pages and the validity mask.
