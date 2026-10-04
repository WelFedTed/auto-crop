// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The analysis stand-ins (ROADMAP M1.55): equivalent work for the `analyse` stage of Table A,
//! labelled **STAND-IN** until M2 (classical analysis) and M4 (the corner net) exist.
//!
//! * [`Analyse::StandinNet`]: a random-weight 256x256 MobileNetV3-class net
//!   (`spikes/inference/gen_models.py`, `cargo xtask make-standin-net`) run through an
//!   `auto_crop_infer::InferenceBackend` (ONNX Runtime or rten, ADR-0007) on the 1024 px detection
//!   proxy, squashed to 256x256 and normalised exactly as a real corner net would need. The corner
//!   decode is an argmax per heatmap. **The weights are random, so the corners are noise: the
//!   pipeline rectifies the full frame when the decoded quad is not plausible** (it usually is
//!   not). Only the timing is meaningful.
//! * [`Analyse::StandinCanny`] (cargo feature `standin-canny`): Canny edges, contours, the convex
//!   hull of the largest one and its minimum-area rectangle, with `imageproc`, on the same proxy.
//!   This one does produce a quad, a crude page finder with no confidence and no refinement.
//! * [`Analyse::Classical`]: the existing line-based detector of `auto-crop-imgproc`.
//!
//! No claim about M2 or M4 is made by any of them.

use crate::error::ErrKind;
use auto_crop_core::Pt;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::scale::resize_area;
use auto_crop_infer::{InferError, InferenceBackend};
use std::sync::{Arc, Mutex};

/// Side of the net input: the stand-in has the 256x256 shape of DocQuadNet-256 (PLAN 7.1).
pub const NET_SIDE: usize = 256;

/// What the `analyse` stage runs.
#[derive(Clone, Default)]
pub enum Analyse {
    /// The existing classical detector (the default).
    #[default]
    Classical,
    /// STAND-IN: the random-weight corner net on an inference backend.
    StandinNet(Arc<StandinNet>),
    /// STAND-IN: Canny, contours and a minimum-area rectangle (needs the `standin-canny` feature).
    StandinCanny,
}

impl Analyse {
    /// The `--analyse` names.
    pub const NAMES: &'static str = "classical | standin-net | standin-canny";

    /// A short label for the timing table, `STAND-IN` first so nobody reads it as final.
    pub fn label(&self) -> String {
        match self {
            Analyse::Classical => "STAND-IN (classical detector)".to_owned(),
            Analyse::StandinNet(n) => format!(
                "STAND-IN (random-weight 256x256 net, {} {} thread{})",
                n.backend_name(),
                n.threads(),
                if n.threads() == 1 { "" } else { "s" }
            ),
            Analyse::StandinCanny => "STAND-IN (Canny + contours, imageproc)".to_owned(),
        }
    }
}

impl std::fmt::Debug for Analyse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// A loaded net on one backend. `run` needs `&mut` on the backend, so it sits behind a mutex; the
/// batch benchmark gives every worker its own.
pub struct StandinNet {
    backend: Mutex<Box<dyn InferenceBackend>>,
    name: &'static str,
    threads: usize,
}

impl StandinNet {
    pub fn new(backend: Box<dyn InferenceBackend>) -> Arc<Self> {
        Arc::new(Self {
            name: backend.name(),
            threads: backend.threads(),
            backend: Mutex::new(backend),
        })
    }

    pub fn backend_name(&self) -> &'static str {
        self.name
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Preprocess, run, decode. `Ok(None)` when the decoded corners are not a plausible page.
    pub fn analyse(&self, proxy: &Raster) -> Result<Option<[Pt; 4]>, ErrKind> {
        let input = net_input(proxy);
        let out = {
            let mut b = self.backend.lock().map_err(|_| ErrKind::InternalPanic)?;
            b.run(&input, [1, 3, NET_SIDE, NET_SIDE])
                .map_err(infer_err)?
        };
        Ok(decode_corners(&out.shape, &out.data))
    }
}

/// The backend a `standin-net` run uses when none is named: ONNX Runtime when it is compiled in
/// (ADR-0007: the first choice), else rten; `None` in a build with neither feature.
pub fn default_backend() -> Option<&'static str> {
    auto_crop_infer::compiled_backends().first().copied()
}

/// Loads the generated stand-in net (hash-checked against its sidecar digest) on the named
/// backend, `"ort"` or `"rten"`, with `threads` intra-op threads. The ONNX Runtime library is
/// located and verified by `auto_crop_infer::runtime` (`AUTOCROP_ORT_DYLIB`, else next to the
/// executable); a missing backend feature or runtime is an error, never a silent fallback.
pub fn load_net(
    model_path: &std::path::Path,
    backend: &str,
    threads: usize,
) -> Result<Arc<StandinNet>, String> {
    load_net_at(model_path, backend, threads, None)
}

/// [`load_net`] with an explicit absolute runtime path for `"ort"` (verified all the same).
pub fn load_net_at(
    model_path: &std::path::Path,
    backend: &str,
    threads: usize,
    runtime_path: Option<&std::path::Path>,
) -> Result<Arc<StandinNet>, String> {
    let model = auto_crop_infer::VerifiedModel::from_file_with_sidecar(model_path)
        .map_err(|e| format!("{e} (generate it with `cargo xtask make-standin-net`)"))?;
    auto_crop_infer::make_backend_at(backend, &model, threads, runtime_path)
        .map(StandinNet::new)
        .map_err(|e| e.to_string())
}

fn infer_err(e: InferError) -> ErrKind {
    match e {
        InferError::NotCompiled(_) => ErrKind::UnsupportedFeature,
        _ => ErrKind::ModelLoadFailed,
    }
}

/// Squashes the proxy to 256x256 (area average, so it is thread-count independent) and writes
/// NCHW `f32` with the ImageNet mean and standard deviation, the input a MobileNetV3 backbone
/// expects.
pub fn net_input(proxy: &Raster) -> Vec<f32> {
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let small = resize_area(proxy, NET_SIDE as u32, NET_SIDE as u32);
    let plane = NET_SIDE * NET_SIDE;
    let mut out = vec![0f32; 3 * plane];
    for (i, px) in small.data.as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = (f32::from(px[c]) / 255.0 - MEAN[c]) / STD[c];
        }
    }
    out
}

/// Decodes `[1, 4, H, W]` heatmaps into TL, TR, BR, BL: the argmax of each channel, as normalised
/// coordinates. `None` when the shape is not 4 channels or the quad is not a plausible page
/// (see [`plausible_quad`]).
pub fn decode_corners(shape: &[usize], data: &[f32]) -> Option<[Pt; 4]> {
    let &[1, 4, h, w] = shape else { return None };
    if h == 0 || w == 0 || data.len() != 4 * h * w {
        return None;
    }
    let mut q = [Pt::new(0.0, 0.0); 4];
    for (c, corner) in q.iter_mut().enumerate() {
        let plane = &data[c * h * w..(c + 1) * h * w];
        let (best, _) =
            plane
                .iter()
                .enumerate()
                .fold((0usize, f32::NEG_INFINITY), |(bi, bv), (i, &v)| {
                    if v > bv { (i, v) } else { (bi, bv) }
                });
        *corner = Pt::new(
            ((best % w) as f64 + 0.5) / w as f64,
            ((best / w) as f64 + 0.5) / h as f64,
        );
    }
    plausible_quad(q)
}

/// A quad is plausible when it is convex, in TL, TR, BR, BL order, inside the frame and covers at
/// least 10% of it.
pub fn plausible_quad(q: [Pt; 4]) -> Option<[Pt; 4]> {
    if q.iter().any(|p| {
        !(p.x.is_finite()
            && p.y.is_finite()
            && (0.0..=1.0).contains(&p.x)
            && (0.0..=1.0).contains(&p.y))
    }) {
        return None;
    }
    // Shoelace area and the sign of every turn.
    let mut area2 = 0.0;
    let mut sign = 0.0f64;
    for i in 0..4 {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        area2 += a.x * b.y - b.x * a.y;
        let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if cross == 0.0 || (sign != 0.0 && cross.signum() != sign) {
            return None;
        }
        sign = cross.signum();
    }
    // Clockwise on screen (y down) is a positive shoelace sum: TL, TR, BR, BL.
    (area2 / 2.0 >= 0.10).then_some(q)
}

/// Orders four points as TL, TR, BR, BL (image coordinates, y down) by the usual sum and
/// difference rule; exact for rectangles turned by less than 45 degrees.
pub fn order_quad(p: [(f64, f64); 4]) -> [(f64, f64); 4] {
    let by = |f: &dyn Fn(&(f64, f64)) -> f64, max: bool| {
        let it = p.iter().copied();
        let cmp = |a: &(f64, f64), b: &(f64, f64)| f(a).total_cmp(&f(b));
        if max { it.max_by(cmp) } else { it.min_by(cmp) }.unwrap_or((0.0, 0.0))
    };
    [
        by(&|q| q.0 + q.1, false),
        by(&|q| q.0 - q.1, true),
        by(&|q| q.0 + q.1, true),
        by(&|q| q.0 - q.1, false),
    ]
}

/// BT.601 luma of an RGB8 raster (same weights as the pipeline's `luma`).
#[cfg(any(feature = "standin-canny", test))]
fn grey(r: &Raster) -> Vec<u8> {
    r.data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|px| {
            let y = 77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]);
            ((y + 128) >> 8) as u8
        })
        .collect()
}

/// Canny (hysteresis 40 / 100 on the Sobel magnitude), outer contours, the largest contour by
/// bounding box, its convex hull and the minimum-area rectangle of that, as a normalised quad.
#[cfg(feature = "standin-canny")]
pub fn canny_quad(proxy: &Raster) -> Option<[Pt; 4]> {
    use imageproc::contours::find_contours;
    use imageproc::edges::canny;
    use imageproc::geometry::{convex_hull, min_area_rect};
    use imageproc::image::GrayImage;
    let (w, h) = (proxy.width, proxy.height);
    let img = GrayImage::from_raw(w, h, grey(proxy))?;
    let edges = canny(&img, 40.0, 100.0);
    let contours = find_contours::<i32>(&edges);
    let bbox_area = |pts: &[imageproc::point::Point<i32>]| {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        i64::from(x1 - x0) * i64::from(y1 - y0)
    };
    let best = contours
        .iter()
        .filter(|c| c.points.len() >= 8)
        .max_by_key(|c| bbox_area(&c.points))?;
    let hull = convex_hull(best.points.clone());
    if hull.len() < 3 {
        return None;
    }
    let rect = min_area_rect(&hull);
    let (fw, fh) = (f64::from(w), f64::from(h));
    let ordered = order_quad(rect.map(|p| (f64::from(p.x), f64::from(p.y))));
    plausible_quad(
        ordered.map(|(x, y)| Pt::new((x / fw).clamp(0.0, 1.0), (y / fh).clamp(0.0, 1.0))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize * 3;
                r.data[i..i + 3].copy_from_slice(&f(x, y));
            }
        }
        r
    }

    #[test]
    fn the_net_input_is_nchw_normalised() {
        let r = raster(512, 384, |_, _| [255, 0, 128]);
        let v = net_input(&r);
        assert_eq!(v.len(), 3 * NET_SIDE * NET_SIDE);
        let plane = NET_SIDE * NET_SIDE;
        assert!((v[0] - (1.0 - 0.485) / 0.229).abs() < 1e-5);
        assert!((v[plane] - (0.0 - 0.456) / 0.224).abs() < 1e-5);
        assert!((v[2 * plane] - (128.0 / 255.0 - 0.406) / 0.225).abs() < 1e-4);
    }

    fn heat(peaks: [(usize, usize); 4]) -> Vec<f32> {
        let mut d = vec![0.0f32; 4 * 64 * 64];
        for (c, (x, y)) in peaks.iter().enumerate() {
            d[c * 4096 + y * 64 + x] = 1.0;
        }
        d
    }

    #[test]
    fn corners_are_decoded_by_argmax_and_a_plausible_page_is_kept() {
        let q =
            decode_corners(&[1, 4, 64, 64], &heat([(8, 8), (56, 8), (56, 56), (8, 56)])).unwrap();
        assert!((q[0].x - 8.5 / 64.0).abs() < 1e-12 && (q[2].y - 56.5 / 64.0).abs() < 1e-12);
    }

    #[test]
    fn implausible_decodes_become_none_so_the_pipeline_uses_the_full_frame() {
        // Collapsed (all channels peak at the same pixel), tiny, bow-tie and wrong shape.
        assert!(decode_corners(&[1, 4, 64, 64], &heat([(5, 5); 4])).is_none());
        assert!(
            decode_corners(&[1, 4, 64, 64], &heat([(8, 8), (20, 8), (20, 20), (8, 20)])).is_none()
        );
        assert!(
            decode_corners(&[1, 4, 64, 64], &heat([(8, 8), (56, 56), (56, 8), (8, 56)])).is_none()
        );
        assert!(decode_corners(&[1, 3, 64, 64], &vec![0.0; 3 * 4096]).is_none());
        assert!(decode_corners(&[1, 4, 64, 64], &[0.0; 10]).is_none());
        // All-zero heatmaps: every argmax is pixel 0, a collapsed quad.
        assert!(decode_corners(&[1, 4, 64, 64], &vec![0.0; 4 * 4096]).is_none());
    }

    #[test]
    fn quads_are_ordered_tl_tr_br_bl() {
        let o = order_quad([(90.0, 80.0), (10.0, 10.0), (90.0, 10.0), (10.0, 80.0)]);
        assert_eq!(o, [(10.0, 10.0), (90.0, 10.0), (90.0, 80.0), (10.0, 80.0)]);
    }

    #[cfg(feature = "standin-canny")]
    #[test]
    fn canny_finds_a_bright_page_on_a_dark_desk() {
        // 400x300 frame, page at x 80..320, y 50..250 (60% x 67% of the frame area is 0.4).
        let r = raster(400, 300, |x, y| {
            if (80..320).contains(&x) && (50..250).contains(&y) {
                [235, 235, 230]
            } else {
                [60, 45, 30]
            }
        });
        let q = canny_quad(&r).expect("a page");
        let want = [
            (80.0 / 400.0, 50.0 / 300.0),
            (320.0 / 400.0, 50.0 / 300.0),
            (320.0 / 400.0, 250.0 / 300.0),
            (80.0 / 400.0, 250.0 / 300.0),
        ];
        for (p, (wx, wy)) in q.iter().zip(want) {
            assert!(
                (p.x - wx).abs() < 0.03 && (p.y - wy).abs() < 0.03,
                "{p:?} vs {wx},{wy}"
            );
        }
    }

    #[cfg(feature = "standin-canny")]
    #[test]
    fn canny_finds_nothing_on_a_flat_frame() {
        let r = raster(200, 150, |_, _| [128, 128, 128]);
        assert!(canny_quad(&r).is_none());
    }

    #[test]
    fn grey_uses_the_pipeline_weights() {
        let r = raster(1, 1, |_, _| [255, 255, 255]);
        assert_eq!(grey(&r), vec![255]);
    }
}
