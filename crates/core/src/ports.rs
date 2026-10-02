// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Ports (PLAN 2.4): the pixel containers and the traits behind the pipeline stages. `core`
//! defines them; `codecs`, `imgproc`, the worker and the inference runtime implement them, so the
//! CLI, GUI and benchmark harness run the same code. Nothing here performs I/O: decoders take
//! bytes, never paths.
//!
//! Dependency direction: `codecs` and `imgproc` depend on `core`, never the reverse.

use crate::cancel::CancelToken;
use crate::edit::EditState;
use crate::error::ErrKind;
use crate::geometry::ExifOrientation;
use crate::output::{OutFormat, OutputSpec};
use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// Pixels
// ---------------------------------------------------------------------------------------------

/// Sample layout of a [`Raster`]. Colour data is interleaved, row-major, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    Gray8,
    Rgb8,
    Gray16,
    Rgb16,
}

impl PixelFormat {
    pub const fn channels(self) -> usize {
        match self {
            PixelFormat::Gray8 | PixelFormat::Gray16 => 1,
            PixelFormat::Rgb8 | PixelFormat::Rgb16 => 3,
        }
    }

    pub const fn bytes_per_sample(self) -> usize {
        match self {
            PixelFormat::Gray8 | PixelFormat::Rgb8 => 1,
            PixelFormat::Gray16 | PixelFormat::Rgb16 => 2,
        }
    }

    pub const fn bytes_per_pixel(self) -> usize {
        self.channels() * self.bytes_per_sample()
    }

    pub const fn bit_depth(self) -> u8 {
        (self.bytes_per_sample() * 8) as u8
    }
}

/// Estimated bytes of a `width` x `height` image in `format`, saturating (never overflows).
pub fn est_bytes(width: u32, height: u32, format: PixelFormat) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(format.bytes_per_pixel() as u64)
}

/// Owned samples; 16-bit data is stored as `u16` so it is always aligned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl Samples {
    pub fn len(&self) -> usize {
        match self {
            Samples::U8(v) => v.len(),
            Samples::U16(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An owned image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    width: u32,
    height: u32,
    format: PixelFormat,
    samples: Samples,
}

impl Raster {
    /// A black image. Fails with `TooLarge` if the size does not fit in memory addressing.
    pub fn zeroed(width: u32, height: u32, format: PixelFormat) -> Result<Self, ErrKind> {
        let n = Self::sample_count(width, height, format)?;
        let samples = if format.bytes_per_sample() == 1 {
            Samples::U8(vec![0; n])
        } else {
            Samples::U16(vec![0; n])
        };
        Ok(Self {
            width,
            height,
            format,
            samples,
        })
    }

    /// Wraps existing samples; `None` if the sample type or count does not match the format.
    pub fn from_samples(
        width: u32,
        height: u32,
        format: PixelFormat,
        samples: Samples,
    ) -> Option<Self> {
        let n = Self::sample_count(width, height, format).ok()?;
        let type_ok = matches!(
            (&samples, format.bytes_per_sample()),
            (Samples::U8(_), 1) | (Samples::U16(_), 2)
        );
        (type_ok && samples.len() == n).then_some(Self {
            width,
            height,
            format,
            samples,
        })
    }

    fn sample_count(width: u32, height: u32, format: PixelFormat) -> Result<usize, ErrKind> {
        usize::try_from(u64::from(width) * u64::from(height) * format.channels() as u64)
            .map_err(|_| ErrKind::TooLarge)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn samples(&self) -> &Samples {
        &self.samples
    }

    pub fn into_samples(self) -> Samples {
        self.samples
    }

    /// Bytes this image occupies; the memory budget and the limits use it.
    pub fn est_bytes(&self) -> u64 {
        est_bytes(self.width, self.height, self.format)
    }

    pub fn view(&self) -> RasterView<'_> {
        RasterView {
            width: self.width,
            height: self.height,
            format: self.format,
            stride: self.width as usize * self.format.channels(),
            samples: match &self.samples {
                Samples::U8(v) => SampleSlice::U8(v),
                Samples::U16(v) => SampleSlice::U16(v),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleSlice<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

/// A borrowed, possibly strided window onto a [`Raster`] (or any other buffer): how kernels read
/// bands and tiles without copying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterView<'a> {
    width: u32,
    height: u32,
    format: PixelFormat,
    /// Samples (not bytes) from the start of one row to the next.
    stride: usize,
    samples: SampleSlice<'a>,
}

impl<'a> RasterView<'a> {
    /// A view over `samples` with an explicit row stride (in samples); `None` if the buffer is
    /// too short for the described window or the stride is smaller than a row.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        samples: SampleSlice<'a>,
    ) -> Option<Self> {
        let row = width as usize * format.channels();
        let needed = if height == 0 {
            0
        } else {
            stride.checked_mul(height as usize - 1)?.checked_add(row)?
        };
        let (len, type_ok) = match samples {
            SampleSlice::U8(s) => (s.len(), format.bytes_per_sample() == 1),
            SampleSlice::U16(s) => (s.len(), format.bytes_per_sample() == 2),
        };
        (type_ok && stride >= row && len >= needed).then_some(Self {
            width,
            height,
            format,
            stride,
            samples,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn samples(&self) -> SampleSlice<'a> {
        self.samples
    }

    pub fn est_bytes(&self) -> u64 {
        est_bytes(self.width, self.height, self.format)
    }

    /// Rows `y0..y1` (clamped) as a view: a band for a strip-wise kernel.
    pub fn rows(&self, y0: u32, y1: u32) -> RasterView<'a> {
        let y1 = y1.min(self.height);
        let y0 = y0.min(y1);
        let off = y0 as usize * self.stride;
        let samples = match self.samples {
            SampleSlice::U8(s) => SampleSlice::U8(&s[off.min(s.len())..]),
            SampleSlice::U16(s) => SampleSlice::U16(&s[off.min(s.len())..]),
        };
        RasterView {
            width: self.width,
            height: y1 - y0,
            format: self.format,
            stride: self.stride,
            samples,
        }
    }

    /// The row `y` as 8-bit samples, if this view is 8-bit and `y` is in range.
    pub fn row_u8(&self, y: u32) -> Option<&'a [u8]> {
        if y >= self.height {
            return None;
        }
        match self.samples {
            SampleSlice::U8(s) => {
                let o = y as usize * self.stride;
                s.get(o..o + self.width as usize * self.format.channels())
            }
            SampleSlice::U16(_) => None,
        }
    }

    /// The row `y` as 16-bit samples, if this view is 16-bit and `y` is in range.
    pub fn row_u16(&self, y: u32) -> Option<&'a [u16]> {
        if y >= self.height {
            return None;
        }
        match self.samples {
            SampleSlice::U16(s) => {
                let o = y as usize * self.stride;
                s.get(o..o + self.width as usize * self.format.channels())
            }
            SampleSlice::U8(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Probe, limits, requests
// ---------------------------------------------------------------------------------------------

/// A container format the sniffer recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Tiff,
    Webp,
    Heic,
    Avif,
    Gif,
    Bmp,
    Jxl,
}

/// What a header read tells us, before any pixel allocation.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub format: ImageFormat,
    /// Stored dimensions, before the EXIF turn.
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub channels: u8,
    pub frames: u32,
    pub orientation: ExifOrientation,
    pub icc: Option<Arc<[u8]>>,
}

impl Probe {
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn oriented_dims(&self) -> (u32, u32) {
        self.orientation.oriented_dims(self.width, self.height)
    }

    /// Bytes a full decode to 8-bit RGB would take.
    pub fn est_bytes_rgb8(&self) -> u64 {
        est_bytes(self.width, self.height, PixelFormat::Rgb8)
    }
}

/// How much of the image the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Want {
    #[default]
    Full,
    /// The smallest decoder-native reduction (for JPEG: 1/8, 1/4, 1/2, 1) that keeps the long
    /// edge at least `min_edge`; never upscales.
    Scaled { min_edge: u32 },
}

/// Caps applied before any allocation (PLAN 3.10.1). Defaults are the shipping values; the
/// Advanced setting, `--max-pixels` and "allow this file" raise `max_pixels` to at most
/// [`DecodeLimits::HARD_MAX_PIXELS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    pub max_pixels: u64,
    pub max_file_bytes: u64,
    pub max_metadata_bytes: u64,
    pub max_scans: u32,
    pub max_frames: u32,
}

impl DecodeLimits {
    /// 100 MP (C1).
    pub const DEFAULT_MAX_PIXELS: u64 = 100_000_000;
    /// The worker refuses more than this whatever the setting says.
    pub const HARD_MAX_PIXELS: u64 = 500_000_000;

    /// The same limits with the pixel cap raised (or lowered), never above the hard maximum.
    pub fn with_max_pixels(mut self, px: u64) -> Self {
        self.max_pixels = px.min(Self::HARD_MAX_PIXELS);
        self
    }

    /// `TooLarge` if `width` x `height` exceeds the pixel cap.
    pub fn check_dims(&self, width: u32, height: u32) -> Result<(), ErrKind> {
        if u64::from(width) * u64::from(height) > self.max_pixels {
            Err(ErrKind::TooLarge)
        } else {
            Ok(())
        }
    }

    /// Checks a probe against every limit it can be checked against.
    pub fn check_probe(&self, p: &Probe) -> Result<(), ErrKind> {
        self.check_dims(p.width, p.height)?;
        if p.frames > self.max_frames {
            return Err(ErrKind::TooLarge);
        }
        if p.icc
            .as_ref()
            .is_some_and(|i| i.len() as u64 > self.max_metadata_bytes)
        {
            return Err(ErrKind::TooLarge);
        }
        Ok(())
    }

    pub fn check_file_size(&self, bytes: u64) -> Result<(), ErrKind> {
        if bytes > self.max_file_bytes {
            Err(ErrKind::TooLarge)
        } else {
            Ok(())
        }
    }
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_pixels: Self::DEFAULT_MAX_PIXELS,
            max_file_bytes: 2 << 30,
            max_metadata_bytes: 16 << 20,
            max_scans: 100,
            max_frames: 10_000,
        }
    }
}

/// A decoded image with the facts the engine keeps about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// EXIF orientation already applied (PLAN 2.1); `probe.orientation` is the tag as found.
    pub raster: Raster,
    pub probe: Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parallelism {
    /// Strip-parallel on rayon's ambient pool: the caller's `pool.install()` picks preview or
    /// batch.
    #[default]
    Strips,
    /// On the calling thread.
    Sequential,
}

/// An axis-aligned region of the rendered output, in output pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderReq {
    /// Output scale relative to the natural size (1.0 = full size).
    pub scale: Option<f32>,
    pub region: Option<Region>,
    pub parallelism: Parallelism,
}

/// Metadata the caller hands to an encoder: only validated data (PLAN 2.4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EncodeMeta {
    pub icc: Option<Arc<[u8]>>,
    pub dpi: Option<(u32, u32)>,
}

/// Identifier of an inference model (name and version), never a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef(pub String);

/// A dense f32 tensor for the inference port.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

// ---------------------------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------------------------

/// Bytes to pixels. Takes bytes, never paths (PLAN 2.4): the caller reads the file once with the
/// size limit applied. The header probe and pixel cap precede any allocation.
pub trait Decoder: Send + Sync {
    fn probe(&self, bytes: &[u8], limits: &DecodeLimits) -> Result<Probe, ErrKind>;

    fn decode(
        &self,
        bytes: &[u8],
        want: Want,
        limits: &DecodeLimits,
        cancel: &CancelToken,
    ) -> Result<Decoded, ErrKind>;
}

/// Pixels plus an `OutputSpec` to bytes. Encoders take only our own pixels and validated
/// metadata, so they run in-process.
pub trait Encoder: Send + Sync {
    /// The container this encoder writes.
    fn format(&self) -> OutFormat;

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind>;
}

/// (source, `EditState`, region, scale) to rasters, one per included item. Pure: the same inputs
/// give the same pixels.
pub trait Renderer: Send + Sync {
    fn render(
        &self,
        src: &RasterView<'_>,
        state: &EditState,
        req: &RenderReq,
        cancel: &CancelToken,
    ) -> Result<Vec<Raster>, ErrKind>;
}

/// A neural-network runtime (ADR 0007). The engine asks for a model by reference and runs it on
/// a tensor; which runtime does the work is the implementation's business.
pub trait InferenceBackend: Send + Sync {
    /// Short runtime name for `doctor` and logs.
    fn name(&self) -> &'static str;

    fn run(
        &self,
        model: &ModelRef,
        input: &Tensor,
        cancel: &CancelToken,
    ) -> Result<Tensor, ErrKind>;
}

/// A HEIC/HEIF decode path (bundled libheif in the worker, or an OS codec). It decodes like any
/// other decoder and also says which engine it is and whether it can decode HEVC.
pub trait HeicBackend: Decoder {
    fn engine(&self) -> &'static str;

    /// False when the HEVC decoder is missing (the `no-hevc` variant).
    fn can_decode_hevc(&self) -> bool;
}

// ---------------------------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------------------------

/// Decoder stub: recognises nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullDecoder;

impl Decoder for NullDecoder {
    fn probe(&self, _: &[u8], _: &DecodeLimits) -> Result<Probe, ErrKind> {
        Err(ErrKind::UnsupportedFormat)
    }
    fn decode(
        &self,
        _: &[u8],
        _: Want,
        _: &DecodeLimits,
        _: &CancelToken,
    ) -> Result<Decoded, ErrKind> {
        Err(ErrKind::UnsupportedFormat)
    }
}

/// Encoder stub: writes nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEncoder;

impl Encoder for NullEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::KeepSource
    }
    fn encode(
        &self,
        _: &RasterView<'_>,
        _: &OutputSpec,
        _: &EncodeMeta,
        _: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        Err(ErrKind::UnsupportedOutput)
    }
}

/// Renderer stub: renders nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullRenderer;

impl Renderer for NullRenderer {
    fn render(
        &self,
        _: &RasterView<'_>,
        _: &EditState,
        _: &RenderReq,
        _: &CancelToken,
    ) -> Result<Vec<Raster>, ErrKind> {
        Err(ErrKind::Internal)
    }
}

/// Inference stub: no runtime.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullInference;

impl InferenceBackend for NullInference {
    fn name(&self) -> &'static str {
        "none"
    }
    fn run(&self, _: &ModelRef, _: &Tensor, _: &CancelToken) -> Result<Tensor, ErrKind> {
        Err(ErrKind::ModelLoadFailed)
    }
}

/// HEIC stub: no HEIC support in this build.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullHeic;

impl Decoder for NullHeic {
    fn probe(&self, _: &[u8], _: &DecodeLimits) -> Result<Probe, ErrKind> {
        Err(ErrKind::HevcDecoderMissing)
    }
    fn decode(
        &self,
        _: &[u8],
        _: Want,
        _: &DecodeLimits,
        _: &CancelToken,
    ) -> Result<Decoded, ErrKind> {
        Err(ErrKind::HevcDecoderMissing)
    }
}

impl HeicBackend for NullHeic {
    fn engine(&self) -> &'static str {
        "none"
    }
    fn can_decode_hevc(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object safety: every port can be used as a trait object, boxed and shared across threads.
    #[allow(dead_code)]
    fn object_safety(
        d: &dyn Decoder,
        e: &dyn Encoder,
        r: &dyn Renderer,
        i: &dyn InferenceBackend,
        h: &dyn HeicBackend,
    ) {
        let _: Box<dyn Decoder> = Box::new(NullDecoder);
        let _: Arc<dyn Encoder> = Arc::new(NullEncoder);
        let _: Arc<dyn Renderer> = Arc::new(NullRenderer);
        let _: Box<dyn InferenceBackend> = Box::new(NullInference);
        let _: Arc<dyn HeicBackend> = Arc::new(NullHeic);
        // A HEIC backend is usable wherever a decoder is.
        let _: &dyn Decoder = h;
        let _ = (d, e, r, i);
    }

    fn sample_probe(w: u32, h: u32) -> Probe {
        Probe {
            format: ImageFormat::Jpeg,
            width: w,
            height: h,
            bit_depth: 8,
            channels: 3,
            frames: 1,
            orientation: ExifOrientation::Rotate90Cw,
            icc: None,
        }
    }

    /// A mock decoder that "decodes" a 2x2 grey image whatever the bytes are.
    struct Mock;

    impl Decoder for Mock {
        fn probe(&self, bytes: &[u8], limits: &DecodeLimits) -> Result<Probe, ErrKind> {
            if bytes.is_empty() {
                return Err(ErrKind::Corrupt);
            }
            let p = sample_probe(2, 2);
            limits.check_probe(&p)?;
            Ok(p)
        }
        fn decode(
            &self,
            bytes: &[u8],
            _: Want,
            limits: &DecodeLimits,
            cancel: &CancelToken,
        ) -> Result<Decoded, ErrKind> {
            cancel.check().map_err(ErrKind::from)?;
            let probe = self.probe(bytes, limits)?;
            Ok(Decoded {
                raster: Raster::zeroed(2, 2, PixelFormat::Gray8)?,
                probe,
            })
        }
    }

    impl Encoder for Mock {
        fn format(&self) -> OutFormat {
            OutFormat::Png
        }
        fn encode(
            &self,
            src: &RasterView<'_>,
            _: &OutputSpec,
            _: &EncodeMeta,
            _: &CancelToken,
        ) -> Result<Vec<u8>, ErrKind> {
            Ok(vec![0; src.est_bytes() as usize])
        }
    }

    impl Renderer for Mock {
        fn render(
            &self,
            src: &RasterView<'_>,
            state: &EditState,
            _: &RenderReq,
            _: &CancelToken,
        ) -> Result<Vec<Raster>, ErrKind> {
            let n = state.included().count().max(1);
            (0..n)
                .map(|_| Raster::zeroed(src.width(), src.height(), src.format()))
                .collect()
        }
    }

    impl InferenceBackend for Mock {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn run(&self, _: &ModelRef, t: &Tensor, _: &CancelToken) -> Result<Tensor, ErrKind> {
            Ok(t.clone())
        }
    }

    #[test]
    fn mocks_drive_the_pipeline_through_trait_objects() {
        let dec: Box<dyn Decoder> = Box::new(Mock);
        let enc: Box<dyn Encoder> = Box::new(Mock);
        let ren: Box<dyn Renderer> = Box::new(Mock);
        let inf: Box<dyn InferenceBackend> = Box::new(Mock);
        let tok = CancelToken::never();

        let limits = DecodeLimits::default();
        let d = dec.decode(b"x", Want::Full, &limits, &tok).unwrap();
        assert_eq!(d.probe.oriented_dims(), (2, 2));
        let out = ren
            .render(
                &d.raster.view(),
                &EditState::default(),
                &RenderReq::default(),
                &tok,
            )
            .unwrap();
        assert_eq!(out.len(), 1);
        let bytes = enc
            .encode(
                &out[0].view(),
                &OutputSpec::default(),
                &EncodeMeta::default(),
                &tok,
            )
            .unwrap();
        assert_eq!(bytes.len(), 4);
        let t = Tensor {
            shape: vec![1],
            data: vec![0.5],
        };
        assert_eq!(inf.run(&ModelRef("m".into()), &t, &tok).unwrap(), t);
        assert_eq!(inf.name(), "mock");

        // Errors flow as codes; a cancelled token stops the decode.
        assert_eq!(dec.probe(b"", &limits), Err(ErrKind::Corrupt));
        let cancelled = CancelToken::new_batch();
        cancelled.cancel();
        assert_eq!(
            dec.decode(b"x", Want::Full, &limits, &cancelled)
                .unwrap_err(),
            ErrKind::Cancelled
        );
    }

    #[test]
    fn stubs_fail_with_the_documented_codes() {
        let tok = CancelToken::never();
        let l = DecodeLimits::default();
        assert_eq!(NullDecoder.probe(b"x", &l), Err(ErrKind::UnsupportedFormat));
        assert_eq!(
            NullDecoder.decode(b"x", Want::Full, &l, &tok).unwrap_err(),
            ErrKind::UnsupportedFormat
        );
        let r = Raster::zeroed(1, 1, PixelFormat::Gray8).unwrap();
        assert_eq!(
            NullEncoder
                .encode(
                    &r.view(),
                    &OutputSpec::default(),
                    &EncodeMeta::default(),
                    &tok
                )
                .unwrap_err(),
            ErrKind::UnsupportedOutput
        );
        assert!(
            NullRenderer
                .render(
                    &r.view(),
                    &EditState::default(),
                    &RenderReq::default(),
                    &tok
                )
                .is_err()
        );
        assert_eq!(
            NullInference
                .run(
                    &ModelRef("m".into()),
                    &Tensor {
                        shape: vec![],
                        data: vec![]
                    },
                    &tok
                )
                .unwrap_err(),
            ErrKind::ModelLoadFailed
        );
        assert_eq!(
            NullHeic.decode(b"x", Want::Full, &l, &tok).unwrap_err(),
            ErrKind::HevcDecoderMissing
        );
        assert!(!NullHeic.can_decode_hevc());
        assert_eq!(NullHeic.engine(), "none");
    }

    #[test]
    fn raster_sizes_and_est_bytes() {
        for (f, bpp) in [
            (PixelFormat::Gray8, 1),
            (PixelFormat::Rgb8, 3),
            (PixelFormat::Gray16, 2),
            (PixelFormat::Rgb16, 6),
        ] {
            assert_eq!(f.bytes_per_pixel(), bpp);
            let r = Raster::zeroed(5, 4, f).unwrap();
            assert_eq!(r.est_bytes(), 20 * bpp as u64);
            assert_eq!(r.view().est_bytes(), r.est_bytes());
            assert_eq!(r.samples().len(), 20 * f.channels());
        }
        assert_eq!(PixelFormat::Rgb16.bit_depth(), 16);
        // Saturating, never overflowing, for absurd sizes.
        assert_eq!(est_bytes(u32::MAX, u32::MAX, PixelFormat::Rgb16), u64::MAX);
        assert!(Raster::from_samples(2, 2, PixelFormat::Rgb8, Samples::U8(vec![0; 11])).is_none());
        assert!(Raster::from_samples(2, 2, PixelFormat::Rgb8, Samples::U16(vec![0; 12])).is_none());
        assert!(Raster::from_samples(2, 2, PixelFormat::Rgb8, Samples::U8(vec![0; 12])).is_some());
    }

    #[test]
    fn views_window_rows_and_check_bounds() {
        let data: Vec<u8> = (0..4 * 3).collect();
        let r = Raster::from_samples(4, 3, PixelFormat::Gray8, Samples::U8(data)).unwrap();
        let v = r.view();
        assert_eq!(v.row_u8(1), Some(&[4u8, 5, 6, 7][..]));
        assert_eq!(v.row_u8(3), None);
        assert_eq!(v.row_u16(0), None);
        let band = v.rows(1, 3);
        assert_eq!((band.width(), band.height()), (4, 2));
        assert_eq!(band.row_u8(0), Some(&[4u8, 5, 6, 7][..]));
        assert_eq!(v.rows(2, 99).height(), 1);
        assert_eq!(v.rows(9, 99).height(), 0);

        // A strided view over a wider buffer.
        let buf: Vec<u16> = (0..16).collect();
        let sv = RasterView::new(2, 3, PixelFormat::Gray16, 6, SampleSlice::U16(&buf)).unwrap();
        assert_eq!(sv.row_u16(1), Some(&[6u16, 7][..]));
        assert!(RasterView::new(2, 3, PixelFormat::Gray16, 6, SampleSlice::U8(&[0; 99])).is_none());
        assert!(RasterView::new(2, 3, PixelFormat::Gray16, 1, SampleSlice::U16(&buf)).is_none());
        assert!(RasterView::new(2, 9, PixelFormat::Gray16, 6, SampleSlice::U16(&buf)).is_none());
    }

    #[test]
    fn limits_check_dims_frames_and_metadata() {
        let l = DecodeLimits::default();
        assert_eq!(l.max_pixels, 100_000_000);
        assert_eq!(l.check_dims(10_000, 10_000), Ok(()));
        assert_eq!(l.check_dims(10_001, 10_000), Err(ErrKind::TooLarge));
        assert_eq!(l.check_dims(60_000, 60_000), Err(ErrKind::TooLarge));
        let mut p = sample_probe(100, 100);
        assert_eq!(l.check_probe(&p), Ok(()));
        p.frames = 10_001;
        assert_eq!(l.check_probe(&p), Err(ErrKind::TooLarge));
        p.frames = 1;
        p.icc = Some(vec![0u8; (16 << 20) + 1].into());
        assert_eq!(l.check_probe(&p), Err(ErrKind::TooLarge));
        assert_eq!(
            l.check_file_size(l.max_file_bytes + 1),
            Err(ErrKind::TooLarge)
        );
        // The raised cap never passes the hard maximum.
        assert_eq!(
            l.with_max_pixels(u64::MAX).max_pixels,
            DecodeLimits::HARD_MAX_PIXELS
        );
        assert_eq!(l.with_max_pixels(5).max_pixels, 5);
    }
}
