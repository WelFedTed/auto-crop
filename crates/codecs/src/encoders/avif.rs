// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! AVIF writer (ROADMAP M11.22, M11.23; PLAN 3.3 "AVIF"): rav1e for the AV1 picture, our own
//! ISOBMFF/HEIF container around it.
//!
//! **Why not `ravif`.** `ravif` always writes 4:4:4, has no way to write an ICC profile (its
//! container crate, `avif-serialize`, writes `colr` as `nclx` only) and exposes no seam to add one,
//! so ICC byte-exactness (PLAN 3.6) needs our own container anyway. The container of one still
//! picture is small (this file: about 300 lines with the reader), so rav1e is driven directly and
//! `ravif` and `avif-serialize` are not dependencies. rav1e is built **without its `asm` feature**:
//! that needs `nasm`, which only CI has. The build is pure Rust and slower; the measured cost is in
//! `docs/dev/output-formats.md`.
//!
//! **What is written** (a MIAF still picture, major brand `avif`):
//!
//! * 8-bit AV1, 4:2:0 or 4:4:4 (a grey raster is written 4:0:0, one plane), full range, BT.601
//!   matrix (nclx matrix 6, the most widely decoded choice).
//! * `colr`: without a profile `nclx` (sRGB primaries 1, transfer 13); with one a `prof` box with the
//!   profile **byte-exact** and an `nclx` that claims only the matrix (primaries and transfer
//!   unspecified), as libavif does.
//! * EXIF as an `Exif` item with the 4-byte TIFF-header offset in front (PLAN 3.6 "TIFF-offset
//!   prefix"), linked to the picture by a `cdsc` reference. The caller passes a bare TIFF blob with
//!   Orientation already 1 and GPS per policy ([`AvifEncoder::exif`]).
//! * `ispe`, `pixi`, `av1C`; **no `irot`, no `imir`** (the pixels were turned once). No alpha (the
//!   engine's rasters have none). No pixel density (AVIF has none outside EXIF).
//!
//! **Quality.** The `OutputSpec` quality 1 to 100 maps to rav1e's base quantiser index (0 to 255) by
//! [`quality_to_quantizer`], the piecewise curve of the `cavif` tool. rav1e has no psychovisual
//! rate control in this mode; the quantiser is fixed for the whole picture.
//!
//! **Cancellation.** rav1e cannot stop in the middle of a frame, so the encode runs on its own
//! thread and the caller waits in 20 ms steps: a cancel returns `Cancelled` at once and the thread
//! finishes its frame on its own (one orphan per cancelled encode; its result is dropped).

use super::avif_box::{self as container, exif_item_payload};
use super::{check, samples_u8};
use auto_crop_core::output::{Bits, OutFormat, Quality};
use auto_crop_core::ports::{EncodeMeta, Encoder, RasterView};
use auto_crop_core::{CancelToken, ErrKind, OutputSpec};

/// The quality used when the spec does not pin one: "Balanced" of the plan (PROVISIONAL).
pub const DEFAULT_AVIF_QUALITY: u8 = 75;
/// The speed (rav1e preset 0 slowest to 10 fastest) used when none is chosen (measured, see the
/// notes in `docs/dev/output-formats.md`).
pub const DEFAULT_AVIF_SPEED: u8 = 8;
/// The largest side we write (AV1 allows 65,536; this keeps `u16` arithmetic honest).
pub const AVIF_MAX_SIDE: u32 = 65_535;

/// Chroma layout of the colour planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AvifChroma {
    /// 4:2:0: half resolution chroma, about 30% smaller files; colour fringes on 1 px text edges.
    #[default]
    Yuv420,
    /// 4:4:4: full resolution chroma; the choice for text, receipts and line art.
    Yuv444,
}

/// rav1e's base quantiser index (0 best to 255 worst) for a quality of 1 to 100: the `cavif`
/// curve, `(1 - q) * 2.6` from q 0.82 up, `0.875 - q / 2` between 0.25 and 0.82, `1 - q` below,
/// times 255.
pub fn quality_to_quantizer(quality: u8) -> u8 {
    let q = f64::from(quality.clamp(1, 100)) / 100.0;
    let x = if q >= 0.82 {
        (1.0 - q) * 2.6
    } else if q > 0.25 {
        q.mul_add(-0.5, 0.875)
    } else {
        1.0 - q
    };
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// AVIF through rav1e (8-bit, 4:0:0, 4:2:0 or 4:4:4).
#[derive(Debug, Clone)]
pub struct AvifEncoder {
    pub chroma: AvifChroma,
    /// rav1e speed preset 0 to 10 (higher is faster and larger).
    pub speed: u8,
    /// Worker threads of the encoder; `None` = one per core.
    pub threads: Option<usize>,
    /// A bare TIFF-structured EXIF blob, already patched (Orientation 1, GPS per policy). Written
    /// with the 4-byte offset prefix; anything that is not a TIFF header is not written.
    pub exif: Option<Vec<u8>>,
    /// The quality used when the spec says `MatchSource`.
    pub default_quality: u8,
}

impl Default for AvifEncoder {
    fn default() -> Self {
        Self {
            chroma: AvifChroma::default(),
            speed: DEFAULT_AVIF_SPEED,
            threads: None,
            exif: None,
            default_quality: DEFAULT_AVIF_QUALITY,
        }
    }
}

impl AvifEncoder {
    fn quality(&self, spec: &OutputSpec) -> Option<u8> {
        if !matches!(spec.bits, Bits::Eight) {
            return None;
        }
        match spec.quality {
            Quality::Fixed { value } => Some(value.clamp(1, 100)),
            Quality::MatchSource { floor, cap } => Some(
                self.default_quality
                    .clamp(floor.min(cap), cap.max(floor))
                    .clamp(1, 100),
            ),
            Quality::Lossless => None,
        }
    }
}

/// The planes of one picture, 8-bit, full range, BT.601.
struct Planes {
    width: usize,
    height: usize,
    y: Vec<u8>,
    /// `None` for a monochrome picture.
    chroma: Option<(Vec<u8>, Vec<u8>)>,
    /// Chroma plane width and height.
    cw: usize,
    subsampled: bool,
}

/// RGB to BT.601 full-range YCbCr in 16.16 fixed point (the rows of the matrix sum to 65,536 for Y
/// and to 0 for Cb and Cr, so grey stays exactly grey: Cb = Cr = 128).
#[inline]
fn ycbcr(r: i32, g: i32, b: i32) -> (u8, u8, u8) {
    let y = (19595 * r + 38470 * g + 7471 * b + 32768) >> 16;
    let cb = 128 + ((-11059 * r - 21709 * g + 32768 * b + 32768) >> 16);
    let cr = 128 + ((32768 * r - 27439 * g - 5329 * b + 32768) >> 16);
    (
        y.clamp(0, 255) as u8,
        cb.clamp(0, 255) as u8,
        cr.clamp(0, 255) as u8,
    )
}

fn to_planes(
    samples: &[u8],
    width: usize,
    height: usize,
    channels: usize,
    chroma: AvifChroma,
) -> Planes {
    if channels == 1 {
        return Planes {
            width,
            height,
            y: samples[..width * height].to_vec(),
            chroma: None,
            cw: 0,
            subsampled: false,
        };
    }
    let px = |x: usize, y: usize| {
        let o = (y * width + x) * 3;
        (
            i32::from(samples[o]),
            i32::from(samples[o + 1]),
            i32::from(samples[o + 2]),
        )
    };
    let mut yp = Vec::with_capacity(width * height);
    for row in samples.chunks_exact(width * 3).take(height) {
        for p in row.as_chunks::<3>().0 {
            yp.push(ycbcr(i32::from(p[0]), i32::from(p[1]), i32::from(p[2])).0);
        }
    }
    match chroma {
        AvifChroma::Yuv444 => {
            let mut cb = Vec::with_capacity(width * height);
            let mut cr = Vec::with_capacity(width * height);
            for p in samples.as_chunks::<3>().0.iter().take(width * height) {
                let (_, b, r) = ycbcr(i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
                cb.push(b);
                cr.push(r);
            }
            Planes {
                width,
                height,
                y: yp,
                chroma: Some((cb, cr)),
                cw: width,
                subsampled: false,
            }
        }
        AvifChroma::Yuv420 => {
            let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
            let mut cb = Vec::with_capacity(cw * ch);
            let mut cr = Vec::with_capacity(cw * ch);
            for j in 0..ch {
                let (y0, y1) = (j * 2, (j * 2 + 1).min(height - 1));
                for i in 0..cw {
                    let (x0, x1) = (i * 2, (i * 2 + 1).min(width - 1));
                    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                    let avg =
                        |f: fn(&(i32, i32, i32)) -> i32| (f(&a) + f(&b) + f(&c) + f(&d) + 2) >> 2;
                    let (_, u, v) = ycbcr(avg(|t| t.0), avg(|t| t.1), avg(|t| t.2));
                    cb.push(u);
                    cr.push(v);
                }
            }
            Planes {
                width,
                height,
                y: yp,
                chroma: Some((cb, cr)),
                cw,
                subsampled: true,
            }
        }
    }
}

struct Av1 {
    /// The `av1C` record (4 bytes plus the sequence header OBU).
    av1c: Vec<u8>,
    /// The temporal unit: the item's data.
    data: Vec<u8>,
}

fn run_rav1e(
    planes: &Planes,
    quantizer: u8,
    speed: u8,
    threads: Option<usize>,
    nclx_unspecified: bool,
) -> Result<Av1, ErrKind> {
    use rav1e::prelude::*;
    let (w, h) = (planes.width, planes.height);
    let mut enc = EncoderConfig::with_speed_preset(speed.min(10));
    enc.width = w;
    enc.height = h;
    enc.bit_depth = 8;
    enc.chroma_sampling = match (&planes.chroma, planes.subsampled) {
        (None, _) => ChromaSampling::Cs400,
        (Some(_), true) => ChromaSampling::Cs420,
        (Some(_), false) => ChromaSampling::Cs444,
    };
    enc.pixel_range = PixelRange::Full;
    enc.color_description = Some(ColorDescription {
        color_primaries: if nclx_unspecified {
            ColorPrimaries::Unspecified
        } else {
            ColorPrimaries::BT709
        },
        transfer_characteristics: if nclx_unspecified {
            TransferCharacteristics::Unspecified
        } else {
            TransferCharacteristics::SRGB
        },
        matrix_coefficients: MatrixCoefficients::BT601,
    });
    enc.still_picture = true;
    enc.quantizer = usize::from(quantizer);
    enc.min_quantizer = quantizer;
    enc.tune = Tune::Psychovisual;
    enc.speed_settings.multiref = false;
    enc.speed_settings.rdo_lookahead_frames = 1;
    enc.speed_settings.scene_detection_mode = SceneDetectionSpeed::None;
    enc.min_key_frame_interval = 0;
    enc.max_key_frame_interval = 0;
    let n_threads =
        threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from));
    // Tiles are the only parallelism inside one frame; they cost a little compression, so never
    // smaller than 256 x 256 px.
    enc.tiles = n_threads.min((w * h) / (256 * 256)).max(1);
    let cfg = Config::new()
        .with_encoder_config(enc)
        .with_threads(n_threads);
    let mut ctx: Context<u8> = cfg.new_context().map_err(|_| ErrKind::EncodeFailed)?;
    let mut frame = ctx.new_frame();
    frame.planes[0].copy_from_raw_u8(&planes.y, w, 1);
    if let Some((cb, cr)) = &planes.chroma {
        frame.planes[1].copy_from_raw_u8(cb, planes.cw, 1);
        frame.planes[2].copy_from_raw_u8(cr, planes.cw, 1);
    }
    ctx.send_frame(frame).map_err(|_| ErrKind::EncodeFailed)?;
    ctx.flush();
    let av1c = ctx.container_sequence_header();
    let mut data = Vec::new();
    loop {
        match ctx.receive_packet() {
            Ok(mut p) => {
                if matches!(p.frame_type, FrameType::KEY) {
                    data.append(&mut p.data);
                }
            }
            Err(EncoderStatus::Encoded) => {}
            Err(EncoderStatus::LimitReached) => break,
            Err(_) => return Err(ErrKind::EncodeFailed),
        }
    }
    if data.is_empty() || av1c.len() < 4 {
        return Err(ErrKind::EncodeFailed);
    }
    Ok(Av1 { av1c, data })
}

impl Encoder for AvifEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Avif
    }

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        check(cancel)?;
        let quality = self.quality(spec).ok_or(ErrKind::UnsupportedOutput)?;
        let (w, h) = (src.width(), src.height());
        if w == 0 || h == 0 || w > AVIF_MAX_SIDE || h > AVIF_MAX_SIDE {
            return Err(ErrKind::UnsupportedOutput);
        }
        let channels = src.format().channels();
        let planes = to_planes(
            &samples_u8(src),
            w as usize,
            h as usize,
            channels,
            self.chroma,
        );
        check(cancel)?;
        let icc = meta.icc.as_deref().filter(|p| !p.is_empty());
        let (quantizer, speed, threads) = (quality_to_quantizer(quality), self.speed, self.threads);
        let unspecified = icc.is_some();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("avif-encode".into())
            .spawn(move || {
                // rav1e panics on inputs it cannot handle; that is an encode error, never an abort.
                let r = std::panic::catch_unwind(|| {
                    run_rav1e(&planes, quantizer, speed, threads, unspecified)
                })
                .unwrap_or(Err(ErrKind::EncodeFailed));
                let _ = tx.send(r);
            })
            .map_err(|_| ErrKind::EncodeFailed)?;
        let av1 = loop {
            match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(r) => break r?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => check(cancel)?,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ErrKind::EncodeFailed);
                }
            }
        };
        check(cancel)?;
        let exif = self.exif.as_deref().and_then(exif_item_payload);
        Ok(container::write(&container::Picture {
            width: w,
            height: h,
            monochrome: channels == 1,
            subsampled: planes_subsampled(channels, self.chroma),
            av1c: &av1.av1c,
            av1_data: &av1.data,
            icc,
            nclx_unspecified: unspecified,
            exif: exif.as_deref(),
        }))
    }
}

fn planes_subsampled(channels: usize, chroma: AvifChroma) -> bool {
    channels != 1 && chroma == AvifChroma::Yuv420
}
