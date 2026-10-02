// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Proxy pyramid and tiles (ROADMAP M1.22, PLAN 7.3 rank 1). One full-resolution decode feeds
//! four proxies, each an area-average reduction, none ever larger than the source:
//!
//! | Level | Target | Used for |
//! |---|---|---|
//! | [`Level::Thumb`] | long edge <= 256 | filmstrip and grid thumbnails |
//! | [`Level::Detect`] | long edge <= 1024 | corner net and classical detector input |
//! | [`Level::Analysis`] | about 1.5 MP, long edge <= 3072 | illumination, orientation, skew |
//! | [`Level::Display`] | about 3 MP (the 2-4 MP window), long edge <= 4096 | on-screen preview |
//!
//! Each level is reduced from the next larger one (display from the source, analysis from
//! display, and so on), so the whole pyramid costs little more than one pass over the source, and
//! levels whose target is not smaller than their parent share it instead of copying. Sizes follow
//! the source's aspect ratio; a source already below a target keeps its own size for that level
//! (no upscaling). [`Pyramid::tiles`] cuts a level into square tiles whose rebuild is bit-exact.

use crate::Raster;
use crate::scale::resize_area;
use std::sync::Arc;

/// The four proxy levels, largest to smallest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Display,
    Analysis,
    Detect,
    Thumb,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Display, Level::Analysis, Level::Detect, Level::Thumb];

    fn index(self) -> usize {
        self as usize
    }
}

/// Long-edge cap of the thumbnail.
pub const THUMB_EDGE: u32 = 256;
/// Long-edge cap of the detection proxy.
pub const DETECT_EDGE: u32 = 1024;
/// Target pixel count of the analysis proxy.
pub const ANALYSIS_PIXELS: f64 = 1.5e6;
/// Long-edge cap of the analysis proxy.
pub const ANALYSIS_EDGE: u32 = 3072;
/// Target pixel count of the display proxy (inside the 2-4 MP window).
pub const DISPLAY_PIXELS: f64 = 3.0e6;
/// Long-edge cap of the display proxy.
pub const DISPLAY_EDGE: u32 = 4096;

/// Output size for `level` of a `width` x `height` source: never larger than the source, never
/// below 1 x 1, aspect ratio preserved up to rounding.
pub fn level_size(width: u32, height: u32, level: Level) -> (u32, u32) {
    let (w, h) = (f64::from(width), f64::from(height));
    let long = w.max(h);
    if width == 0 || height == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = match level {
        Level::Thumb => f64::from(THUMB_EDGE) / long,
        Level::Detect => f64::from(DETECT_EDGE) / long,
        Level::Analysis => (ANALYSIS_PIXELS / (w * h))
            .sqrt()
            .min(f64::from(ANALYSIS_EDGE) / long),
        Level::Display => (DISPLAY_PIXELS / (w * h))
            .sqrt()
            .min(f64::from(DISPLAY_EDGE) / long),
    }
    .min(1.0);
    (
        ((w * scale).round() as u32).clamp(1, width),
        ((h * scale).round() as u32).clamp(1, height),
    )
}

/// A square-ish piece of a level: its top-left corner and pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub raster: Raster,
}

/// The proxies of one source image.
#[derive(Debug, Clone)]
pub struct Pyramid {
    source: (u32, u32),
    levels: [Arc<Raster>; 4],
}

impl Pyramid {
    /// Builds all four proxies from the full-resolution `source` (taken by value so a level equal
    /// to the source can share it).
    pub fn build(source: Raster) -> Pyramid {
        Self::build_shared(Arc::new(source))
    }

    /// Like [`Pyramid::build`] for a source the caller keeps (the full-resolution raster stays
    /// available for the rectify stage without a copy).
    pub fn build_shared(source: Arc<Raster>) -> Pyramid {
        let dims = (source.width, source.height);
        let mut parent = source;
        let mut levels: [Option<Arc<Raster>>; 4] = [None, None, None, None];
        for level in Level::ALL {
            let (w, h) = level_size(dims.0, dims.1, level);
            let img = if (w, h) == (parent.width, parent.height) {
                Arc::clone(&parent)
            } else {
                Arc::new(resize_area(&parent, w, h))
            };
            levels[level.index()] = Some(Arc::clone(&img));
            parent = img;
        }
        Pyramid {
            source: dims,
            levels: levels.map(|l| l.expect("every level was built")),
        }
    }

    /// Size of the full-resolution source.
    pub fn source_size(&self) -> (u32, u32) {
        self.source
    }

    pub fn level(&self, level: Level) -> &Raster {
        &self.levels[level.index()]
    }

    /// Output pixels per source pixel along the long edge of `level` (at most 1).
    pub fn scale(&self, level: Level) -> f64 {
        let r = self.level(level);
        f64::from(r.width.max(r.height)) / f64::from(self.source.0.max(self.source.1).max(1))
    }

    /// `level` cut into `tile` x `tile` tiles in row-major order (right and bottom tiles are
    /// smaller when the size is not a multiple).
    pub fn tiles(&self, level: Level, tile: u32) -> TileIter<'_> {
        tiles(self.level(level), tile)
    }
}

/// Iterator over the tiles of a raster.
#[derive(Debug, Clone)]
pub struct TileIter<'a> {
    src: &'a Raster,
    tile: u32,
    next: u64,
    cols: u32,
    rows: u32,
}

/// Cuts `src` into `tile` x `tile` tiles in row-major order. A `tile` of 0 yields nothing.
pub fn tiles(src: &Raster, tile: u32) -> TileIter<'_> {
    let (cols, rows) = if tile == 0 {
        (0, 0)
    } else {
        (src.width.div_ceil(tile), src.height.div_ceil(tile))
    };
    TileIter {
        src,
        tile,
        next: 0,
        cols,
        rows,
    }
}

impl Iterator for TileIter<'_> {
    type Item = Tile;

    fn next(&mut self) -> Option<Tile> {
        let total = u64::from(self.cols) * u64::from(self.rows);
        if self.next >= total {
            return None;
        }
        let (tx, ty) = (
            (self.next % u64::from(self.cols)) as u32,
            (self.next / u64::from(self.cols)) as u32,
        );
        self.next += 1;
        let (x, y) = (tx * self.tile, ty * self.tile);
        let (w, h) = (
            self.tile.min(self.src.width - x),
            self.tile.min(self.src.height - y),
        );
        let mut raster = Raster::new(w, h);
        let sw = self.src.width as usize;
        for r in 0..h as usize {
            let s = ((y as usize + r) * sw + x as usize) * 3;
            raster.data[r * w as usize * 3..(r + 1) * w as usize * 3]
                .copy_from_slice(&self.src.data[s..s + w as usize * 3]);
        }
        Some(Tile { x, y, raster })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = (u64::from(self.cols) * u64::from(self.rows)).saturating_sub(self.next) as usize;
        (left, Some(left))
    }
}

/// Puts tiles back together into a `width` x `height` raster (the inverse of [`tiles`]). Tiles
/// outside the frame are clipped; uncovered pixels stay black.
pub fn assemble(width: u32, height: u32, tiles: &[Tile]) -> Raster {
    let mut out = Raster::new(width, height);
    for t in tiles {
        if t.x >= width || t.y >= height {
            continue;
        }
        let w = t.raster.width.min(width - t.x) as usize;
        for r in 0..t.raster.height.min(height - t.y) as usize {
            let d = ((t.y as usize + r) * width as usize + t.x as usize) * 3;
            let s = r * t.raster.width as usize * 3;
            out.data[d..d + w * 3].copy_from_slice(&t.raster.data[s..s + w * 3]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn noise(w: u32, h: u32, seed: u32) -> Raster {
        let mut r = Raster::new(w, h);
        let mut s = seed | 1;
        for b in &mut r.data {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *b = (s >> 9) as u8;
        }
        r
    }

    #[test]
    fn level_sizes_follow_the_targets_for_12_24_48_and_100_mp() {
        for (w, h) in [
            (4000u32, 3000u32),
            (6000, 4000),
            (8000, 6000),
            (11_547, 8_660),
            (12_000, 1_200),
        ] {
            let t = level_size(w, h, Level::Thumb);
            let d = level_size(w, h, Level::Detect);
            let a = level_size(w, h, Level::Analysis);
            let p = level_size(w, h, Level::Display);
            assert_eq!(t.0.max(t.1), 256, "{w}x{h}");
            assert_eq!(d.0.max(d.1), 1024, "{w}x{h}");
            assert!(a.0.max(a.1) <= 3072 && p.0.max(p.1) <= 4096);
            let (am, pm) = (
                f64::from(a.0) * f64::from(a.1),
                f64::from(p.0) * f64::from(p.1),
            );
            if w <= 8000 {
                // Normal aspect: inside the targeted windows.
                assert!((1.2e6..=1.8e6).contains(&am), "analysis {am} for {w}x{h}");
                assert!((2.0e6..=4.0e6).contains(&pm), "display {pm} for {w}x{h}");
            }
            // Aspect ratio preserved to within rounding.
            let (ar, tr) = (f64::from(w) / f64::from(h), f64::from(a.0) / f64::from(a.1));
            assert!((ar / tr - 1.0).abs() < 0.01, "{w}x{h}");
        }
        // A 10:1 receipt strip hits the long-edge caps, not the pixel targets.
        let a = level_size(12_000, 1_200, Level::Analysis);
        assert_eq!(a.0, 3072);
    }

    #[test]
    fn small_sources_are_never_upscaled() {
        let p = Pyramid::build(noise(300, 200, 5));
        assert_eq!(p.level(Level::Display).width, 300);
        assert_eq!(p.level(Level::Analysis).width, 300);
        assert_eq!(p.level(Level::Detect).width, 300);
        assert_eq!(
            (p.level(Level::Thumb).width, p.level(Level::Thumb).height),
            (256, 171)
        );
        for l in Level::ALL {
            let r = p.level(l);
            assert!(r.width <= 300 && r.height <= 200);
            assert!(p.scale(l) <= 1.0);
        }
        // Levels equal to the source share it.
        assert!(Arc::ptr_eq(&p.levels[0], &p.levels[1]));
        assert_eq!(level_size(0, 0, Level::Thumb), (1, 1));
        assert_eq!(level_size(1, 1, Level::Display), (1, 1));
        let tiny = Pyramid::build(noise(1, 1, 3));
        assert_eq!(tiny.level(Level::Thumb).width, 1);
    }

    #[test]
    fn a_large_source_builds_the_expected_sizes() {
        let src = noise(3000, 2000, 11);
        let p = Pyramid::build(src.clone());
        assert_eq!(p.source_size(), (3000, 2000));
        assert_eq!(
            (
                p.level(Level::Display).width,
                p.level(Level::Display).height
            ),
            level_size(3000, 2000, Level::Display)
        );
        assert_eq!(p.level(Level::Thumb).width, 256);
        assert_eq!(p.level(Level::Detect).width, 1024);
        // Flat content stays flat through the cascade.
        let flat = Pyramid::build(Raster::filled(2500, 1800, [10, 130, 250]));
        for l in Level::ALL {
            assert!(
                flat.level(l).data.chunks(3).all(|p| p == [10, 130, 250]),
                "{l:?}"
            );
        }
    }

    #[test]
    fn same_bytes_at_1_and_8_threads() {
        let src = noise(2400, 1600, 21);
        let run = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| Pyramid::build(src.clone()))
        };
        let (a, b) = (run(1), run(8));
        for l in Level::ALL {
            assert_eq!(a.level(l), b.level(l), "{l:?}");
        }
    }

    #[test]
    fn tiles_rebuild_every_level_bit_exactly() {
        let p = Pyramid::build(noise(2100, 1500, 31));
        for l in Level::ALL {
            for tile in [64u32, 512, 1000, 4096] {
                let level = p.level(l);
                let ts: Vec<Tile> = p.tiles(l, tile).collect();
                let n =
                    u64::from(level.width.div_ceil(tile)) * u64::from(level.height.div_ceil(tile));
                assert_eq!(ts.len() as u64, n);
                assert!(
                    ts.iter()
                        .all(|t| t.raster.width <= tile && t.raster.height <= tile)
                );
                assert_eq!(
                    &assemble(level.width, level.height, &ts),
                    level,
                    "{l:?} tile {tile}"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn any_size_and_tile_rebuilds_exactly(w in 1u32..90, h in 1u32..90, tile in 1u32..40, seed in 1u32..1000) {
            let src = noise(w, h, seed);
            let ts: Vec<Tile> = tiles(&src, tile).collect();
            prop_assert_eq!(assemble(w, h, &ts), src);
        }
    }

    #[test]
    fn tile_edge_cases() {
        let r = noise(10, 10, 1);
        assert_eq!(tiles(&r, 0).count(), 0);
        assert_eq!(tiles(&Raster::new(0, 0), 8).count(), 0);
        let ts: Vec<_> = tiles(&r, 512).collect();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].raster, r);
    }
}
