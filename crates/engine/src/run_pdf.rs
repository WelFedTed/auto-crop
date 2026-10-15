// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Scan to PDF: the crops of a selection become the pages of ONE multi-page PDF (ROADMAP M11.11
//! `ManyToOne`, M11.31, M11.36, M11.37, M11.38; PLAN 3.3 "PDF"). [`Engine::save_run_pdf`] writes a
//! selection that is known up front; [`PdfRun`] ([`Engine::begin_run_pdf`]) takes one scan at a time
//! (the CLI feeds it as its workers finish, in order).
//!
//! * **Pages** are the included crops of the selected scans in the order given (the review grid's
//!   order), each scan's crops in their own reading order. Every crop is rendered as it stands in
//!   the edit state (geometry, curved page, enhancement). One page is rendered, encoded and written
//!   at a time; nothing but the current scan's decoded pixels is in memory, so a 200-page document
//!   needs the memory of one page, not of the document.
//! * **Page types**: an unedited JPEG crop that the lossless transform can cut exactly is embedded
//!   **as it is** (no new JPEG generation: "pass-through"); any other colour crop is a JPEG page at
//!   the save's quality; an enhanced grey page is a lossless Flate page, or Group 4 with `one_bit`.
//!   The page size follows the density (`dpi` of the options, else the source's, else 300 dpi).
//! * **Safe write** (copy semantics: the originals are only read, so there is no backup): the PDF is
//!   written to a temp file beside the target, synced, then **verified by parse-back**: the file
//!   holds exactly the bytes that were written (blake3), then `pdf::read_pages_streaming` reads the
//!   structure back with seeks (one page in memory at a time); every page must have the codec,
//!   size, media box and profile that were planned, its stream must be the stream that was written
//!   (blake3) and, in `Full` mode, decode to the pixels (lossless pages) or the luma fingerprint
//!   (JPEG pages) of what was rendered. Only then is the temp moved to its name with a no-clobber
//!   link: an existing file is never replaced; the name becomes `Name (2).pdf` and the result says
//!   so (`pdf.renamed`).
//! * **Cancellation** is checked before every page and between the steps; a cancel, an error or a
//!   failed page removes the temp and leaves no output (also when the run is dropped).

use crate::commit::{Content, Expect, VerifyMode, gray_pixel_hash, luma_fingerprint};
use crate::curved::PageShape;
use crate::engine::{Engine, lock};
use crate::error::{ErrKind, Result, codec_err};
use crate::fsplan::{OnCollision, PlanError, PlanInput, ReservedKeys, path_key};
use crate::group::move_no_clobber;
use crate::lossless::{Backend, try_lossless};
use crate::output::{EngineOptions, QualitySetting, SourceMeta};
use crate::scan::Reservation;
use crate::space::{Need, preflight};
use crate::util::{blake3_hex, new_id};
use auto_crop_codecs::Format;
use auto_crop_codecs::encoders::bilevel::{BilevelImage, DEFAULT_CUT};
use auto_crop_codecs::encoders::default_jpeg_encoder;
use auto_crop_codecs::encoders::pdf::{
    MAX_PAGES, PageCodec, PageImage, PageInfo, PdfPageRead, PdfWriter, jpeg_info,
    read_pages_streaming, strip_jpeg,
};
use auto_crop_core::output::Quality;
use auto_crop_core::ports::{EncodeMeta, PixelFormat, RasterView, SampleSlice};
use auto_crop_core::{CancelToken, Enhance, OutputSpec};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::enhance::{Enhanced, GrayImage};
use auto_crop_imgproc::render::Limits;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Notice: a page had no density to go by and was sized at 300 dpi.
pub const NOTICE_PDF_DPI_ASSUMED: &str = "pdf.dpi_assumed";
/// Notice: a page was scaled to stay within the page size limits.
pub const NOTICE_PDF_PAGE_SCALED: &str = "pdf.page_scaled";
/// Notice: the file name was taken, so the PDF was saved under another name.
pub const NOTICE_PDF_RENAMED: &str = "pdf.renamed";
/// Notice: a page is the original JPEG data, cut without re-encoding.
pub const NOTICE_JPEG_PASS_THROUGH: &str = "pdf.jpeg_pass_through";

/// Which crops go into the PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfSelect {
    /// The opened image (a scan).
    pub item: u32,
    /// One crop of it; `None` = every included crop, in reading order.
    pub crop: Option<u32>,
}

/// What the caller chooses about the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PdfRunOptions {
    /// Pixel density of every page; `None` = each crop's own, 300 when its source says none.
    pub dpi: Option<u32>,
    /// The document title; `None` = the file name without its extension.
    pub title: Option<String>,
    /// Enhanced black-and-white pages as 1-bit Group 4.
    pub one_bit: bool,
    /// The JPEG quality of colour pages (a pass-through page keeps its own).
    pub quality: QualitySetting,
}

/// Where a run is, for a progress bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfRunProgress {
    /// `rendering`, `verifying` or `saving`.
    pub stage: &'static str,
    pub done: u32,
    pub total: u32,
}

/// A finished document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfRunSaved {
    /// Where it is (the planned name, or the next free one).
    pub path: PathBuf,
    pub pages: u32,
    pub bytes: u64,
    /// Pages that are the original JPEG data.
    pub pass_through_pages: u32,
    pub notices: Vec<String>,
    /// The page sizes in points, in page order.
    pub page_sizes_pt: Vec<(f64, f64)>,
}

/// Steps a test can fail (`PdfRun::set_hook`): returning `Err` there is a failure of that step.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfStep {
    /// Page `i` (0-based) was written.
    Page(usize),
    /// The document is complete and synced.
    Written,
    /// The parse-back passed.
    Verified,
}

/// A source scan, as far as the run needs it.
struct Source {
    path: PathBuf,
    original_path: PathBuf,
    format: Format,
    blake3: String,
    icc: Option<Arc<Vec<u8>>>,
}

/// One planned page.
struct Planned {
    shape: PageShape,
    enhance: Enhance,
}

/// What a written page must read back as.
struct PageExpect {
    expect: Expect,
    /// blake3 of the image stream (empty: checked by decoding only).
    stream_blake3: String,
    media_pt: (f64, f64),
    codec: PageCodec,
}

/// A writer that hashes and counts what goes through it.
struct Hashing<W: Write> {
    inner: W,
    hasher: blake3::Hasher,
    len: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

type Doc = PdfWriter<Hashing<BufWriter<fs::File>>>;

fn spec(quality: u8) -> OutputSpec {
    OutputSpec {
        quality: Quality::Fixed { value: quality },
        ..OutputSpec::default()
    }
}

fn gray_view(g: &GrayImage) -> Result<RasterView<'_>> {
    RasterView::new(
        g.width,
        g.height,
        PixelFormat::Gray8,
        g.width as usize,
        SampleSlice::U8(&g.data),
    )
    .ok_or(ErrKind::Internal)
}

/// blake3 of the RGB expansion of a thresholded grey page (what a 1-bit page decodes to).
fn bilevel_hash(b: &BilevelImage, w: u32, h: u32) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut row = vec![0u8; w as usize * 3];
    for y in 0..h {
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            px.fill(if b.is_black(x as u32, y) { 0 } else { 255 });
        }
        hasher.update(&row);
    }
    hasher.finalize().to_hex().to_string()
}

impl Engine {
    /// Writes the selected crops as the pages of one PDF at `out` (see the module docs).
    pub fn save_run_pdf(
        &self,
        select: &[PdfSelect],
        out: &Path,
        o: &PdfRunOptions,
        cancel: &CancelToken,
        progress: &dyn Fn(PdfRunProgress),
    ) -> Result<PdfRunSaved> {
        self.save_run_pdf_with(select, out, o, cancel, progress, None)
    }

    /// [`Engine::save_run_pdf`] with a hook that is called after each step and can fail it.
    #[doc(hidden)]
    pub fn save_run_pdf_with(
        &self,
        select: &[PdfSelect],
        out: &Path,
        o: &PdfRunOptions,
        cancel: &CancelToken,
        progress: &dyn Fn(PdfRunProgress),
        hook: Option<Box<dyn Fn(PdfStep) -> Result<()>>>,
    ) -> Result<PdfRunSaved> {
        // The size of what will be read, for the free-space check; the count, for the progress bar.
        let mut bytes = 0u64;
        let mut pages = 0u32;
        let mut seen: Vec<u32> = Vec::new();
        for s in select {
            let item = self.item(s.item).ok_or(ErrKind::Internal)?;
            let it = lock(&item);
            if !seen.contains(&s.item) {
                seen.push(s.item);
                bytes += it.snapshot.size;
            }
            if let Some(h) = &it.history {
                pages += h
                    .current()
                    .included()
                    .filter(|c| s.crop.is_none_or(|id| id == c.id.0))
                    .count() as u32;
            }
        }
        let mut run = self.begin_run_pdf(out, o, cancel, bytes)?;
        run.expect_pages(pages);
        if let Some(h) = hook {
            run.hook = Some(h);
        }
        for s in select {
            run.add(*s, progress)?;
        }
        run.finish(progress)
    }

    /// Starts a document at `out` (a `.pdf` path; the folder must exist). `expected_bytes` is the
    /// size of what will be read (a free-space check, 0 skips it).
    pub fn begin_run_pdf<'e>(
        &'e self,
        out: &Path,
        o: &PdfRunOptions,
        cancel: &CancelToken,
        expected_bytes: u64,
    ) -> Result<PdfRun<'e>> {
        let dir = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        if !dir.is_dir() {
            return Err(ErrKind::Unreadable);
        }
        if cancel.is_cancelled() {
            return Err(ErrKind::Cancelled);
        }
        if expected_bytes > 0 {
            preflight(&Need::new(&dir, expected_bytes, None))?;
        }
        let stem = out
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Scans".to_owned());
        let temp = dir.join(format!(".autocrop-{}.tmp", new_id()));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| ErrKind::from_io(&e))?;
        let sink = Hashing {
            inner: BufWriter::with_capacity(1 << 20, file),
            hasher: blake3::Hasher::new(),
            len: 0,
        };
        let mut doc = match PdfWriter::new(sink) {
            Ok(d) => d,
            Err(_) => {
                let _ = fs::remove_file(&temp);
                return Err(ErrKind::EncodeFailed);
            }
        };
        doc.set_title(o.title.as_deref().unwrap_or(&stem));
        Ok(PdfRun {
            engine: self,
            o: o.clone(),
            opts: self.options(),
            cancel: cancel.clone(),
            out: out.to_path_buf(),
            stem,
            dir,
            temp,
            doc: Some(doc),
            expects: Vec::new(),
            notices: Vec::new(),
            sizes: Vec::new(),
            pass_through: 0,
            total_hint: 0,
            hook: None,
            failed: false,
        })
    }
}

/// A document being written (see [`Engine::begin_run_pdf`]). Dropping it unfinished removes the
/// temp file.
pub struct PdfRun<'e> {
    engine: &'e Engine,
    o: PdfRunOptions,
    opts: EngineOptions,
    cancel: CancelToken,
    out: PathBuf,
    stem: String,
    dir: PathBuf,
    temp: PathBuf,
    doc: Option<Doc>,
    expects: Vec<PageExpect>,
    notices: Vec<String>,
    sizes: Vec<(f64, f64)>,
    pass_through: u32,
    total_hint: u32,
    #[allow(clippy::type_complexity)]
    hook: Option<Box<dyn Fn(PdfStep) -> Result<()>>>,
    failed: bool,
}

impl Drop for PdfRun<'_> {
    fn drop(&mut self) {
        self.doc = None;
        if self.temp.exists() {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

impl PdfRun<'_> {
    /// Pages written so far.
    pub fn pages(&self) -> usize {
        self.expects.len()
    }

    /// How many pages the run will have (only for the progress bar).
    pub fn expect_pages(&mut self, n: u32) {
        self.total_hint = n;
    }

    #[doc(hidden)]
    pub fn set_hook(&mut self, h: Box<dyn Fn(PdfStep) -> Result<()>>) {
        self.hook = Some(h);
    }

    fn step(&self, s: PdfStep) -> Result<()> {
        self.hook.as_ref().map_or(Ok(()), |h| h(s))
    }

    fn note(&mut self, n: &str) {
        if !self.notices.iter().any(|x| x == n) {
            self.notices.push(n.to_owned());
        }
    }

    /// Adds the pages of one selection entry; the number of pages added. An error leaves the
    /// document unusable: drop it.
    pub fn add(&mut self, s: PdfSelect, progress: &dyn Fn(PdfRunProgress)) -> Result<usize> {
        let r = self.add_inner(s, progress);
        if r.is_err() {
            self.failed = true;
        }
        r
    }

    fn add_inner(&mut self, s: PdfSelect, progress: &dyn Fn(PdfRunProgress)) -> Result<usize> {
        if self.failed {
            return Err(ErrKind::Internal);
        }
        let item = self.engine.item(s.item).ok_or(ErrKind::Internal)?;
        let (src, state) = {
            let it = lock(&item);
            if it.status != crate::api::ItemStatus::Ready {
                return Err(ErrKind::Internal);
            }
            let state = it
                .history
                .as_ref()
                .ok_or(ErrKind::Internal)?
                .current()
                .clone();
            (
                Source {
                    path: it.path.clone(),
                    original_path: it.original_path.clone(),
                    format: it.format,
                    blake3: it.snapshot.blake3.clone(),
                    icc: it.icc.clone(),
                },
                state,
            )
        };
        let mut planned: Vec<Planned> = Vec::new();
        for c in state.included() {
            if s.crop.is_some_and(|id| id != c.id.0) {
                continue;
            }
            if let Some(shape) = PageShape::of(&c.geometry) {
                planned.push(Planned {
                    shape,
                    enhance: state.effective_enhance(c.id).unwrap_or_default(),
                });
            }
        }
        if planned.is_empty() {
            return Err(ErrKind::NoCrop);
        }
        if self.expects.len() + planned.len() > MAX_PAGES {
            return Err(ErrKind::TooLarge);
        }
        if self.cancel.is_cancelled() {
            return Err(ErrKind::Cancelled);
        }
        crate::fsstate::check_readable(&src.original_path, self.opts.hydrate_cloud_files)?;
        let bytes = fs::read(&src.original_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ErrKind::SourceChanged
            } else {
                ErrKind::from_io(&e)
            }
        })?;
        if src.original_path == src.path && blake3_hex(&bytes) != src.blake3 {
            return Err(ErrKind::SourceChanged);
        }
        let meta = SourceMeta::read(src.format, &bytes, src.icc.clone());
        let mut raster: Option<Raster> = None;
        for p in &planned {
            if self.cancel.is_cancelled() {
                return Err(ErrKind::Cancelled);
            }
            let i = self.expects.len();
            progress(PdfRunProgress {
                stage: "rendering",
                done: i as u32,
                total: self.total_hint.max(i as u32 + 1),
            });
            let (info, pe, passed) = self.one_page(p, &src, &bytes, &meta, &mut raster)?;
            if passed {
                self.pass_through += 1;
                self.note(NOTICE_JPEG_PASS_THROUGH);
            }
            if info.dpi_assumed {
                self.note(NOTICE_PDF_DPI_ASSUMED);
            }
            if info.scaled {
                self.note(NOTICE_PDF_PAGE_SCALED);
            }
            self.sizes.push((info.width_pt, info.height_pt));
            self.expects.push(pe);
            self.step(PdfStep::Page(i))?;
        }
        Ok(planned.len())
    }

    fn one_page(
        &mut self,
        p: &Planned,
        src: &Source,
        bytes: &[u8],
        meta: &SourceMeta,
        raster: &mut Option<Raster>,
    ) -> Result<(PageInfo, PageExpect, bool)> {
        let dpi = self.o.dpi.map(|d| (d, d)).or_else(|| meta.dpi());
        let icc: Option<Arc<[u8]>> = meta
            .icc
            .as_ref()
            .map(|v| Arc::<[u8]>::from(v.as_slice()))
            .filter(|v| !v.is_empty());
        // A 1-bit document has only black-and-white pages: an unenhanced (colour) crop is a typed
        // error, never a silent colour page among them.
        if self.o.one_bit && p.enhance.is_identity() {
            return Err(ErrKind::UnsupportedOutput);
        }
        let (o, opts, cancel) = (&self.o, &self.opts, &self.cancel);
        let doc = self.doc.as_mut().ok_or(ErrKind::Internal)?;

        // The exact path: an unedited JPEG crop that the lossless transform cuts as it is.
        let lossless = (src.format == Format::Jpeg && p.enhance.is_identity())
            .then(|| {
                p.shape
                    .as_quad()
                    .and_then(|q| try_lossless(&mut Backend::new(), bytes, q, meta, opts))
            })
            .flatten()
            .filter(|l| jpeg_info(&l.bytes).is_ok());
        crate::run_isolated(std::panic::AssertUnwindSafe(|| -> Result<_> {
            if let Some(l) = lossless {
                let stripped = strip_jpeg(&l.bytes).map_err(|_| ErrKind::EncodeFailed)?;
                let info = doc
                    .add_page(
                        PageImage::Jpeg {
                            bytes: &l.bytes,
                            icc: icc.as_deref(),
                        },
                        dpi,
                    )
                    .map_err(|_| ErrKind::EncodeFailed)?;
                return Ok((
                    info,
                    PageExpect {
                        expect: Expect {
                            dims: l.dims,
                            format: Format::Pdf,
                            icc: icc
                                .as_deref()
                                .filter(|_| !info.icc_dropped)
                                .map(<[u8]>::to_vec),
                            content: Content::None,
                        },
                        stream_blake3: blake3_hex(&stripped),
                        media_pt: (info.width_pt, info.height_pt),
                        codec: PageCodec::Dct,
                    },
                    true,
                ));
            }
            if raster.is_none() {
                let d = auto_crop_codecs::decode_with(bytes, &opts.limits()).map_err(codec_err)?;
                *raster = Some(d.raster);
            }
            let src_raster = raster.as_ref().ok_or(ErrKind::Internal)?;
            let page = p
                .shape
                .render_enhanced(
                    src_raster,
                    Limits::pixels(opts.max_pixels),
                    p.enhance,
                    cancel,
                )
                .map_err(|e| {
                    if cancel.is_cancelled() {
                        ErrKind::Cancelled
                    } else if e == ErrKind::Cancelled {
                        e
                    } else {
                        ErrKind::NoCrop
                    }
                })?;
            pdf_page(doc, &page.pixels, meta, icc.as_deref(), dpi, o).map(|(i, e)| (i, e, false))
        }))
        .unwrap_or(Err(ErrKind::InternalPanic))
    }

    /// Completes the document: writes the page tree and the cross-reference table, syncs, verifies
    /// by parse-back, and moves it to its name.
    pub fn finish(mut self, progress: &dyn Fn(PdfRunProgress)) -> Result<PdfRunSaved> {
        if self.failed || self.expects.is_empty() {
            return Err(if self.expects.is_empty() {
                ErrKind::NoCrop
            } else {
                ErrKind::Internal
            });
        }
        let total = self.expects.len() as u32;
        let doc = self.doc.take().ok_or(ErrKind::Internal)?;
        let sink = doc.finish().map_err(|_| ErrKind::EncodeFailed)?;
        let Hashing { inner, hasher, len } = sink;
        let file = inner
            .into_inner()
            .map_err(|e| ErrKind::from_io(&e.into_error()))?;
        file.sync_all().map_err(|e| ErrKind::from_io(&e))?;
        drop(file);
        let written_hash = hasher.finalize().to_hex().to_string();
        self.step(PdfStep::Written)?;
        verify_written(
            &self.temp,
            len,
            &written_hash,
            &self.expects,
            self.opts.verify,
            &self.cancel,
            progress,
            total,
        )?;
        self.step(PdfStep::Verified)?;
        progress(PdfRunProgress {
            stage: "saving",
            done: total,
            total,
        });
        if self.cancel.is_cancelled() {
            return Err(ErrKind::Cancelled);
        }
        // The name: planned and reserved, then taken with a no-clobber link; a name that appeared
        // meanwhile makes the plan stale and it is planned again.
        let wanted = self.dir.join(format!("{}.pdf", self.stem));
        let mut attempt = 0;
        let dest = loop {
            attempt += 1;
            let reserved = self.engine.reserved_names(0);
            let plan = crate::fsplan::plan_group(&PlanInput {
                source: &self.out,
                dir: &self.dir,
                count: 1,
                template: None,
                ext: "pdf",
                on_collision: OnCollision::Rename,
                reserved: &reserved,
                own: &[],
            })
            .map_err(|e: PlanError| e.kind())?;
            let r = match Reservation::take(self.engine, &plan.paths, &[]) {
                Ok(r) => r,
                Err(_) if attempt < 4 => continue,
                Err(e) => return Err(e),
            };
            plan.recheck(&ReservedKeys::default(), &[])?;
            let dest = plan.paths[0].clone();
            match move_no_clobber(&self.temp, &dest) {
                Ok(()) => {
                    drop(r);
                    break dest;
                }
                Err(ErrKind::PlanStale) if attempt < 4 => {}
                Err(e) => return Err(e),
            }
        };
        let mut notices = std::mem::take(&mut self.notices);
        if path_key(&dest) != path_key(&wanted) {
            notices.push(NOTICE_PDF_RENAMED.to_owned());
        }
        Ok(PdfRunSaved {
            path: dest,
            pages: total,
            bytes: len,
            pass_through_pages: self.pass_through,
            notices,
            page_sizes_pt: std::mem::take(&mut self.sizes),
        })
    }
}

/// Encodes one rendered page onto `doc` and says what reading it back must give.
fn pdf_page<W: Write>(
    doc: &mut PdfWriter<W>,
    page: &Enhanced,
    meta: &SourceMeta,
    icc: Option<&[u8]>,
    dpi: Option<(u32, u32)>,
    o: &PdfRunOptions,
) -> Result<(PageInfo, PageExpect)> {
    let never = CancelToken::never();
    match page {
        Enhanced::Rgb(r) => {
            if o.one_bit {
                // A 1-bit page is for a black-and-white page; colour is a typed error.
                return Err(ErrKind::UnsupportedOutput);
            }
            let view = crate::output::view(r)?;
            let enc_meta = EncodeMeta {
                icc: icc.map(Arc::from),
                dpi,
            };
            let jpeg = default_jpeg_encoder()
                .encode(
                    &view,
                    &spec(o.quality.jpeg_quality(meta.q_est())),
                    &enc_meta,
                    &never,
                )
                .map_err(|_| ErrKind::EncodeFailed)?;
            let stripped = strip_jpeg(&jpeg).map_err(|_| ErrKind::EncodeFailed)?;
            let info = doc
                .add_page(PageImage::Jpeg { bytes: &jpeg, icc }, dpi)
                .map_err(|_| ErrKind::EncodeFailed)?;
            Ok((
                info,
                PageExpect {
                    expect: Expect {
                        dims: (r.width, r.height),
                        format: Format::Pdf,
                        icc: icc.filter(|_| !info.icc_dropped).map(<[u8]>::to_vec),
                        content: Content::Luma(luma_fingerprint(r)),
                    },
                    stream_blake3: blake3_hex(&stripped),
                    media_pt: (info.width_pt, info.height_pt),
                    codec: PageCodec::Dct,
                },
            ))
        }
        Enhanced::Gray(g) => {
            let dims = (g.width, g.height);
            if o.one_bit {
                let view = gray_view(g)?;
                let b = BilevelImage::from_view(&view, DEFAULT_CUT, &never)
                    .map_err(|_| ErrKind::EncodeFailed)?;
                let info = doc
                    .add_page(PageImage::G4(&b), dpi)
                    .map_err(|_| ErrKind::EncodeFailed)?;
                return Ok((
                    info,
                    PageExpect {
                        expect: Expect {
                            dims,
                            format: Format::Pdf,
                            icc: None,
                            content: Content::Pixels(bilevel_hash(&b, g.width, g.height)),
                        },
                        stream_blake3: String::new(),
                        media_pt: (info.width_pt, info.height_pt),
                        codec: PageCodec::G4,
                    },
                ));
            }
            let info = doc
                .add_page(
                    PageImage::Flate {
                        width: g.width,
                        height: g.height,
                        channels: 1,
                        samples: &g.data,
                        icc: None,
                    },
                    dpi,
                )
                .map_err(|_| ErrKind::EncodeFailed)?;
            Ok((
                info,
                PageExpect {
                    expect: Expect {
                        dims,
                        format: Format::Pdf,
                        icc: None,
                        content: Content::Pixels(gray_pixel_hash(g)),
                    },
                    stream_blake3: String::new(),
                    media_pt: (info.width_pt, info.height_pt),
                    codec: PageCodec::Flate,
                },
            ))
        }
    }
}

/// Reads the temp back: the bytes are the ones written, the structure holds, and every page is the
/// page that was planned.
#[allow(clippy::too_many_arguments)]
fn verify_written(
    temp: &Path,
    len: u64,
    written_hash: &str,
    expects: &[PageExpect],
    mode: VerifyMode,
    cancel: &CancelToken,
    progress: &dyn Fn(PdfRunProgress),
    total: u32,
) -> Result<()> {
    // 1. The file holds exactly the bytes that were written (truncation, a failed flush).
    let f = fs::File::open(temp).map_err(|_| ErrKind::VerifyFailed)?;
    if f.metadata().map_err(|_| ErrKind::VerifyFailed)?.len() != len {
        return Err(ErrKind::VerifyFailed);
    }
    let mut hasher = blake3::Hasher::new();
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf).map_err(|_| ErrKind::VerifyFailed)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    if hasher.finalize().to_hex().as_str() != written_hash {
        return Err(ErrKind::VerifyFailed);
    }
    // 2. The structure, page by page, with seeks.
    let mut f = r.into_inner();
    let mut bad = false;
    let n = read_pages_streaming(&mut f, &mut |i, page| {
        if cancel.is_cancelled() {
            bad = true;
            return false;
        }
        progress(PdfRunProgress {
            stage: "verifying",
            done: i as u32,
            total,
        });
        let ok = expects.get(i).is_some_and(|e| page_matches(&page, e, mode));
        bad |= !ok;
        ok
    });
    if cancel.is_cancelled() {
        return Err(ErrKind::Cancelled);
    }
    match n {
        Some(n) if n == expects.len() && !bad => Ok(()),
        _ => Err(ErrKind::VerifyFailed),
    }
}

fn page_matches(page: &PdfPageRead, e: &PageExpect, mode: VerifyMode) -> bool {
    if page.codec != e.codec
        || (page.width_px, page.height_px) != e.expect.dims
        || (page.media_box.0 - e.media_pt.0).abs() > 0.01
        || (page.media_box.1 - e.media_pt.1).abs() > 0.01
    {
        return false;
    }
    if !e.stream_blake3.is_empty() && blake3_hex(page.stream_bytes()) != e.stream_blake3 {
        return false;
    }
    if let Some(want) = &e.expect.icc
        && page.icc.as_ref() != Some(want)
    {
        return false;
    }
    if mode == VerifyMode::Fast && page.codec == PageCodec::Dct {
        // Fast mode: no full JPEG decode; the stream hash above already proved the bytes.
        return true;
    }
    crate::commit::verify_pdf_page(page, &e.expect).is_ok()
}
