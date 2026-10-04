// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The pipeline skeleton (ROADMAP M1.54, PLAN 7.1 Table A): one image through
//! `read_probe`, `decode`, `proxy`, `analyse`, `rectify`, `enhance` and `encode`, each stage a
//! `tracing` span named exactly like its Table A line and each reporting a `stage_done` event with
//! `stage`, `ms` and `px` fields. It is the measurement target of `auto-crop dev-pipeline`, the
//! stage-sum check and the batch benchmark (M1.59, M1.60, M1.63); the real engine (M2) replaces the
//! stand-ins stage by stage and keeps the names.
//!
//! Stand-ins, honestly labelled:
//!
//! * `analyse` runs the existing **classical detector** on the 1024 px proxy. The M2/M4 corner net,
//!   orientation and fusion are not here, so this row is a STAND-IN (ROADMAP M1.55).
//! * `enhance` is the **prototype** "grey + threshold": luma, then Otsu or Sauvola from
//!   `auto-crop-imgproc`, written back as black and white. It is neither the Auto/Grayscale
//!   illumination flatten (M7) nor a quality claim, only a kernel of the right size and shape.
//! * `encode` is the `image` crate's JPEG encoder at a fixed quality (the turbojpeg encoder of
//!   M1.21/M1.57 is not in the tree).
//! * Not chained: Table A's `refine` (stage 5) and `commit` (stage 9, the safe-write path of B3). So
//!   the sum of the seven stages is compared with 515 ms of the 700 ms total, not with 700 ms.
//!
//! What `px` means per stage: `read_probe` the probed size, `decode` the decoded size, `proxy` the
//! source pixels read, `analyse` the detection proxy size, `rectify`, `enhance` and `encode` the
//! output size. It is the work size that a ms/px rate divides by, not "pixels written".
//!
//! Determinism: every kernel here is byte-identical at any thread count (tests below run 1 and 8).
//! Cancellation is checked before every stage and every 64 rows inside the warp; a stopped run
//! returns a [`Failure`] naming the stage and the stages already done, never partial bytes.

pub mod bench_images;
pub mod standin;

pub use standin::{Analyse, StandinNet};

use crate::error::{ErrKind, codec_err};
use auto_crop_codecs::{DecodeLimits, Format, MAX_PIXELS, decode_with, encode, probe_with, sniff};
use auto_crop_core::{CancelToken, Pt, QuadWarp};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::pyramid::{Level, Pyramid};
use auto_crop_imgproc::render::{Limits, RenderError, render_quad_cancellable};
use auto_crop_imgproc::threshold::{binarize, histogram, otsu_from_histogram, sauvola};
use rayon::prelude::*;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Quality of the benchmark JPEG (Table A: "JPEG at a fixed q90").
pub const BENCH_JPEG_QUALITY: u8 = 90;

/// A stage of the skeleton, in execution order. The serialised and span names are Table A's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    ReadProbe,
    Decode,
    Proxy,
    Analyse,
    Rectify,
    Enhance,
    Encode,
}

impl Stage {
    pub const ALL: [Stage; 7] = [
        Stage::ReadProbe,
        Stage::Decode,
        Stage::Proxy,
        Stage::Analyse,
        Stage::Rectify,
        Stage::Enhance,
        Stage::Encode,
    ];

    /// The span name, equal to the Table A line.
    pub const fn name(self) -> &'static str {
        match self {
            Stage::ReadProbe => "read_probe",
            Stage::Decode => "decode",
            Stage::Proxy => "proxy",
            Stage::Analyse => "analyse",
            Stage::Rectify => "rectify",
            Stage::Enhance => "enhance",
            Stage::Encode => "encode",
        }
    }

    /// The PROVISIONAL Table A budget in ms (12 MP JPEG, Tier-M, p50). `analyse` is also the p95
    /// ceiling over T1 exits.
    pub const fn budget_ms(self) -> f64 {
        match self {
            Stage::ReadProbe => 10.0,
            Stage::Decode => 120.0,
            Stage::Proxy => 25.0,
            Stage::Analyse => 40.0,
            Stage::Rectify => 90.0,
            Stage::Enhance => 120.0,
            Stage::Encode => 110.0,
        }
    }

    fn span(self) -> tracing::Span {
        // Span names must be literals, so each stage names its own.
        match self {
            Stage::ReadProbe => tracing::info_span!("read_probe"),
            Stage::Decode => tracing::info_span!("decode"),
            Stage::Proxy => tracing::info_span!("proxy"),
            Stage::Analyse => tracing::info_span!("analyse"),
            Stage::Rectify => tracing::info_span!("rectify"),
            Stage::Enhance => tracing::info_span!("enhance"),
            Stage::Encode => tracing::info_span!("encode"),
        }
    }
}

/// Sum of the Table A budgets of the seven chained stages (the table's 700 ms also holds `refine`,
/// `commit` and 75 ms of slack, which this skeleton does not run).
pub fn chained_budget_ms() -> f64 {
    Stage::ALL.iter().map(|s| s.budget_ms()).sum()
}

/// One finished stage: the `stage_done{stage, ms, px}` event as data.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct StageTiming {
    pub stage: Stage,
    pub ms: f64,
    pub px: u64,
}

/// What the `enhance` stand-in does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Enhance {
    /// Skip the stage (the crop-only variant of Table A, 580 ms).
    Off,
    /// Luma then global Otsu (the prototype).
    #[default]
    Otsu,
    /// Luma then Sauvola, window 51, k = 0.2 (the B&W/Faded print path of Table A).
    Sauvola,
}

#[derive(Clone)]
pub struct Options {
    /// JPEG quality of the output.
    pub quality: u8,
    pub enhance: Enhance,
    /// What the `analyse` stage runs (M1.55): the classical detector or one of the STAND-INs.
    pub analyse: Analyse,
    pub limits: DecodeLimits,
    /// Run every parallel kernel in this pool (`None`: rayon's global pool). One single-thread
    /// pool per batch worker gives the "one image per worker" mode of PLAN 7.3.
    pub pool: Option<Arc<rayon::ThreadPool>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quality: BENCH_JPEG_QUALITY,
            enhance: Enhance::default(),
            analyse: Analyse::default(),
            limits: DecodeLimits::default(),
            pool: None,
        }
    }
}

/// A rayon pool with exactly `threads` workers (at least 1).
pub fn thread_pool(threads: usize) -> Result<Arc<rayon::ThreadPool>, ErrKind> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .thread_name(|i| format!("skeleton-{i}"))
        .build()
        .map(Arc::new)
        .map_err(|_| ErrKind::Internal)
}

/// Where the bytes come from. A path makes `read_probe` include the file read.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    Path(&'a Path),
    Bytes(&'a [u8]),
}

/// A finished run's measurements.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub stages: Vec<StageTiming>,
    /// Wall time of the whole run, including the glue between stages.
    pub total_ms: f64,
    pub source: (u32, u32),
    pub output: (u32, u32),
    pub output_bytes: usize,
    /// False when the detector found nothing and the full frame was rectified instead.
    pub quad_found: bool,
    /// What the `analyse` stage ran, as a label that starts with `STAND-IN` (M1.55).
    pub analyse: String,
}

impl Report {
    /// Sum of the stage times (never more than `total_ms`).
    pub fn stage_sum_ms(&self) -> f64 {
        self.stages.iter().map(|s| s.ms).sum()
    }

    pub fn ms_of(&self, stage: Stage) -> Option<f64> {
        self.stages.iter().find(|s| s.stage == stage).map(|s| s.ms)
    }
}

/// The encoded JPEG and what it cost.
#[derive(Debug, Clone)]
pub struct Output {
    pub bytes: Vec<u8>,
    pub report: Report,
}

/// A run that stopped: the error code, the stage it stopped in, and the stages that had finished.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub kind: ErrKind,
    pub stage: Stage,
    pub done: Vec<StageTiming>,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.stage.name(), self.kind)
    }
}

impl std::error::Error for Failure {}

/// Runs the skeleton. See [`run_observed`].
pub fn run(input: Input<'_>, opts: &Options, cancel: &CancelToken) -> Result<Output, Failure> {
    run_observed(input, opts, cancel, &mut |_| {})
}

/// Runs the skeleton, calling `observer` after every finished stage (the same data as the
/// `stage_done` event; tests cancel from inside it to stop a run at an exact boundary).
pub fn run_observed(
    input: Input<'_>,
    opts: &Options,
    cancel: &CancelToken,
    observer: &mut (dyn FnMut(&StageTiming) + Send),
) -> Result<Output, Failure> {
    match &opts.pool {
        Some(pool) => {
            // The pool's threads do not inherit this thread's tracing dispatcher.
            let dispatch = tracing::dispatcher::get_default(Clone::clone);
            pool.install(|| {
                tracing::dispatcher::with_default(&dispatch, || {
                    run_inner(input, opts, cancel, observer)
                })
            })
        }
        None => run_inner(input, opts, cancel, observer),
    }
}

struct Ctx<'a> {
    started: Instant,
    done: Vec<StageTiming>,
    cancel: &'a CancelToken,
    observer: &'a mut (dyn FnMut(&StageTiming) + Send),
}

impl Ctx<'_> {
    /// Runs one stage: cancel check, span, timing, event, observer.
    fn stage<T>(
        &mut self,
        stage: Stage,
        f: impl FnOnce(&CancelToken) -> Result<(T, u64), ErrKind>,
    ) -> Result<T, Failure> {
        let fail = |done: &[StageTiming], kind: ErrKind| Failure {
            kind,
            stage,
            done: done.to_vec(),
        };
        if let Err(i) = self.cancel.check() {
            return Err(fail(&self.done, i.into()));
        }
        let span = stage.span();
        let _enter = span.enter();
        let t = Instant::now();
        let r = f(self.cancel);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        match r {
            Ok((value, px)) => {
                let timing = StageTiming { stage, ms, px };
                tracing::info!(stage = stage.name(), ms, px, "stage_done");
                self.done.push(timing);
                (self.observer)(&timing);
                Ok(value)
            }
            Err(kind) => {
                tracing::warn!(stage = stage.name(), ms, kind = %kind, "stage_failed");
                Err(fail(&self.done, kind))
            }
        }
    }
}

fn run_inner(
    input: Input<'_>,
    opts: &Options,
    cancel: &CancelToken,
    observer: &mut (dyn FnMut(&StageTiming) + Send),
) -> Result<Output, Failure> {
    let mut cx = Ctx {
        started: Instant::now(),
        done: Vec::with_capacity(Stage::ALL.len()),
        cancel,
        observer,
    };

    // 1. read_probe: read, sniff the magic bytes, probe the header (no pixels allocated).
    let bytes = cx.stage(Stage::ReadProbe, |_| {
        let bytes = match input {
            Input::Path(p) => Bytes::Owned(std::fs::read(p).map_err(|_| ErrKind::Unreadable)?),
            Input::Bytes(b) => Bytes::Borrowed(b),
        };
        sniff(bytes.as_slice()).ok_or(ErrKind::UnsupportedFormat)?;
        let p = probe_with(bytes.as_slice(), &opts.limits).map_err(codec_err)?;
        let px = u64::from(p.width) * u64::from(p.height);
        Ok((bytes, px))
    })?;

    // 2. decode: full decode to RGB8 with the EXIF turn applied once.
    let decoded = cx.stage(Stage::Decode, |_| {
        let d = decode_with(bytes.as_slice(), &opts.limits).map_err(codec_err)?;
        let px = u64::from(d.raster.width) * u64::from(d.raster.height);
        Ok((d, px))
    })?;
    drop(bytes);
    let icc = decoded.icc;
    let source = Arc::new(decoded.raster);
    let src_size = (source.width, source.height);

    // 3. proxy: the 1024 px detection proxy and the ~1.5 MP analysis proxy (plus the 3 MP display
    // level they are cascaded from, and the thumbnail).
    let pyramid = cx.stage(Stage::Proxy, |_| {
        let px = u64::from(source.width) * u64::from(source.height);
        Ok((Pyramid::build_shared(Arc::clone(&source)), px))
    })?;

    // 4. analyse (STAND-IN): the classical detector, the random-weight net or Canny + contours on
    // the detection proxy, as `opts.analyse` says.
    let quad = cx.stage(Stage::Analyse, |_| {
        let proxy = pyramid.level(Level::Detect);
        let px = u64::from(proxy.width) * u64::from(proxy.height);
        let quad = match &opts.analyse {
            Analyse::Classical => detect(proxy).quad,
            Analyse::StandinNet(net) => net.analyse(proxy)?,
            #[cfg(feature = "standin-canny")]
            Analyse::StandinCanny => standin::canny_quad(proxy),
            #[cfg(not(feature = "standin-canny"))]
            Analyse::StandinCanny => return Err(ErrKind::UnsupportedFeature),
        };
        Ok((quad, px))
    })?;
    drop(pyramid);
    let quad_found = quad.is_some();
    let warp = quad.map_or_else(|| QuadWarp::inset_frame(0.0), |q: [Pt; 4]| QuadWarp::new(q));

    // 5. rectify: one composed homography, one Lanczos3 warp.
    let rectified = cx.stage(Stage::Rectify, |cancel| {
        let out = render_quad_cancellable(&source, &warp, Limits::pixels(MAX_PIXELS), cancel)
            .map_err(|e| match e {
                RenderError::DegenerateQuad => ErrKind::Degenerate,
                RenderError::Cancelled => {
                    cancel.check().err().map_or(ErrKind::Cancelled, Into::into)
                }
            })?;
        let px = u64::from(out.width) * u64::from(out.height);
        Ok((out, px))
    })?;
    drop(source);

    // 6. enhance (PROTOTYPE): grey, then a threshold, written back as black and white.
    let enhanced = if opts.enhance == Enhance::Off {
        rectified
    } else {
        cx.stage(Stage::Enhance, |_| {
            let px = u64::from(rectified.width) * u64::from(rectified.height);
            Ok((enhance(rectified, opts.enhance), px))
        })?
    };
    let output_size = (enhanced.width, enhanced.height);

    // 7. encode: JPEG at the requested quality, ICC carried over.
    let bytes = cx.stage(Stage::Encode, |_| {
        let px = u64::from(enhanced.width) * u64::from(enhanced.height);
        let out =
            encode(&enhanced, Format::Jpeg, opts.quality, icc.as_deref()).map_err(codec_err)?;
        Ok((out, px))
    })?;

    let total_ms = cx.started.elapsed().as_secs_f64() * 1e3;
    let report = Report {
        stages: cx.done,
        total_ms,
        source: src_size,
        output: output_size,
        output_bytes: bytes.len(),
        quad_found,
        analyse: opts.analyse.label(),
    };
    tracing::info!(
        total_ms,
        sum_ms = report.stage_sum_ms(),
        out_bytes = bytes.len(),
        "pipeline_done"
    );
    Ok(Output { bytes, report })
}

/// The `--timings` table: one line per stage with its PROVISIONAL Table A budget, then the sum and
/// the total. Stand-in stages are labelled so nobody reads them as final.
pub fn format_timings(r: &Report) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<10} {:>10} {:>12} {:>11} {:>6}",
        "stage", "ms", "px", "budget ms", "x"
    );
    for t in &r.stages {
        let note = match t.stage {
            Stage::Analyse => format!("  {}", r.analyse),
            Stage::Enhance => "  PROTOTYPE (grey + threshold)".to_owned(),
            _ => String::new(),
        };
        let b = t.stage.budget_ms();
        let _ = writeln!(
            s,
            "{:<10} {:>10.2} {:>12} {:>11.0} {:>6.2}{note}",
            t.stage.name(),
            t.ms,
            t.px,
            b,
            t.ms / b
        );
    }
    let _ = writeln!(
        s,
        "{:<10} {:>10.2}   stage sum; total {:.2} ms (glue {:.2} ms); chained budget {:.0} ms (PROVISIONAL)",
        "sum",
        r.stage_sum_ms(),
        r.total_ms,
        r.total_ms - r.stage_sum_ms(),
        chained_budget_ms()
    );
    let _ = write!(
        s,
        "{}x{} -> {}x{}, {} bytes, quad {}",
        r.source.0,
        r.source.1,
        r.output.0,
        r.output.1,
        r.output_bytes,
        if r.quad_found {
            "found"
        } else {
            "not found (full frame)"
        }
    );
    s
}

/// Prints `stage_done` and the other pipeline events to stderr until the guard drops
/// (`auto-crop dev-pipeline --trace`).
pub fn trace_to_stderr() -> tracing::subscriber::DefaultGuard {
    let sub = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_default(sub)
}

/// Bytes that are either owned (a file read) or borrowed from the caller.
enum Bytes<'a> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
}

impl Bytes<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Owned(v) => v,
            Bytes::Borrowed(b) => b,
        }
    }
}

/// Luma of an RGB8 raster, parallel by rows of 64 Ki pixels.
fn luma(r: &Raster) -> Vec<u8> {
    const CHUNK: usize = 1 << 16;
    let mut grey = vec![0u8; r.width as usize * r.height as usize];
    grey.par_chunks_mut(CHUNK)
        .zip(r.data.par_chunks(CHUNK * 3))
        .for_each(|(out, rgb)| {
            for (o, px) in out.iter_mut().zip(rgb.as_chunks::<3>().0) {
                // BT.601 weights 77/150/29 over 256: exact in u32, no float.
                let y = 77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]);
                *o = ((y + 128) >> 8) as u8;
            }
        });
    grey
}

/// Grey plus a threshold, back into the RGB raster in place.
fn enhance(mut r: Raster, mode: Enhance) -> Raster {
    const CHUNK: usize = 1 << 16;
    let grey = luma(&r);
    let bw = match mode {
        Enhance::Sauvola => sauvola(&grey, r.width, r.height, 51, 0.2, 128.0),
        _ => binarize(&grey, otsu_from_histogram(&histogram(&grey))),
    };
    drop(grey);
    r.data
        .par_chunks_mut(CHUNK * 3)
        .zip(bw.par_chunks(CHUNK))
        .for_each(|(rgb, v)| {
            for (px, b) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(v) {
                px.fill(*b);
            }
        });
    r
}

#[cfg(test)]
mod tests;
