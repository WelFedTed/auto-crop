// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Image side of the labeller: decode through the repository's own codecs (so the oriented
//! pixels and size are exactly what the evaluation harness sees), keep the last couple of decoded
//! images in memory, and serve downscaled previews and tiles. The browser never receives the file
//! itself, only re-encoded previews.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::scale::{fit_dimensions, resize_area, resize_to_fit};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Long edge of the default preview.
pub const PREVIEW_EDGE: u32 = 2400;
/// Largest edge of a tile.
pub const TILE_EDGE_MAX: u32 = 1800;
const CACHE_SIZE: usize = 2;

pub struct Loaded {
    pub raster: Raster,
    pub sha256: String,
    pub format: String,
    preview: Mutex<Option<Arc<Vec<u8>>>>,
}

impl Loaded {
    /// Encoded default preview (cached) and its pixel width.
    pub fn preview(&self) -> Result<(Arc<Vec<u8>>, u32), String> {
        let w = fit_dimensions(self.raster.width, self.raster.height, PREVIEW_EDGE).0;
        let mut slot = self.preview.lock().map_err(|_| "preview lock poisoned")?;
        if let Some(p) = slot.as_ref() {
            return Ok((Arc::clone(p), w));
        }
        let (bytes, _) = encode_preview(&self.raster, PREVIEW_EDGE)?;
        let arc = Arc::new(bytes);
        *slot = Some(Arc::clone(&arc));
        Ok((arc, w))
    }

    /// The preview's width in pixels (without encoding it).
    pub fn preview_width(&self) -> u32 {
        fit_dimensions(self.raster.width, self.raster.height, PREVIEW_EDGE).0
    }
}

/// Fits `r` into `edge` and encodes it (PNG when small, JPEG otherwise). Returns the bytes and
/// the content type.
pub fn encode_preview(r: &Raster, edge: u32) -> Result<(Vec<u8>, &'static str), String> {
    let small = if r.width.max(r.height) > edge {
        resize_to_fit(r, edge)
    } else {
        r.clone()
    };
    let png = u64::from(small.width) * u64::from(small.height) <= 1_500_000;
    let (fmt, ct) = if png {
        (Format::Png, "image/png")
    } else {
        (Format::Jpeg, "image/jpeg")
    };
    encode(&small, fmt, 90, None)
        .map(|b| (b, ct))
        .map_err(|e| format!("cannot encode the preview: {e}"))
}

/// The region `x, y, w, h` (fractions of the image) scaled to at most `out` pixels on its long
/// edge, never upscaled. Returns `None` for an empty or out-of-range region.
pub fn tile(r: &Raster, x: f64, y: f64, w: f64, h: f64, out: u32) -> Option<Raster> {
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (iw, ih) = (f64::from(r.width), f64::from(r.height));
    let x0 = (x.clamp(0.0, 1.0) * iw).floor();
    let y0 = (y.clamp(0.0, 1.0) * ih).floor();
    let x1 = ((x + w).clamp(0.0, 1.0) * iw).ceil();
    let y1 = ((y + h).clamp(0.0, 1.0) * ih).ceil();
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    let (cx, cy, cw, ch) = (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
    let mut crop = Raster::new(cw, ch);
    for row in 0..ch {
        let src = (((cy + row) * r.width + cx) * 3) as usize;
        let dst = (row * cw * 3) as usize;
        let n = (cw * 3) as usize;
        crop.data[dst..dst + n].copy_from_slice(&r.data[src..src + n]);
    }
    let out = out.clamp(64, TILE_EDGE_MAX);
    if cw.max(ch) <= out {
        return Some(crop);
    }
    let (tw, th) = fit_dimensions(cw, ch, out);
    Some(resize_area(&crop, tw, th))
}

#[derive(Default)]
pub struct Cache {
    slots: Mutex<Vec<(usize, Arc<Loaded>)>>,
}

impl Cache {
    /// The decoded image number `idx` (decoding it, under the cache lock, if it is not cached).
    pub fn get(&self, idx: usize, path: &Path) -> Result<Arc<Loaded>, String> {
        let mut slots = self.slots.lock().map_err(|_| "cache lock poisoned")?;
        if let Some((_, l)) = slots.iter().find(|(i, _)| *i == idx) {
            return Ok(Arc::clone(l));
        }
        let loaded = Arc::new(load(path)?);
        slots.insert(0, (idx, Arc::clone(&loaded)));
        slots.truncate(CACHE_SIZE);
        Ok(loaded)
    }
}

fn load(path: &Path) -> Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read the image: {e}"))?;
    let sha256 = auto_crop_eval::manifest::sha256_hex(&bytes);
    let d = decode(&bytes).map_err(|e| format!("this build cannot decode it: {e}"))?;
    Ok(Loaded {
        format: format!("{:?}", d.format).to_lowercase(),
        raster: d.raster,
        sha256,
        preview: Mutex::new(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                r.data[i] = (x % 256) as u8;
                r.data[i + 1] = (y % 256) as u8;
                r.data[i + 2] = 7;
            }
        }
        r
    }

    #[test]
    fn tiles_crop_the_right_region_and_never_upscale() {
        let r = gradient(400, 300);
        let t = tile(&r, 0.25, 0.5, 0.5, 0.25, 1800).expect("tile");
        assert_eq!((t.width, t.height), (200, 75));
        assert_eq!(t.data[0], 100); // x = 100
        assert_eq!(t.data[1], 150); // y = 150
        let small = tile(&r, 0.0, 0.0, 1.0, 1.0, 100).expect("tile");
        assert_eq!(small.width.max(small.height), 100);
        for bad in [
            (0.0, 0.0, 0.0, 1.0),
            (2.0, 0.0, 1.0, 1.0),
            (f64::NAN, 0.0, 1.0, 1.0),
        ] {
            assert!(
                tile(&r, bad.0, bad.1, bad.2, bad.3, 500).is_none(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn previews_are_png_when_small_and_jpeg_when_big() {
        let (_, ct) = encode_preview(&gradient(400, 300), 2400).expect("encodes");
        assert_eq!(ct, "image/png");
        let (b, ct) = encode_preview(&gradient(3000, 2000), 2400).expect("encodes");
        assert_eq!(ct, "image/jpeg");
        assert_eq!(&b[..2], &[0xff, 0xd8]);
    }

    #[test]
    fn the_cache_decodes_once_applies_orientation_and_keeps_two() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut paths = Vec::new();
        for i in 0..3u32 {
            let png = encode(&gradient(40 + i, 30), Format::Png, 90, None).expect("encodes");
            let p = dir.path().join(format!("{i}.png"));
            std::fs::write(&p, png).expect("write");
            paths.push(p);
        }
        let c = Cache::default();
        let a = c.get(0, &paths[0]).expect("decodes");
        assert_eq!((a.raster.width, a.raster.height), (40, 30));
        assert_eq!(a.sha256.len(), 64);
        assert_eq!(a.format, "png");
        let again = c.get(0, &paths[0]).expect("cached");
        assert!(Arc::ptr_eq(&a, &again));
        c.get(1, &paths[1]).expect("decodes");
        c.get(2, &paths[2]).expect("decodes");
        let third = c.get(0, &paths[0]).expect("decodes again after eviction");
        assert!(!Arc::ptr_eq(&a, &third));
        assert!(c.get(5, &dir.path().join("missing.png")).is_err());
        // EXIF orientation 6 (rotate 90 clockwise) is applied: labels live in the oriented space.
        let rotated = auto_crop_codecs::fixtures::jpeg_with_exif_blob(
            &auto_crop_codecs::fixtures::jpeg_baseline(60, 40),
            &auto_crop_codecs::fixtures::exif_blob(6, true),
        );
        let rp = dir.path().join("rotated.jpg");
        std::fs::write(&rp, rotated).expect("write");
        let r = c.get(7, &rp).expect("decodes");
        assert_eq!((r.raster.width, r.raster.height), (40, 60));
        let junk = dir.path().join("junk.png");
        std::fs::write(&junk, b"not an image").expect("write");
        assert!(
            c.get(6, &junk)
                .err()
                .expect("junk fails")
                .contains("cannot decode")
        );
    }
}
